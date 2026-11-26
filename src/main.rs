//! NagaButtons: turns the Razer Naga V2 HyperSpeed side buttons into real
//! mouse buttons. This is the menu bar app: it shows the status, pauses or
//! quits, and keeps the daemon (nagabuttonsd, next to this binary) running.
//! The daemon does the work; nothing here is on the path of a press.
//! Licensed under the MIT licence; see LICENSE.

use std::cell::OnceCell;
use std::ffi::c_void;
use std::io::{BufRead, Write};
use std::process::{ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject, Sel};
use objc2::{define_class, msg_send, sel, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate, NSControlStateValueOff,
    NSControlStateValueOn, NSImage, NSMenu, NSMenuDelegate, NSMenuItem, NSRunningApplication, NSStatusBar,
    NSStatusItem, NSVariableStatusItemLength, NSWorkspace,
};
use objc2_foundation::{ns_string, MainThreadMarker, NSNotification, NSObject, NSObjectProtocol, NSString, NSURL, NSUserDefaults};
use objc2_service_management::{SMAppService, SMAppServiceStatus};

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGRequestListenEventAccess() -> bool;
    fn CGRequestPostEventAccess() -> bool;
}

extern "C" {
    static _dispatch_main_q: c_void;
    fn dispatch_async_f(queue: *const c_void, ctx: *mut c_void, work: extern "C" fn(*mut c_void));
}

const BUNDLE_ID: &str = "io.github.fol2.nagabuttons";
const GAME_BUNDLE_ID: &str = "com.blizzard.worldofwarcraft";

/// What the menu shows: the daemon's last report, and the Enabled switch.
struct View { enabled: bool, open: bool, devices: usize, last: String }

static VIEW: Mutex<View> = Mutex::new(View { enabled: true, open: false, devices: 0, last: String::new() });
static DAEMON_IN: Mutex<Option<ChildStdin>> = Mutex::new(None);
static QUITTING: AtomicBool = AtomicBool::new(false);
static DAEMON_GONE: AtomicBool = AtomicBool::new(false);

struct Ui {
    status: Retained<NSStatusItem>,
    info: Retained<NSMenuItem>,
    last: Retained<NSMenuItem>,
    toggle: Retained<NSMenuItem>,
    login: Retained<NSMenuItem>,
}

thread_local! { static UI: OnceCell<Ui> = const { OnceCell::new() }; }

fn send(line: &str) {
    if let Some(w) = DAEMON_IN.lock().unwrap().as_mut() { let _ = writeln!(w, "{line}"); }
}

/// Starts the daemon and follows it on a thread of its own. As a child of
/// the app it uses the app's privacy grants. If it dies it is started
/// again; exit code 75 means a fresh grant needs a fresh app.
fn run_daemon(trace: bool) {
    let relaunched = std::env::args().any(|a| a == "--relaunched");
    std::thread::spawn(move || loop {
        if QUITTING.load(Ordering::Relaxed) { DAEMON_GONE.store(true, Ordering::Relaxed); return; }
        let exe = std::env::current_exe().map(|p| p.with_file_name("nagabuttonsd")).unwrap_or_default();
        let mut cmd = Command::new(exe);
        if trace { cmd.arg("--trace"); }
        if relaunched { cmd.arg("--no-relaunch"); }
        let code = match cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).spawn() {
            Ok(mut child) => {
                *DAEMON_IN.lock().unwrap() = child.stdin.take();
                if !VIEW.lock().unwrap().enabled { send("disable"); }
                let out = std::io::BufReader::new(child.stdout.take().unwrap());
                for line in out.lines().map_while(Result::ok) {
                    let mut v = VIEW.lock().unwrap();
                    if let Some(text) = line.strip_prefix("last ") {
                        v.last = format!("Last press: {text}"); // shown when the menu opens
                    } else if let Some(state) = line.strip_prefix("state ") {
                        let mut it = state.split(' ');
                        v.open = it.next() == Some("1");
                        v.devices = it.next().and_then(|n| n.parse().ok()).unwrap_or(0);
                        drop(v);
                        refresh_soon();
                    }
                }
                *DAEMON_IN.lock().unwrap() = None;
                child.wait().ok().and_then(|s| s.code())
            }
            Err(e) => { eprintln!("nagabuttons: cannot start the daemon: {e}"); None }
        };
        if QUITTING.load(Ordering::Relaxed) { DAEMON_GONE.store(true, Ordering::Relaxed); return; }
        if code == Some(75) {
            let bundle = objc2_foundation::NSBundle::mainBundle().bundlePath().to_string();
            let _ = Command::new("/bin/sh").args(["-c", "sleep 1; open \"$0\" --args --relaunched", &bundle]).spawn();
            std::process::exit(75);
        }
        { let mut v = VIEW.lock().unwrap(); v.open = false; v.devices = 0; }
        refresh_soon();
        std::thread::sleep(std::time::Duration::from_secs(1));
    });
}

/// Trace only: tells the daemon when the game is the front app, so its
/// input log records nothing typed into any other app.
fn watch_game() {
    std::thread::spawn(|| {
        let mut was = false;
        for tick in 0u32.. {
            let front = objc2::rc::autoreleasepool(|_| NSWorkspace::sharedWorkspace().frontmostApplication()
                .and_then(|a| a.bundleIdentifier()).is_some_and(|b| b.to_string() == GAME_BUNDLE_ID));
            // On a change, and every 5 s in case the daemon was started again.
            if front != was || tick % 20 == 0 { was = front; send(if front { "game 1" } else { "game 0" }); }
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
    });
}

/// For other threads: the menu belongs to the main thread.
fn refresh_soon() {
    extern "C" fn run(_: *mut c_void) { refresh(); }
    unsafe { dispatch_async_f(&_dispatch_main_q, std::ptr::null_mut(), run) };
}

fn refresh() {
    let (text, on, active, last) = {
        let v = VIEW.lock().unwrap();
        let text = if !v.open { "Needs Input Monitoring and Accessibility" }
            else if v.devices == 0 { "Mouse not connected" }
            else if v.enabled { "Active" } else { "Paused: side buttons type 1 to =" };
        let last = if v.last.is_empty() { "Last press: none yet".into() } else { v.last.clone() };
        (text, v.enabled, v.open && v.devices > 0 && v.enabled, last)
    };
    UI.with(|ui| if let Some(ui) = ui.get() {
        ui.info.setTitle(&NSString::from_str(text));
        ui.last.setTitle(&NSString::from_str(&last));
        ui.toggle.setState(if on { NSControlStateValueOn } else { NSControlStateValueOff });
        let login = unsafe { SMAppService::mainAppService().status() };
        ui.login.setState(if login == SMAppServiceStatus::Enabled { NSControlStateValueOn } else { NSControlStateValueOff });
        ui.login.setTitle(if login == SMAppServiceStatus::RequiresApproval { ns_string!("Start at Login (approve in Settings)") } else { ns_string!("Start at Login") });
        let mtm = MainThreadMarker::new().unwrap();
        if let Some(b) = ui.status.button(mtm) { b.setAppearsDisabled(!active); }
    });
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
            let on = { let mut v = VIEW.lock().unwrap(); v.enabled = !v.enabled; v.enabled };
            send(if on { "enable" } else { "disable" });
            refresh();
        }

        #[unsafe(method(toggleLogin:))]
        fn toggle_login(&self, _sender: Option<&AnyObject>) {
            let svc = unsafe { SMAppService::mainAppService() };
            let r = unsafe { match svc.status() {
                SMAppServiceStatus::Enabled => svc.unregisterAndReturnError(),
                SMAppServiceStatus::RequiresApproval => { SMAppService::openSystemSettingsLoginItems(); Ok(()) }
                _ => svc.registerAndReturnError(),
            } };
            if let Err(e) = r { eprintln!("nagabuttons: login item: {e:?}"); }
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

    unsafe impl NSMenuDelegate for Delegate {
        #[unsafe(method(menuNeedsUpdate:))]
        fn menu_needs_update(&self, _menu: &NSMenu) { refresh(); }
    }

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
            let login = menu_item(mtm, "Start at Login", Some(sel!(toggleLogin:)), "", Some(me));
            menu.addItem(&menu_item(mtm, "NagaButtons", None, "", None));
            menu.addItem(&info);
            menu.addItem(&last);
            menu.addItem(&NSMenuItem::separatorItem(mtm));
            menu.addItem(&toggle);
            menu.addItem(&login);
            menu.addItem(&NSMenuItem::separatorItem(mtm));
            for line in ["Side 1–12 → BUTTON6–17", "Top front → BUTTON18", "Hold top rear (Hypershift) + side 1–12 → BUTTON19–30"] {
                menu.addItem(&menu_item(mtm, line, None, "", None));
            }
            menu.addItem(&NSMenuItem::separatorItem(mtm));
            menu.addItem(&menu_item(mtm, "Privacy Settings…", Some(sel!(openPrivacy:)), "", Some(me)));
            menu.addItem(&NSMenuItem::separatorItem(mtm));
            menu.addItem(&menu_item(mtm, "Quit NagaButtons", Some(sel!(terminate:)), "q", None));
            menu.setDelegate(Some(ProtocolObject::from_ref(self)));
            status.setMenu(Some(&menu));
            // Keeps the place you ⌘-drag the icon to across restarts.
            status.setAutosaveName(Some(ns_string!("NagaButtons")));
            UI.with(|u| { let _ = u.set(Ui { status, info, last, toggle, login }); });
            // Start at login by default, once; the menu item turns it off.
            let defaults = NSUserDefaults::standardUserDefaults();
            if !defaults.boolForKey(ns_string!("LoginItemOffered")) {
                defaults.setBool_forKey(true, ns_string!("LoginItemOffered"));
                if let Err(e) = unsafe { SMAppService::mainAppService().registerAndReturnError() } {
                    eprintln!("nagabuttons: login item: {e:?}");
                }
            }
            // The prompts name this app; the daemon is covered by its grants.
            unsafe {
                CGRequestListenEventAccess();
                CGRequestPostEventAccess();
            }
            // Opt-in event log: defaults write io.github.fol2.nagabuttons Trace -bool true
            let trace = defaults.boolForKey(ns_string!("Trace"));
            run_daemon(trace);
            if trace { watch_game(); }
            refresh();
        }

        #[unsafe(method(applicationWillTerminate:))]
        fn will_terminate(&self, _n: &NSNotification) {
            // Closing its stdin tells the daemon to let go of every button,
            // give the side buttons back their keys and exit. Wait for that.
            QUITTING.store(true, Ordering::Relaxed);
            *DAEMON_IN.lock().unwrap() = None;
            for _ in 0..40 {
                if DAEMON_GONE.load(Ordering::Relaxed) { break; }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
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
