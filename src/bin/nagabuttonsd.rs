//! nagabuttonsd: the part of NagaButtons that reads the mouse and posts the
//! buttons. It links no AppKit, so it stays small, and nothing but presses
//! runs on its run loop. The menu bar app starts it as a child process, so
//! macOS applies the app's Input Monitoring and Accessibility grants to it.
//! Licensed under the MIT licence; see LICENSE.
//!
//! hidutil remaps the mouse's own keys to a key macOS ignores (seizing a
//! keyboard needs root), while this process still reads the raw HID usages
//! and posts a distinct mouse button for each control.
//!
//! The app talks over two pipes. stdin: "enable", "disable", "game 0|1"; end
//! of file means the app has gone, so clean up and exit. stdout:
//! "state <open> <devices>" and "last <text>".

use std::ffi::c_void;
use std::io::{BufRead, Write};
use std::ptr::null;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

type Ref = *const c_void;

#[repr(C)]
#[derive(Clone, Copy)]
struct CGPoint { x: f64, y: f64 }

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    static kCFTypeDictionaryKeyCallBacks: c_void;
    static kCFTypeDictionaryValueCallBacks: c_void;
    static kCFRunLoopCommonModes: Ref;
    fn CFStringCreateWithCString(a: Ref, s: *const u8, enc: u32) -> Ref;
    fn CFNumberCreate(a: Ref, kind: isize, v: *const i32) -> Ref;
    fn CFDictionaryCreate(a: Ref, k: *const Ref, v: *const Ref, n: isize, kc: *const c_void, vc: *const c_void) -> Ref;
    fn CFRunLoopGetCurrent() -> Ref;
    fn CFRunLoopRun();
    fn CFRunLoopAddTimer(rl: Ref, t: Ref, mode: Ref);
    fn CFRunLoopAddSource(rl: Ref, s: Ref, mode: Ref);
    fn CFMachPortCreateRunLoopSource(a: Ref, port: Ref, order: isize) -> Ref;
    fn CFRunLoopTimerCreate(a: Ref, fire: f64, interval: f64, flags: u64, order: isize, cb: extern "C" fn(Ref, *mut c_void), ctx: Ref) -> Ref;
    fn CFRunLoopTimerInvalidate(t: Ref);
    fn CFAbsoluteTimeGetCurrent() -> f64;
    fn CFRelease(r: Ref);
}

#[link(name = "IOKit", kind = "framework")]
extern "C" {
    fn IOHIDManagerCreate(a: Ref, opts: u32) -> Ref;
    fn IOHIDManagerSetDeviceMatching(m: Ref, d: Ref);
    fn IOHIDManagerRegisterDeviceMatchingCallback(m: Ref, cb: extern "C" fn(*mut c_void, i32, Ref, Ref), ctx: *mut c_void);
    fn IOHIDManagerRegisterDeviceRemovalCallback(m: Ref, cb: extern "C" fn(*mut c_void, i32, Ref, Ref), ctx: *mut c_void);
    fn IOHIDManagerRegisterInputValueCallback(m: Ref, cb: extern "C" fn(*mut c_void, i32, Ref, Ref), ctx: *mut c_void);
    fn IOHIDManagerScheduleWithRunLoop(m: Ref, rl: Ref, mode: Ref);
    fn IOHIDManagerOpen(m: Ref, opts: u32) -> i32;
    fn IOHIDValueGetElement(v: Ref) -> Ref;
    fn IOHIDValueGetIntegerValue(v: Ref) -> isize;
    fn IOHIDValueGetTimeStamp(v: Ref) -> u64;
    fn IOHIDElementGetUsagePage(e: Ref) -> u32;
    fn IOHIDElementGetUsage(e: Ref) -> u32;
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGEventCreate(src: Ref) -> Ref;
    fn CGEventGetLocation(e: Ref) -> CGPoint;
    fn CGEventCreateMouseEvent(src: Ref, kind: u32, at: CGPoint, button: u32) -> Ref;
    fn CGEventSetIntegerValueField(e: Ref, field: u32, v: i64);
    fn CGEventGetIntegerValueField(e: Ref, field: u32) -> i64;
    fn CGEventSetFlags(e: Ref, flags: u64);
    fn CGEventGetFlags(e: Ref) -> u64;
    fn CGEventPost(tap: u32, e: Ref);
    fn CGEventSourceFlagsState(state: i32) -> u64;
    fn CGPreflightListenEventAccess() -> bool;
    fn CGPreflightPostEventAccess() -> bool;
    fn CGEventTapCreate(tap: u32, place: u32, options: u32, mask: u64, cb: extern "C" fn(Ref, u32, Ref, *mut c_void) -> Ref, ctx: *mut c_void) -> Ref;
    fn CGEventTapEnable(tap: Ref, on: bool);
}

extern "C" {
    fn mach_absolute_time() -> u64;
    fn mach_timebase_info(info: *mut [u32; 2]) -> i32;
    fn pthread_set_qos_class_self_np(qos: u32, relative_priority: i32) -> i32;
    fn fcntl(fd: i32, cmd: i32, ...) -> i32;
}

/// Milliseconds from the mouse's HID report to now.
fn delay_ms(stamp: u64) -> f64 {
    let mut tb = [0u32; 2];
    unsafe { mach_timebase_info(&mut tb) };
    let ticks = unsafe { mach_absolute_time() }.saturating_sub(stamp);
    ticks as f64 * tb[0] as f64 / tb[1] as f64 / 1e6
}

const OTHER_DOWN: u32 = 25;
const OTHER_UP: u32 = 26;
const BUTTON_NUMBER_FIELD: u32 = 3;
const HID_SYSTEM_STATE: i32 = 1;
// Shift, Control, Option, Command. Caps Lock and Fn are left out on purpose.
const MODIFIERS: u64 = 0x0002_0000 | 0x0004_0000 | 0x0008_0000 | 0x0010_0000;

/// A press older than this is dropped, never posted: in a game a missed
/// press costs less than one that lands late. Normal delivery takes 1-2 ms.
const MAX_LATE_MS: f64 = 50.0;

/// Seconds between dry runs of the press path. Raise it if the wake-ups
/// matter more than a slow first press on a machine short of memory.
const KEEP_WARM_SECS: f64 = 0.25;

/// What the mouse's keys are remapped to: ExSel, which macOS has no key
/// code for. "No key" (usage 0) was the first choice, but with that mapping
/// in place World of Warcraft interrupted channelled spells with no input
/// at all; without it they ran to the end.
const MUTE_DST: u64 = 0x7_0000_00a4;

/// Physical control behind a keyboard usage, as set in the mouse's onboard
/// profile: side 1-12 send 1..0, -, = and the two top buttons send F13, F14.
#[derive(Debug, PartialEq, Clone, Copy)]
enum Control { Side(u32), Top, Hypershift }

const USAGES: [u32; 14] = [0x1e, 0x1f, 0x20, 0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x27, 0x2d, 0x2e, 0x68, 0x69];

fn control(usage: u32) -> Option<Control> {
    match usage {
        0x1e..=0x27 => Some(Control::Side(usage - 0x1e + 1)),
        0x2d => Some(Control::Side(11)),
        0x2e => Some(Control::Side(12)),
        0x68 => Some(Control::Top),
        0x69 => Some(Control::Hypershift),
        _ => None,
    }
}

/// CGEvent button number (0-based). WoW names it BUTTON<n + 1>:
/// side 1-12 -> BUTTON6-17, top -> BUTTON18, Hypershift + side 1-12 -> BUTTON19-30.
fn button(c: Control, hypershift: bool) -> Option<u32> {
    match c {
        Control::Side(i) if hypershift => Some(17 + i),
        Control::Side(i) => Some(4 + i),
        Control::Top => Some(17),
        Control::Hypershift => None,
    }
}

fn slot(c: Control) -> usize {
    match c { Control::Side(i) => i as usize, Control::Top => 13, Control::Hypershift => 0 }
}

struct State {
    enabled: bool,
    open: bool,
    devices: usize,
    hypershift: bool,
    // The button each control pressed, so release matches even if
    // Hypershift is let go first.
    held: [Option<u32>; 14],
    manager: usize,
    timer: usize,
}

// Shared by the run loop (presses) and the stdin thread (commands).
static ST: Mutex<State> = Mutex::new(State {
    enabled: true, open: false, devices: 0, hypershift: false, held: [None; 14], manager: 0, timer: 0,
});

fn st<R>(f: impl FnOnce(&mut State) -> R) -> R { f(&mut ST.lock().unwrap()) }

/// A line for the app. stdout does not block, so a stuck app costs a lost
/// line, never a late press.
fn tell(line: std::fmt::Arguments) {
    let _ = writeln!(std::io::stdout(), "{line}");
}

fn tell_state() {
    let (open, devices) = st(|s| (s.open, s.devices));
    tell(format_args!("state {} {devices}", open as u8));
}

static TRACE: OnceLock<std::fs::File> = OnceLock::new();

/// Opt-in event log (~/Library/Logs/NagaButtons.log) for diagnosing lag or
/// stray presses. Turn on, then restart the app:
/// `defaults write io.github.fol2.nagabuttons Trace -bool true`
fn trace(line: std::fmt::Arguments) {
    if let Some(mut f) = TRACE.get() {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis());
        let _ = writeln!(f, "{t} {line}");
    }
}

fn make_event(kind: u32, b: u32) -> Ref {
    unsafe {
        let here = CGEventCreate(null());
        let at = CGEventGetLocation(here);
        CFRelease(here);
        let e = CGEventCreateMouseEvent(null(), kind, at, b);
        CGEventSetIntegerValueField(e, BUTTON_NUMBER_FIELD, b as i64);
        CGEventSetFlags(e, CGEventSourceFlagsState(HID_SYSTEM_STATE) & MODIFIERS);
        e
    }
}

fn post(kind: u32, b: u32) {
    let e = make_event(kind, b);
    unsafe { CGEventPost(0, e); CFRelease(e); }
}

/// When memory is short, macOS swaps an idle process out, and the first
/// press then waits on swap-in. Building (not posting) an event keeps the
/// press path's pages in use.
// ponytail: referenced pages can still be evicted under extreme pressure,
// and the HID queue's own pages are not touched; mlock the regions if the
// trace still shows "dropped: late" on a first press.
extern "C" fn keep_warm(_: Ref, _: *mut c_void) {
    st(|s| std::hint::black_box(button(Control::Side(1), s.hypershift)));
    let e = make_event(OTHER_UP, 5);
    unsafe { CFRelease(e) };
}

/// Mutes the mouse's own keys (on) or gives them back (off). macOS drops a
/// device's mapping when it disconnects, so this runs on every reconnect.
fn mute_mouse_keys(on: bool) {
    let map: Vec<String> = if on {
        USAGES.iter().map(|u| format!(
            r#"{{"HIDKeyboardModifierMappingSrc":{},"HIDKeyboardModifierMappingDst":{MUTE_DST}}}"#,
            0x7_0000_0000u64 | *u as u64)).collect()
    } else { vec![] };
    // Muting touches the two keyboard interfaces only, never the one that
    // carries movement and clicks. Clearing covers every interface, which
    // also removes what an older version left on the mouse interface.
    let matching = if on { r#"{"VendorID":5426,"ProductID":180,"PrimaryUsagePage":1,"PrimaryUsage":6}"# }
        else { r#"{"VendorID":5426,"ProductID":180}"# };
    let r = std::process::Command::new("/usr/bin/hidutil")
        .args(["property", "--matching", matching,
               "--set", &format!(r#"{{"UserKeyMapping":[{}]}}"#, map.join(","))])
        .stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).status();
    if !matches!(r, Ok(s) if s.success()) { trace(format_args!("hidutil failed: {r:?}")); }
}

/// Brings the mute in line with the state: muted only while enabled, open
/// and connected, so the side buttons are never both muted and unread. The
/// lock keeps two hidutil runs (run loop and stdin thread) in order; it is
/// not ST, so hidutil never holds up a press.
fn sync_mute() {
    static ORDER: Mutex<()> = Mutex::new(());
    let _order = ORDER.lock().unwrap();
    mute_mouse_keys(st(|s| s.enabled && s.open && s.devices > 0));
}

fn release_all(s: &mut State) {
    for b in s.held.iter_mut().filter_map(Option::take) { post(OTHER_UP, b); }
}

extern "C" fn on_value(_: *mut c_void, _: i32, sender: Ref, value: Ref) {
    let (page, usage, down, stamp) = unsafe {
        let e = IOHIDValueGetElement(value);
        (IOHIDElementGetUsagePage(e), IOHIDElementGetUsage(e), IOHIDValueGetIntegerValue(value) != 0, IOHIDValueGetTimeStamp(value))
    };
    if page != 0x07 { return; }
    trace(format_args!("hid dev={sender:p} usage={usage:#x} down={down} delay={:.1}ms", delay_ms(stamp)));
    let Some(c) = control(usage) else { return };
    st(|s| {
        if !s.enabled { return; }
        if c == Control::Hypershift { s.hypershift = down; return; }
        let i = slot(c);
        if down {
            let late = delay_ms(stamp);
            if late > MAX_LATE_MS { trace(format_args!("  dropped: late")); return; }
            if s.held[i].is_some() { trace(format_args!("  ignored: already held")); return; }
            let Some(b) = button(c, s.hypershift) else { return };
            s.held[i] = Some(b);
            post(OTHER_DOWN, b);
            trace(format_args!("  post down BUTTON{}", b + 1));
            match c {
                Control::Side(n) if s.hypershift => tell(format_args!("last Hypershift + side {n} → BUTTON{} ({late:.1} ms)", b + 1)),
                Control::Side(n) => tell(format_args!("last Side {n} → BUTTON{} ({late:.1} ms)", b + 1)),
                _ => tell(format_args!("last Top → BUTTON{} ({late:.1} ms)", b + 1)),
            }
        } else if let Some(b) = s.held[i].take() {
            // A release is posted however late it is: a button must not stay down.
            post(OTHER_UP, b);
            trace(format_args!("  post up BUTTON{}", b + 1));
        } else {
            trace(format_args!("  up without down"));
        }
    });
}

extern "C" fn on_device(_: *mut c_void, _: i32, _: Ref, _: Ref) {
    st(|s| s.devices += 1);
    trace(format_args!("device added"));
    sync_mute();
    trace(format_args!("  mute done"));
    tell_state();
}

extern "C" fn on_removed(_: *mut c_void, _: i32, _: Ref, _: Ref) {
    trace(format_args!("device removed"));
    st(|s| { s.devices = s.devices.saturating_sub(1); s.hypershift = false; release_all(s); });
    tell_state();
}

fn granted() -> bool { unsafe { CGPreflightListenEventAccess() && CGPreflightPostEventAccess() } }

/// Opens only with both permissions: without Input Monitoring, opening can
/// still succeed but no presses arrive, and muting the keys would leave the
/// side buttons dead.
fn try_open() -> bool {
    let m = st(|s| s.manager) as Ref;
    let ok = granted() && unsafe { IOHIDManagerOpen(m, 0) } == 0;
    st(|s| s.open = ok);
    ok
}

/// Retries until the permissions are granted. A grant often needs a fresh
/// process, so once both are granted and opening still fails, exit with 75:
/// the app then starts itself again, once (--no-relaunch stops a loop when
/// opening fails for another reason).
extern "C" fn on_retry(_: Ref, _: *mut c_void) {
    if try_open() {
        let t = st(|s| s.timer) as Ref;
        unsafe { CFRunLoopTimerInvalidate(t) };
        trace(format_args!("opened after permission grant"));
        sync_mute();
    } else if granted() && !std::env::args().any(|a| a == "--no-relaunch") {
        std::process::exit(75);
    }
    tell_state();
}

static GAME_FRONT: AtomicBool = AtomicBool::new(false);
static TAP: AtomicUsize = AtomicUsize::new(0);

/// Trace only: a listen-only log of the clicks and keys the game receives,
/// with the process that posted each one (pid 0 is real hardware). Nothing
/// is logged while another app is in front.
extern "C" fn on_tap(_: Ref, kind: u32, e: Ref, _: *mut c_void) -> Ref {
    if kind > 0x7fff_ffff { // the system switched the tap off (timeout or user input)
        trace(format_args!("tap disabled ({kind:#x}); enabling again"));
        unsafe { CGEventTapEnable(TAP.load(Ordering::Relaxed) as Ref, true) };
    } else if GAME_FRONT.load(Ordering::Relaxed) {
        let f = |field| unsafe { CGEventGetIntegerValueField(e, field) };
        let name = match kind { 1 => "left down", 2 => "left up", 3 => "right down", 4 => "right up", 10 => "key down",
            11 => "key up", 12 => "flags", 25 => "other down", 26 => "other up", _ => "?" };
        trace(format_args!("tap {name} button={} keycode={} repeat={} flags={:#x} srcpid={}",
            f(3) + 1, f(9), f(8), unsafe { CGEventGetFlags(e) }, f(41)));
    }
    e
}

/// Commands from the app. End of file means the app has gone: let go of
/// every button, give the mouse its keys back and exit.
fn read_commands() {
    for line in std::io::stdin().lock().lines().map_while(Result::ok) {
        match line.as_str() {
            "enable" | "disable" => {
                let on = line == "enable";
                st(|s| {
                    s.enabled = on;
                    s.hypershift = false;
                    release_all(s);
                });
                trace(format_args!("enabled={on}"));
                sync_mute();
            }
            "game 1" | "game 0" => {
                let front = line == "game 1";
                if GAME_FRONT.swap(front, Ordering::Relaxed) != front { trace(format_args!("game front={front}")); }
            }
            _ => {}
        }
    }
    // Disabled first, so no press is posted while hidutil runs and then
    // left down by the exit.
    st(|s| { s.enabled = false; release_all(s); });
    sync_mute();
    trace(format_args!("app gone; exit"));
    std::process::exit(0);
}

fn cfstr(s: &str) -> Ref {
    let c = format!("{s}\0");
    unsafe { CFStringCreateWithCString(null(), c.as_ptr(), 0x0800_0100) }
}

fn main() {
    if std::env::args().any(|a| a == "--trace") {
        let path = format!("{}/Library/Logs/NagaButtons.log", std::env::var("HOME").unwrap_or_default());
        if let Ok(f) = std::fs::OpenOptions::new().create(true).append(true).open(path) { let _ = TRACE.set(f); }
        trace(format_args!("start {} daemon", env!("CARGO_PKG_VERSION")));
    }
    unsafe {
        pthread_set_qos_class_self_np(0x21, 0); // user-interactive
        fcntl(1, 4, 4); // F_SETFL, O_NONBLOCK: see tell()
    }
    std::thread::spawn(read_commands);
    // A crash or kill can leave the keys muted; give them back until we can
    // really read the mouse.
    mute_mouse_keys(false);
    unsafe {
        // The Naga's keyboard interfaces only; the mouse interface keeps
        // movement, clicks and the wheel. Other keyboards never match.
        let keys = [cfstr("VendorID"), cfstr("ProductID"), cfstr("PrimaryUsagePage"), cfstr("PrimaryUsage")];
        let nums: Vec<Ref> = [0x1532, 0x00b4, 1, 6].iter().map(|v: &i32| CFNumberCreate(null(), 3, v)).collect();
        let matching = CFDictionaryCreate(null(), keys.as_ptr(), nums.as_ptr(), 4,
            &kCFTypeDictionaryKeyCallBacks, &kCFTypeDictionaryValueCallBacks);
        let m = IOHIDManagerCreate(null(), 0);
        IOHIDManagerSetDeviceMatching(m, matching);
        IOHIDManagerRegisterDeviceMatchingCallback(m, on_device, std::ptr::null_mut());
        IOHIDManagerRegisterDeviceRemovalCallback(m, on_removed, std::ptr::null_mut());
        IOHIDManagerRegisterInputValueCallback(m, on_value, std::ptr::null_mut());
        IOHIDManagerScheduleWithRunLoop(m, CFRunLoopGetCurrent(), kCFRunLoopCommonModes);
        st(|s| s.manager = m as usize);
        if !try_open() {
            trace(format_args!("waiting for Input Monitoring and Accessibility"));
            let t = CFRunLoopTimerCreate(null(), CFAbsoluteTimeGetCurrent() + 3.0, 3.0, 0, 0, on_retry, null());
            CFRunLoopAddTimer(CFRunLoopGetCurrent(), t, kCFRunLoopCommonModes);
            st(|s| s.timer = t as usize);
        }
        let warm = CFRunLoopTimerCreate(null(), CFAbsoluteTimeGetCurrent() + KEEP_WARM_SECS, KEEP_WARM_SECS, 0, 0, keep_warm, null());
        CFRunLoopAddTimer(CFRunLoopGetCurrent(), warm, kCFRunLoopCommonModes);
        if TRACE.get().is_some() {
            // Session tap, appended, listen-only: mouse buttons, keys, modifier changes.
            let mask = [1u64, 2, 3, 4, 10, 11, 12, 25, 26].iter().fold(0, |m, t| m | 1 << t);
            let tap = CGEventTapCreate(1, 1, 1, mask, on_tap, std::ptr::null_mut());
            if tap.is_null() { trace(format_args!("input log: no event tap")); } else {
                TAP.store(tap as usize, Ordering::Relaxed);
                CFRunLoopAddSource(CFRunLoopGetCurrent(), CFMachPortCreateRunLoopSource(null(), tap, 0), kCFRunLoopCommonModes);
                trace(format_args!("input log on"));
            }
        }
        tell_state();
        CFRunLoopRun();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mapping() {
        assert_eq!(control(0x1e), Some(Control::Side(1)));
        assert_eq!(control(0x27), Some(Control::Side(10)));
        assert_eq!(control(0x2e), Some(Control::Side(12)));
        assert_eq!(control(0x04), None);
        assert!(USAGES.iter().all(|u| control(*u).is_some()));
        // 25 distinct buttons, all inside the 32 macOS supports.
        let mut all: Vec<u32> = (1..=12).flat_map(|i| [button(Control::Side(i), false), button(Control::Side(i), true)])
            .chain([button(Control::Top, false)]).map(Option::unwrap).collect();
        all.sort();
        all.dedup();
        assert_eq!(all.len(), 25);
        assert!(all[0] == 5 && *all.last().unwrap() == 29); // BUTTON6..BUTTON30
        assert_eq!(button(Control::Top, true), Some(17));
        assert_eq!(button(Control::Hypershift, false), None);
        // The mute target is a keyboard usage no control sends.
        assert!(control((MUTE_DST & 0xffff) as u32).is_none() && MUTE_DST >> 32 == 7);
    }
}
