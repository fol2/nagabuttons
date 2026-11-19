//! NagaButtons: turns the Razer Naga V2 HyperSpeed side buttons into real
//! mouse buttons, with a menu bar item to pause or quit.
//! Licensed under the MIT licence; see LICENSE.
//!
//! hidutil maps the mouse's own keys to "no key" (seizing a keyboard needs
//! root), while this process still reads the raw HID usages and posts a
//! distinct mouse button for each control. Keyboard modifiers stay free, so
//! Shift + side 1 and Shift + Hypershift + side 1 are new combos.
//! Event-driven on the main run loop: no polling, no event tap.

use std::cell::{OnceCell, RefCell};
use std::ffi::c_void;
use std::ptr::null;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject, Sel};
use objc2::{define_class, msg_send, sel, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate, NSControlStateValueOff,
    NSControlStateValueOn, NSImage, NSMenu, NSMenuItem, NSRunningApplication, NSStatusBar,
    NSStatusItem, NSVariableStatusItemLength, NSWorkspace,
};
use objc2_foundation::{ns_string, MainThreadMarker, NSNotification, NSObject, NSObjectProtocol, NSString, NSURL};

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
    fn CFRunLoopGetMain() -> Ref;
    fn CFRunLoopAddTimer(rl: Ref, t: Ref, mode: Ref);
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
    fn IOHIDElementGetUsagePage(e: Ref) -> u32;
    fn IOHIDElementGetUsage(e: Ref) -> u32;
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGEventCreate(src: Ref) -> Ref;
    fn CGEventGetLocation(e: Ref) -> CGPoint;
    fn CGEventCreateMouseEvent(src: Ref, kind: u32, at: CGPoint, button: u32) -> Ref;
    fn CGEventSetIntegerValueField(e: Ref, field: u32, v: i64);
    fn CGEventSetFlags(e: Ref, flags: u64);
    fn CGEventPost(tap: u32, e: Ref);
    fn CGEventSourceFlagsState(state: i32) -> u64;
    fn CGRequestListenEventAccess() -> bool;
    fn CGRequestPostEventAccess() -> bool;
    fn CGPreflightListenEventAccess() -> bool;
    fn CGPreflightPostEventAccess() -> bool;
}

const OTHER_DOWN: u32 = 25;
const OTHER_UP: u32 = 26;
const BUTTON_NUMBER_FIELD: u32 = 3;
const HID_SYSTEM_STATE: i32 = 1;
// Shift, Control, Option, Command. Caps Lock and Fn are left out on purpose.
const MODIFIERS: u64 = 0x0002_0000 | 0x0004_0000 | 0x0008_0000 | 0x0010_0000;
const BUNDLE_ID: &str = "io.github.fol2.nagabuttons";

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
    last: String,
    manager: usize,
    timer: usize,
}

struct Ui {
    status: Retained<NSStatusItem>,
    info: Retained<NSMenuItem>,
    last: Retained<NSMenuItem>,
    toggle: Retained<NSMenuItem>,
}

thread_local! {
    static ST: RefCell<State> = RefCell::new(State {
        enabled: true, open: false, devices: 0, hypershift: false, held: [None; 14],
        last: "Last press: none yet".into(), manager: 0, timer: 0,
    });
    static UI: OnceCell<Ui> = const { OnceCell::new() };
}

fn post(kind: u32, b: u32) {
    unsafe {
        let here = CGEventCreate(null());
        let at = CGEventGetLocation(here);
        CFRelease(here);
        let e = CGEventCreateMouseEvent(null(), kind, at, b);
        CGEventSetIntegerValueField(e, BUTTON_NUMBER_FIELD, b as i64);
        CGEventSetFlags(e, CGEventSourceFlagsState(HID_SYSTEM_STATE) & MODIFIERS);
        CGEventPost(0, e);
        CFRelease(e);
    }
}

/// Mutes the mouse's own keys (on) or gives them back (off). macOS drops a
/// device's mapping when it disconnects, so this runs on every reconnect.
fn mute_mouse_keys(on: bool) {
    let map: Vec<String> = if on {
        USAGES.iter().map(|u| format!(
            r#"{{"HIDKeyboardModifierMappingSrc":{},"HIDKeyboardModifierMappingDst":{}}}"#,
            0x7_0000_0000u64 | *u as u64, 0x7_0000_0000u64)).collect()
    } else { vec![] };
    let r = std::process::Command::new("/usr/bin/hidutil")
        .args(["property", "--matching", r#"{"VendorID":5426,"ProductID":180}"#,
               "--set", &format!(r#"{{"UserKeyMapping":[{}]}}"#, map.join(","))])
        .stdout(std::process::Stdio::null()).status();
    if !matches!(r, Ok(s) if s.success()) { eprintln!("naga-buttons: hidutil failed: {r:?}"); }
}

fn release_all(s: &mut State) {
    for b in s.held.iter_mut().filter_map(Option::take) { post(OTHER_UP, b); }
}

fn refresh() {
    let (text, on, active, last) = ST.with_borrow(|s| {
        let text = if !s.open { "Needs Input Monitoring and Accessibility" }
            else if s.devices == 0 { "Mouse not connected" }
            else if s.enabled { "Active" } else { "Paused: side buttons type 1 to =" };
        (text, s.enabled, s.open && s.devices > 0 && s.enabled, s.last.clone())
    });
    UI.with(|ui| if let Some(ui) = ui.get() {
        ui.info.setTitle(&NSString::from_str(text));
        ui.last.setTitle(&NSString::from_str(&last));
        ui.toggle.setState(if on { NSControlStateValueOn } else { NSControlStateValueOff });
        let mtm = MainThreadMarker::new().unwrap();
        if let Some(b) = ui.status.button(mtm) { b.setAppearsDisabled(!active); }
    });
}

extern "C" fn on_value(_: *mut c_void, _: i32, _: Ref, value: Ref) {
    let (page, usage, down) = unsafe {
        let e = IOHIDValueGetElement(value);
        (IOHIDElementGetUsagePage(e), IOHIDElementGetUsage(e), IOHIDValueGetIntegerValue(value) != 0)
    };
    if page != 0x07 { return; }
    let Some(c) = control(usage) else { return };
    let changed = ST.with_borrow_mut(|s| {
        if !s.enabled { return false; }
        if c == Control::Hypershift { s.hypershift = down; return false; }
        let i = slot(c);
        if down {
            if s.held[i].is_some() { return false; }
            let Some(b) = button(c, s.hypershift) else { return false };
            s.held[i] = Some(b);
            post(OTHER_DOWN, b);
            let name = match c { Control::Side(n) if s.hypershift => format!("Hypershift + side {n}"),
                Control::Side(n) => format!("Side {n}"), _ => "Top".into() };
            s.last = format!("Last press: {name} → BUTTON{}", b + 1);
            true
        } else {
            if let Some(b) = s.held[i].take() { post(OTHER_UP, b); }
            false
        }
    });
    if changed { refresh(); }
}

extern "C" fn on_device(_: *mut c_void, _: i32, _: Ref, _: Ref) {
    let enabled = ST.with_borrow_mut(|s| { s.devices += 1; s.enabled });
    if enabled { mute_mouse_keys(true); }
    refresh();
}

extern "C" fn on_removed(_: *mut c_void, _: i32, _: Ref, _: Ref) {
    ST.with_borrow_mut(|s| { s.devices = s.devices.saturating_sub(1); s.hypershift = false; release_all(s); });
    refresh();
}

fn try_open() -> bool {
    let m = ST.with_borrow(|s| s.manager) as Ref;
    let ok = unsafe { IOHIDManagerOpen(m, 0) } == 0;
    ST.with_borrow_mut(|s| s.open = ok);
    ok
}

/// Retries until the permissions are granted. A grant often needs a fresh
/// process, so once both are granted and opening still fails, start a new
/// copy of the app and quit this one.
extern "C" fn on_retry(_: Ref, _: *mut c_void) {
    if try_open() {
        let t = ST.with_borrow(|s| s.timer) as Ref;
        unsafe { CFRunLoopTimerInvalidate(t) };
        eprintln!("naga-buttons: opened after permission grant");
    } else if unsafe { CGPreflightListenEventAccess() && CGPreflightPostEventAccess() } {
        let bundle = objc2_foundation::NSBundle::mainBundle().bundlePath().to_string();
        let _ = std::process::Command::new("/bin/sh").args(["-c", "sleep 1; open \"$0\"", &bundle]).spawn();
        std::process::exit(75);
    }
    refresh();
}

fn cfstr(s: &str) -> Ref {
    let c = format!("{s}\0");
    unsafe { CFStringCreateWithCString(null(), c.as_ptr(), 0x0800_0100) }
}

fn start_hid() {
    unsafe {
        CGRequestListenEventAccess();
        CGRequestPostEventAccess();
        // The Naga's keyboard interfaces only; the mouse interface keeps
        // movement, clicks and the wheel. The G913 and other keyboards never match.
        let keys = [cfstr("VendorID"), cfstr("ProductID"), cfstr("PrimaryUsagePage"), cfstr("PrimaryUsage")];
        let nums: Vec<Ref> = [0x1532, 0x00b4, 1, 6].iter().map(|v: &i32| CFNumberCreate(null(), 3, v)).collect();
        let matching = CFDictionaryCreate(null(), keys.as_ptr(), nums.as_ptr(), 4,
            &kCFTypeDictionaryKeyCallBacks, &kCFTypeDictionaryValueCallBacks);
        let m = IOHIDManagerCreate(null(), 0);
        IOHIDManagerSetDeviceMatching(m, matching);
        IOHIDManagerRegisterDeviceMatchingCallback(m, on_device, std::ptr::null_mut());
        IOHIDManagerRegisterDeviceRemovalCallback(m, on_removed, std::ptr::null_mut());
        IOHIDManagerRegisterInputValueCallback(m, on_value, std::ptr::null_mut());
        // Common modes, so presses still arrive while the menu is open.
        IOHIDManagerScheduleWithRunLoop(m, CFRunLoopGetMain(), kCFRunLoopCommonModes);
        ST.with_borrow_mut(|s| s.manager = m as usize);
        if !try_open() {
            eprintln!("naga-buttons: waiting for Input Monitoring and Accessibility");
            let t = CFRunLoopTimerCreate(null(), CFAbsoluteTimeGetCurrent() + 3.0, 3.0, 0, 0, on_retry, null());
            CFRunLoopAddTimer(CFRunLoopGetMain(), t, kCFRunLoopCommonModes);
            ST.with_borrow_mut(|s| s.timer = t as usize);
        }
    }
}

fn menu_item(mtm: MainThreadMarker, title: &str, action: Option<Sel>, key: &str, target: Option<&AnyObject>) -> Retained<NSMenuItem> {
    let item = unsafe { NSMenuItem::initWithTitle_action_keyEquivalent(
        NSMenuItem::alloc(mtm), &NSString::from_str(title), action, &NSString::from_str(key)) };
    if let Some(t) = target { unsafe { item.setTarget(Some(t)) }; }
    if action.is_none() { item.setEnabled(false); }
    item
}

define_class!(
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    struct Delegate;

    impl Delegate {
        #[unsafe(method(toggle:))]
        fn toggle(&self, _sender: Option<&AnyObject>) {
            let (on, connected) = ST.with_borrow_mut(|s| {
                s.enabled = !s.enabled;
                s.hypershift = false;
                release_all(s);
                (s.enabled, s.devices > 0)
            });
            if connected { mute_mouse_keys(on); }
            refresh();
        }

        #[unsafe(method(openPrivacy:))]
        fn open_privacy(&self, _sender: Option<&AnyObject>) {
            if let Some(url) = NSURL::URLWithString(ns_string!("x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent")) {
                NSWorkspace::sharedWorkspace().openURL(&url);
            }
        }
    }

    unsafe impl NSObjectProtocol for Delegate {}

    unsafe impl NSApplicationDelegate for Delegate {
        #[unsafe(method(applicationDidFinishLaunching:))]
        fn did_finish_launching(&self, _n: &NSNotification) {
            let mtm = self.mtm();
            let me: &AnyObject = self;
            let status = NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
            if let Some(b) = status.button(mtm) {
                if let Some(img) = NSImage::imageWithSystemSymbolName_accessibilityDescription(ns_string!("computermouse.fill"), Some(ns_string!("NagaButtons"))) {
                    img.setTemplate(true);
                    b.setImage(Some(&img));
                }
                b.setToolTip(Some(ns_string!("NagaButtons")));
            }
            let menu = NSMenu::new(mtm);
            menu.setAutoenablesItems(false);
            let info = menu_item(mtm, "Starting…", None, "", None);
            let last = menu_item(mtm, "Last press: none yet", None, "", None);
            let toggle = menu_item(mtm, "Enabled", Some(sel!(toggle:)), "e", Some(me));
            menu.addItem(&menu_item(mtm, "NagaButtons", None, "", None));
            menu.addItem(&info);
            menu.addItem(&last);
            menu.addItem(&NSMenuItem::separatorItem(mtm));
            menu.addItem(&toggle);
            menu.addItem(&NSMenuItem::separatorItem(mtm));
            for line in ["Side 1–12 → BUTTON6–17", "Top front → BUTTON18", "Hold top rear (Hypershift) + side 1–12 → BUTTON19–30"] {
                menu.addItem(&menu_item(mtm, line, None, "", None));
            }
            menu.addItem(&NSMenuItem::separatorItem(mtm));
            menu.addItem(&menu_item(mtm, "Privacy Settings…", Some(sel!(openPrivacy:)), "", Some(me)));
            menu.addItem(&NSMenuItem::separatorItem(mtm));
            menu.addItem(&menu_item(mtm, "Quit NagaButtons", Some(sel!(terminate:)), "q", None));
            status.setMenu(Some(&menu));
            // Keeps the place you ⌘-drag the icon to across restarts.
            status.setAutosaveName(Some(ns_string!("NagaButtons")));
            UI.with(|u| { let _ = u.set(Ui { status, info, last, toggle }); });
            start_hid();
            refresh();
        }

        #[unsafe(method(applicationWillTerminate:))]
        fn will_terminate(&self, _n: &NSNotification) {
            ST.with_borrow_mut(release_all);
            mute_mouse_keys(false); // the side buttons type 1 to = again
        }
    }
);

impl Delegate {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        unsafe { msg_send![super(this), init] }
    }
}

fn main() {
    let mtm = MainThreadMarker::new().unwrap();
    // Opening the app again while it runs does nothing.
    if NSRunningApplication::runningApplicationsWithBundleIdentifier(&NSString::from_str(BUNDLE_ID)).count() > 1 {
        return;
    }
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    let delegate = Delegate::new(mtm);
    app.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    app.run();
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
    }
}
