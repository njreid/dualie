/// launch_macos.rs — Native macOS app focus and window cycling.
///
/// Uses the ObjC runtime (NSRunningApplication, NSWorkspace) and the
/// Accessibility API (AXUIElement) directly — no subprocesses.
///
/// # Focus / launch flow
///
/// 1. Query `NSRunningApplication.runningApplicationsWithBundleIdentifier:`
///    to find all PIDs for the target bundle.
/// 2. If none found: fall back to `open -b <id>` to launch.
/// 3. If the app is already frontmost: cycle to the next window via AX.
/// 4. Otherwise: activate via `NSRunningApplication.activateWithOptions:`.

use std::ffi::{c_void, CStr, CString};
use tracing::warn;

// ── ObjC runtime types ────────────────────────────────────────────────────────

type Id     = *mut c_void;
type Class  = *mut c_void;
type Sel    = *mut c_void;
type BOOL   = i8;
type NSUInteger = usize;

const YES: BOOL = 1;
const NO:  BOOL = 0;

#[link(name = "objc", kind = "dylib")]
extern "C" {
    fn objc_getClass(name: *const i8) -> Class;
    fn sel_registerName(name: *const i8) -> Sel;
    fn objc_msgSend(receiver: Id, op: Sel, ...) -> Id;
}

// ── CoreFoundation / AX types ────────────────────────────────────────────────

type AXUIElementRef = *mut c_void;
type AXError        = i32;
type CFArrayRef     = *mut c_void;
type CFStringRef    = *const c_void;
type CFIndex        = isize;
type CFAllocatorRef = *mut c_void;
type pid_t          = i32;

const kAXErrorSuccess: AXError = 0;

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXUIElementCreateApplication(pid: pid_t) -> AXUIElementRef;
    fn AXUIElementCopyAttributeValue(
        element: AXUIElementRef,
        attribute: CFStringRef,
        value: *mut *mut c_void,
    ) -> AXError;
    fn AXUIElementPerformAction(element: AXUIElementRef, action: CFStringRef) -> AXError;
    fn CFRelease(cf: *mut c_void);
    fn CFArrayGetCount(arr: CFArrayRef) -> CFIndex;
    fn CFArrayGetValueAtIndex(arr: CFArrayRef, idx: CFIndex) -> *const c_void;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    static kCFAllocatorDefault: CFAllocatorRef;
    fn CFStringCreateWithCStringNoCopy(
        alloc: CFAllocatorRef,
        c_str: *const i8,
        encoding: u32,
        contents_deallocator: CFAllocatorRef,
    ) -> CFStringRef;
    fn CFStringGetCString(
        the_string: CFStringRef,
        buffer: *mut i8,
        buffer_size: CFIndex,
        encoding: u32,
    ) -> BOOL;
}

const kCFStringEncodingUTF8: u32 = 0x0800_0100;
// kCFAllocatorNull — CF does not free the C string we pass in.
// We use a null pointer to mean kCFAllocatorNull (documented as 0).
const CF_ALLOCATOR_NULL: CFAllocatorRef = std::ptr::null_mut();

// NSApplicationActivateIgnoringOtherApps
const NS_APPLICATION_ACTIVATE_IGNORING_OTHER_APPS: NSUInteger = 1 << 1;

// ── Helper: CFString from &str ───────────────────────────────────────────────

/// Wrap a static C string as a CFStringRef without allocation.
/// Caller must ensure the CString lives long enough.
unsafe fn cf_str(s: &CStr) -> CFStringRef {
    CFStringCreateWithCStringNoCopy(
        CF_ALLOCATOR_NULL,
        s.as_ptr(),
        kCFStringEncodingUTF8,
        CF_ALLOCATOR_NULL, // kCFAllocatorNull — don't free the buffer
    )
}

// ── ObjC helpers ─────────────────────────────────────────────────────────────

macro_rules! sel {
    ($name:expr) => {{
        let c = CString::new($name).unwrap();
        unsafe { sel_registerName(c.as_ptr()) }
    }};
}

macro_rules! cls {
    ($name:expr) => {{
        let c = CString::new($name).unwrap();
        unsafe { objc_getClass(c.as_ptr()) }
    }};
}

/// `[NSString stringWithUTF8String: s]` — no-alloc wrapper lives only as long as caller.
unsafe fn ns_string(s: &str) -> Id {
    let c = CString::new(s).unwrap_or_default();
    objc_msgSend(
        cls!("NSString") as Id,
        sel!("stringWithUTF8String:"),
        c.as_ptr() as *const i8,
    )
}

/// Extract a Rust String from an NSString.
unsafe fn rust_string(ns: Id) -> Option<String> {
    if ns.is_null() { return None; }
    let utf8: *const i8 = objc_msgSend(ns, sel!("UTF8String")) as *const i8;
    if utf8.is_null() { return None; }
    Some(CStr::from_ptr(utf8).to_string_lossy().into_owned())
}

// ── Public entry point ────────────────────────────────────────────────────────

/// Focus or launch an app by bundle ID.
/// If the app is already frontmost, cycle to its next window instead.
pub fn focus_or_cycle(app_id: &str, label: &str) {
    unsafe { focus_or_cycle_inner(app_id, label) }
}

unsafe fn focus_or_cycle_inner(app_id: &str, label: &str) {
    // ── Find running instances ────────────────────────────────────────────────
    let ns_bundle_id = ns_string(app_id);
    let running_apps: Id = objc_msgSend(
        cls!("NSRunningApplication") as Id,
        sel!("runningApplicationsWithBundleIdentifier:"),
        ns_bundle_id,
    );

    let count: NSUInteger =
        objc_msgSend(running_apps, sel!("count")) as usize;

    if count == 0 {
        // Not running — launch via open(1).
        launch_via_open(app_id, label);
        return;
    }

    // ── Check frontmost app ───────────────────────────────────────────────────
    let workspace: Id = objc_msgSend(
        cls!("NSWorkspace") as Id,
        sel!("sharedWorkspace"),
    );
    let frontmost: Id = objc_msgSend(workspace, sel!("frontmostApplication"));
    let front_bundle: Id = objc_msgSend(frontmost, sel!("bundleIdentifier"));
    let front_id = rust_string(front_bundle).unwrap_or_default();

    let already_front = front_id == app_id;

    // ── Get PID of the first running instance ─────────────────────────────────
    let app_instance: Id = objc_msgSend(running_apps, sel!("firstObject"));
    let pid: pid_t = objc_msgSend(app_instance, sel!("processIdentifier")) as pid_t;

    if already_front {
        // Cycle to next window via AX.
        cycle_window(pid, label);
    } else {
        // Activate the app.
        objc_msgSend(
            app_instance,
            sel!("activateWithOptions:"),
            NS_APPLICATION_ACTIVATE_IGNORING_OTHER_APPS,
        );
    }
}

/// Raise the second window of the process (cycling the front window to back).
/// No-op if the app has zero or one windows.
unsafe fn cycle_window(pid: pid_t, label: &str) {
    let ax_app = AXUIElementCreateApplication(pid);
    if ax_app.is_null() { return; }

    // kAXWindowsAttribute
    let attr_cstr = CString::new("AXWindows").unwrap();
    let attr_cf = cf_str(&attr_cstr);
    let mut windows_val: *mut c_void = std::ptr::null_mut();
    let err = AXUIElementCopyAttributeValue(ax_app, attr_cf, &mut windows_val);
    CFRelease(attr_cf as *mut c_void);

    if err != kAXErrorSuccess || windows_val.is_null() {
        CFRelease(ax_app);
        return;
    }

    let windows = windows_val as CFArrayRef;
    let n = CFArrayGetCount(windows);

    if n > 1 {
        // Raise window[1] — it becomes front, window[0] moves to back.
        let win1 = CFArrayGetValueAtIndex(windows, 1) as AXUIElementRef;
        let action_cstr = CString::new("AXRaise").unwrap();
        let action_cf = cf_str(&action_cstr);
        let err = AXUIElementPerformAction(win1 as AXUIElementRef, action_cf);
        CFRelease(action_cf as *mut c_void);
        if err != kAXErrorSuccess {
            warn!(label, pid, err, "AXRaise failed");
        }
    }

    CFRelease(windows_val);
    CFRelease(ax_app);
}

fn launch_via_open(app_id: &str, label: &str) {
    if let Err(e) = std::process::Command::new("open")
        .args(["-b", app_id])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        warn!(label, app_id, "open -b: {e}");
    }
}
