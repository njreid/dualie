/// launch_macos.rs — Native macOS app focus and window cycling.
///
/// Entirely thread-safe: uses AX APIs, CoreFoundation, and proc_pidpath.
/// No AppKit, no subprocesses (except `open -b` to launch a not-yet-running app).
///
/// # Focus / cycle flow
///
/// 1. Get the frontmost app's PID via AXUIElement (system-wide, thread-safe).
/// 2. Resolve that PID to a bundle ID via proc_pidpath + CFBundle.
/// 3. If it matches the target: cycle windows via AXRaise on window[1].
/// 4. If not: spawn `open -b <id>` to focus or launch.

use std::ffi::{c_void, CStr, CString};
use std::path::Path;
use tracing::warn;

// ── libc ──────────────────────────────────────────────────────────────────────

use libc::{pid_t, proc_pidpath, PROC_PIDPATHINFO_MAXSIZE};

// ── CoreFoundation types ─────────────────────────────────────────────────────

type CFAllocatorRef = *mut c_void;
type CFStringRef    = *const c_void;
type CFURLRef       = *mut c_void;
type CFBundleRef    = *mut c_void;
type CFTypeRef      = *mut c_void;
type CFArrayRef     = *mut c_void;
type CFIndex        = isize;
type Boolean        = u8;

const kCFStringEncodingUTF8: u32 = 0x0800_0100;

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    static kCFAllocatorDefault: CFAllocatorRef;

    fn CFStringCreateWithCString(
        alloc: CFAllocatorRef,
        c_str: *const i8,
        encoding: u32,
    ) -> CFStringRef;
    fn CFStringGetCString(
        s: CFStringRef,
        buf: *mut i8,
        buf_size: CFIndex,
        encoding: u32,
    ) -> Boolean;
    fn CFRelease(cf: *mut c_void);

    fn CFURLCreateFromFileSystemRepresentation(
        alloc: CFAllocatorRef,
        bytes: *const u8,
        length: CFIndex,
        is_directory: Boolean,
    ) -> CFURLRef;

    fn CFBundleCreate(alloc: CFAllocatorRef, bundle_url: CFURLRef) -> CFBundleRef;
    fn CFBundleGetValueForInfoDictionaryKey(bundle: CFBundleRef, key: CFStringRef) -> CFTypeRef;

    fn CFArrayGetCount(arr: CFArrayRef) -> CFIndex;
    fn CFArrayGetValueAtIndex(arr: CFArrayRef, idx: CFIndex) -> *const c_void;
}

// ── Accessibility types ───────────────────────────────────────────────────────

type AXUIElementRef = *mut c_void;
type AXError        = i32;
const kAXErrorSuccess: AXError = 0;

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXUIElementCreateSystemWide() -> AXUIElementRef;
    fn AXUIElementCreateApplication(pid: pid_t) -> AXUIElementRef;
    fn AXUIElementCopyAttributeValue(
        element: AXUIElementRef,
        attribute: CFStringRef,
        value: *mut *mut c_void,
    ) -> AXError;
    fn AXUIElementGetPid(element: AXUIElementRef, pid: *mut pid_t) -> AXError;
    fn AXUIElementPerformAction(element: AXUIElementRef, action: CFStringRef) -> AXError;
}

// ── Helpers ───────────────────────────────────────────────────────────────────

unsafe fn cf_string(s: &str) -> CFStringRef {
    let c = CString::new(s).unwrap_or_default();
    CFStringCreateWithCString(kCFAllocatorDefault, c.as_ptr(), kCFStringEncodingUTF8)
}

unsafe fn cf_string_to_rust(s: CFStringRef) -> Option<String> {
    if s.is_null() { return None; }
    let mut buf = vec![0i8; 512];
    let ok = CFStringGetCString(s, buf.as_mut_ptr(), buf.len() as CFIndex, kCFStringEncodingUTF8);
    if ok == 0 { return None; }
    Some(CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned())
}

// ── Bundle ID from PID ────────────────────────────────────────────────────────

/// Resolve a PID to its bundle identifier using proc_pidpath + CFBundle.
/// Returns None if the process isn't a bundled app or the lookup fails.
fn bundle_id_for_pid(pid: pid_t) -> Option<String> {
    // Get the executable path.
    let mut buf = vec![0u8; PROC_PIDPATHINFO_MAXSIZE as usize];
    let ret = unsafe { proc_pidpath(pid, buf.as_mut_ptr() as *mut c_void, buf.len() as u32) };
    if ret <= 0 { return None; }
    let exe_path = unsafe { CStr::from_ptr(buf.as_ptr() as *const i8) }
        .to_string_lossy()
        .into_owned();

    // Walk up the path to find the enclosing .app bundle.
    let bundle_path = {
        let mut p: &Path = Path::new(&exe_path);
        loop {
            if p.extension().and_then(|e| e.to_str()) == Some("app") {
                break Some(p);
            }
            match p.parent() {
                Some(parent) if parent != p => p = parent,
                _ => break None,
            }
        }
    }?;

    // Read CFBundleIdentifier from the bundle's Info.plist via CFBundle.
    let path_bytes = bundle_path.as_os_str().as_encoded_bytes();
    unsafe {
        let url = CFURLCreateFromFileSystemRepresentation(
            kCFAllocatorDefault,
            path_bytes.as_ptr(),
            path_bytes.len() as CFIndex,
            1, // is_directory = true
        );
        if url.is_null() { return None; }

        let bundle = CFBundleCreate(kCFAllocatorDefault, url);
        CFRelease(url);
        if bundle.is_null() { return None; }

        let key = cf_string("CFBundleIdentifier");
        let val = CFBundleGetValueForInfoDictionaryKey(bundle, key);
        CFRelease(key as *mut c_void);
        CFRelease(bundle);

        if val.is_null() { return None; }
        cf_string_to_rust(val as CFStringRef)
    }
}

// ── Frontmost PID via AX ──────────────────────────────────────────────────────

/// Returns the PID of the currently frontmost application.
fn frontmost_pid() -> Option<pid_t> {
    unsafe {
        let system = AXUIElementCreateSystemWide();
        if system.is_null() { return None; }

        let attr = cf_string("AXFocusedApplication");
        let mut focused: *mut c_void = std::ptr::null_mut();
        let err = AXUIElementCopyAttributeValue(system, attr, &mut focused);
        CFRelease(attr as *mut c_void);
        CFRelease(system);

        if err != kAXErrorSuccess || focused.is_null() { return None; }

        let mut pid: pid_t = 0;
        let err = AXUIElementGetPid(focused as AXUIElementRef, &mut pid);
        CFRelease(focused);

        if err != kAXErrorSuccess { None } else { Some(pid) }
    }
}

// ── Window cycling ────────────────────────────────────────────────────────────

/// Raise window[1] of the given PID, cycling it to the front.
/// No-op if the app has fewer than two windows.
fn cycle_window(pid: pid_t, label: &str) {
    unsafe {
        let ax_app = AXUIElementCreateApplication(pid);
        if ax_app.is_null() { return; }

        let attr = cf_string("AXWindows");
        let mut windows_val: *mut c_void = std::ptr::null_mut();
        let err = AXUIElementCopyAttributeValue(ax_app, attr, &mut windows_val);
        CFRelease(attr as *mut c_void);

        if err != kAXErrorSuccess || windows_val.is_null() {
            CFRelease(ax_app);
            return;
        }

        let windows = windows_val as CFArrayRef;
        if CFArrayGetCount(windows) > 1 {
            let win1 = CFArrayGetValueAtIndex(windows, 1) as AXUIElementRef;
            let action = cf_string("AXRaise");
            let err = AXUIElementPerformAction(win1, action);
            CFRelease(action as *mut c_void);
            if err != kAXErrorSuccess {
                warn!(label, pid, err, "AXRaise failed");
            }
        }

        CFRelease(windows_val);
        CFRelease(ax_app);
    }
}

// ── Public entry point ────────────────────────────────────────────────────────

/// Focus or launch an app by bundle ID. Cycles windows if already frontmost.
/// Safe to call from any thread.
pub fn focus_or_cycle(app_id: &str, label: &str) {
    // Check if the target app is currently frontmost.
    let already_front = frontmost_pid()
        .and_then(|pid| bundle_id_for_pid(pid).map(|id| (pid, id)))
        .map(|(pid, id)| {
            if id == app_id {
                // Already front — cycle windows.
                cycle_window(pid, label);
                true
            } else {
                false
            }
        })
        .unwrap_or(false);

    if !already_front {
        // Focus if running, or launch if not. `open -b` handles both.
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
}
