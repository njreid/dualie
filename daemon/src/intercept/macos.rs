#![allow(non_upper_case_globals, non_camel_case_types)]

/// intercept/macos.rs — macOS local keyboard remapping.
///
/// # Architecture
///
/// Uses IOHIDManager to exclusively seize physical keyboards (equivalent to
/// Linux `EVIOCGRAB`), processes events through `remap.rs`, and posts remapped
/// reports to the Karabiner VirtualHIDDevice daemon.
///
/// ```text
/// Physical key press
///   → IOKit HID driver
///   → IOHIDManager (kIOHIDOptionsTypeSeizeDevice)  ← our grab
///       ↓ raw HID usage codes — BEFORE IOHIDSystem modifier remapping
///   → remap.rs (process_key)
///       ↓ KvhdReport (67-byte pqrs format)
///   → Karabiner-VirtualHIDDevice-Daemon (Unix datagram socket)
///       ↓ virtual keyboard (DriverKit extension)
///   → IOHIDSystem → apps
/// ```
///
/// Because we grab before IOHIDSystem, Caps Lock (HID 0x39) is always
/// physically identifiable regardless of any System Preferences modifier
/// remapping the user has configured.
///
/// # Requirements
///
/// - Karabiner-DriverKit-VirtualHIDDevice must be installed and its daemon
///   running (does NOT require the Karabiner-Elements app).
/// - The binary must have Accessibility permission (System Preferences →
///   Privacy & Security → Accessibility) for the exclusive device seize.
///
/// # Thread model
///
/// `run()` blocks on `CFRunLoopRun()` — call from a dedicated OS thread.

use std::cell::RefCell;
use std::collections::HashSet;
use std::ffi::c_void;
use std::sync::atomic::Ordering;

use anyhow::{bail, Result};
use tokio::sync::watch;
use tracing::{info, warn};

use crate::config::DualieConfig;
use crate::peer::SerialClient;
use super::ActiveOutput;
use super::keycodes::hid_modifier_bit;
use super::macos_kvhd::{build_report, KvhdHandle};
use super::remap::{CompiledOutputConfig, LayerState, process_key, VALUE_DOWN, VALUE_UP};

// ── Raw FFI declarations ───────────────────────────────────────────────────────

type IOReturn          = i32;
type IOHIDManagerRef   = *mut c_void;
type IOHIDDeviceRef    = *mut c_void;
type IOHIDValueRef     = *mut c_void;
type IOHIDElementRef   = *mut c_void;
type CFRunLoopRef      = *mut c_void;
type CFStringRef       = *const c_void;
type CFAllocatorRef    = *mut c_void;
type CFDictionaryRef   = *mut c_void;

const kIOReturnSuccess: IOReturn = 0;

// IOHIDManager options
const kIOHIDOptionsTypeNone:         u32 = 0x0;
const kIOHIDOptionsTypeSeizeDevice:  u32 = 0x1;

// HID usage page for keyboard
const kHIDPage_GenericDesktop: u32 = 0x01;
const kHIDPage_KeyboardOrKeypad: u32 = 0x07;
const kHIDUsage_GD_Keyboard: u32 = 0x06;

type IOHIDDeviceCallback = unsafe extern "C" fn(
    context: *mut c_void,
    result:  IOReturn,
    sender:  *mut c_void,
    device:  IOHIDDeviceRef,
);

type IOHIDValueCallback = unsafe extern "C" fn(
    context: *mut c_void,
    result:  IOReturn,
    sender:  *mut c_void,
    value:   IOHIDValueRef,
);

#[link(name = "IOKit", kind = "framework")]
extern "C" {
    fn IOHIDManagerCreate(
        allocator: CFAllocatorRef,
        options:   u32,
    ) -> IOHIDManagerRef;

    fn IOHIDManagerSetDeviceMatching(
        manager:  IOHIDManagerRef,
        matching: CFDictionaryRef,
    );

    fn IOHIDManagerOpen(manager: IOHIDManagerRef, options: u32) -> IOReturn;

    fn IOHIDManagerRegisterDeviceMatchingCallback(
        manager:   IOHIDManagerRef,
        callback:  IOHIDDeviceCallback,
        context:   *mut c_void,
    );

    fn IOHIDManagerRegisterInputValueCallback(
        manager:  IOHIDManagerRef,
        callback: IOHIDValueCallback,
        context:  *mut c_void,
    );

    fn IOHIDManagerScheduleWithRunLoop(
        manager:  IOHIDManagerRef,
        run_loop: CFRunLoopRef,
        mode:     CFStringRef,
    );

    fn IOHIDDeviceOpen(device: IOHIDDeviceRef, options: u32) -> IOReturn;
    fn IOHIDDeviceGetProperty(device: IOHIDDeviceRef, key: CFStringRef) -> *mut c_void;

    fn IOHIDValueGetIntegerValue(value: IOHIDValueRef) -> i64;
    fn IOHIDValueGetElement(value: IOHIDValueRef) -> IOHIDElementRef;
    fn IOHIDElementGetUsage(element: IOHIDElementRef) -> u32;
    fn IOHIDElementGetUsagePage(element: IOHIDElementRef) -> u32;
    fn IOHIDElementGetDevice(element: IOHIDElementRef) -> IOHIDDeviceRef;
}

const kIOHIDVendorIDKey_str:  &[u8] = b"VendorID\0";
const kIOHIDProductIDKey_str: &[u8] = b"ProductID\0";

// kIOHIDDeviceUsagePageKey / kIOHIDDeviceUsageKey are #define macros in
// IOKit headers, not exported symbols.  Build CFString refs at runtime.
const kIOHIDDeviceUsagePageKey_str: &[u8] = b"DeviceUsagePage\0";
const kIOHIDDeviceUsageKey_str:     &[u8] = b"DeviceUsage\0";

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFRunLoopGetCurrent() -> CFRunLoopRef;
    fn CFRunLoopRun();
    fn CFStringCreateWithCString(
        alloc:    CFAllocatorRef,
        c_str:    *const u8,
        encoding: u32,  // kCFStringEncodingUTF8 = 0x08000100
    ) -> CFStringRef;

    fn CFDictionaryCreateMutable(
        allocator:     CFAllocatorRef,
        capacity:      isize,
        key_callbacks: *const c_void,
        value_callbacks: *const c_void,
    ) -> CFDictionaryRef;
    fn CFDictionaryAddValue(dict: CFDictionaryRef, key: *const c_void, value: *const c_void);
    fn CFNumberCreate(
        allocator: CFAllocatorRef,
        the_type:  i32,         // kCFNumberSInt32Type = 3
        value_ptr: *const c_void,
    ) -> *mut c_void;
    fn CFNumberGetValue(
        number:   *const c_void,
        the_type: i32,          // kCFNumberSInt32Type = 3
        value_ptr: *mut c_void,
    ) -> bool;
    fn CFRelease(cf: *mut c_void);

    static kCFAllocatorDefault:              CFAllocatorRef;
    static kCFRunLoopDefaultMode:            CFStringRef;
    static kCFTypeDictionaryKeyCallBacks:    c_void;
    static kCFTypeDictionaryValueCallBacks:  c_void;

    static kCFBooleanTrue: *const c_void;
}

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    // Check without prompting.
    fn AXIsProcessTrusted() -> bool;
    // Check and, when options contains kAXTrustedCheckOptionPrompt=true, add
    // the app to System Settings → Accessibility and show the auth dialog.
    fn AXIsProcessTrustedWithOptions(options: CFDictionaryRef) -> bool;
}

// ── Thread-local state ────────────────────────────────────────────────────────

struct MacosState {
    compiled:        CompiledOutputConfig,
    last_output_idx: u8,
    cfg_snapshot:    DualieConfig,
    cfg_rx:          watch::Receiver<DualieConfig>,
    layer:           LayerState,
    virtual_pressed: HashSet<u8>,  // HID codes currently held in the virtual device
    kvhd:            KvhdHandle,
    serial:          SerialClient,
    active_output:   ActiveOutput,
}

thread_local! {
    static MACOS_STATE: RefCell<Option<MacosState>> = RefCell::new(None);
}

// ── IOHIDManager callbacks ────────────────────────────────────────────────────

/// Read a u32 IOHIDDevice property by key string (e.g. "VendorID").
unsafe fn hid_device_u32(device: IOHIDDeviceRef, key_str: &[u8]) -> Option<u32> {
    const kCFStringEncodingUTF8: u32 = 0x0800_0100;
    const kCFNumberSInt32Type:   i32 = 3;
    let cf_key = CFStringCreateWithCString(kCFAllocatorDefault, key_str.as_ptr(), kCFStringEncodingUTF8);
    let cf_val = IOHIDDeviceGetProperty(device, cf_key as CFStringRef);
    CFRelease(cf_key as *mut c_void);
    if cf_val.is_null() { return None; }
    let mut v: i32 = 0;
    CFNumberGetValue(cf_val, kCFNumberSInt32Type, &mut v as *mut _ as *mut c_void);
    Some(v as u32)
}

/// Called when a new keyboard device is found — seize it exclusively.
/// Skip the Karabiner virtual keyboard (vendor=0x16c0, product=0x27db) to
/// avoid a feedback loop where our own injected events get re-intercepted.
unsafe extern "C" fn device_added(
    _context: *mut c_void,
    _result:  IOReturn,
    _sender:  *mut c_void,
    device:   IOHIDDeviceRef,
) {
    let vendor  = hid_device_u32(device, kIOHIDVendorIDKey_str).unwrap_or(0);
    let product = hid_device_u32(device, kIOHIDProductIDKey_str).unwrap_or(0);
    if vendor == 0x16c0 && product == 0x27db {
        info!("macOS: skipping Karabiner virtual keyboard (vendor={vendor:#06x} product={product:#06x})");
        return;
    }
    let ret = IOHIDDeviceOpen(device, kIOHIDOptionsTypeSeizeDevice);
    if ret == kIOReturnSuccess {
        info!("macOS: keyboard device seized (vendor={vendor:#06x} product={product:#06x})");
    } else {
        warn!("macOS: failed to seize keyboard device: {ret:#x} (need Accessibility permission?)");
    }
}

/// Called for every key event on a seized keyboard device.
unsafe extern "C" fn value_available(
    _context: *mut c_void,
    _result:  IOReturn,
    _sender:  *mut c_void,
    value:    IOHIDValueRef,
) {
    let element    = IOHIDValueGetElement(value);
    let usage_page = IOHIDElementGetUsagePage(element);

    // Only handle keyboard usage page (0x07).
    if usage_page != kHIDPage_KeyboardOrKeypad {
        return;
    }

    // Ignore events from the Karabiner virtual keyboard — we inject into it,
    // so its events would create a feedback loop.
    let device  = IOHIDElementGetDevice(element);
    let vendor  = hid_device_u32(device, kIOHIDVendorIDKey_str).unwrap_or(0);
    let product = hid_device_u32(device, kIOHIDProductIDKey_str).unwrap_or(0);
    if vendor == 0x16c0 && product == 0x27db {
        return;
    }

    let usage     = IOHIDElementGetUsage(element) as u8;
    let int_value = IOHIDValueGetIntegerValue(value); // 0 = up, 1 = down

    // IOHIDManager does not deliver key-repeat events — the OS handles repeat
    // at a higher layer (IOHIDSystem) after we seize the device.  Every non-zero
    // value here is a genuine key-down; we never get VALUE_REPEAT from this path.
    let ev_value  = if int_value != 0 { VALUE_DOWN } else { VALUE_UP };

    let modifier_bit = hid_modifier_bit(usage);
    let hid = if modifier_bit != 0 { 0 } else { usage };

    MACOS_STATE.with(|cell| {
        let mut borrow = cell.borrow_mut();
        let Some(state) = borrow.as_mut() else { return };

        // Recompile if config hot-reloaded or active output changed.
        let output_now = state.active_output.load(Ordering::Relaxed);
        let cfg_changed = state.cfg_rx.has_changed().unwrap_or(false);
        if cfg_changed {
            state.cfg_snapshot = state.cfg_rx.borrow_and_update().clone();
        }
        if cfg_changed || output_now != state.last_output_idx {
            state.last_output_idx = output_now;
            state.compiled = super::recompile(&state.cfg_snapshot, &state.active_output);
        }

        let result = process_key(hid, modifier_bit, ev_value, &state.compiled, &mut state.layer);

        super::dispatch_result(&result, &state.cfg_snapshot, &state.active_output, &state.serial);

        // ── Inject synthetic events via KVHD ──────────────────────────────────

        if result.events.is_empty() {
            return;
        }

        for syn in &result.events {
            // hid==0 means modifier-only report; don't track 0 in the key set.
            if syn.hid != 0 {
                match syn.value {
                    VALUE_DOWN => { state.virtual_pressed.insert(syn.hid); }
                    VALUE_UP   => { state.virtual_pressed.remove(&syn.hid); }
                    _          => {}
                }
            }
            let report = build_report(syn.modifiers, &state.virtual_pressed);
            if let Err(e) = state.kvhd.post_report(&report) {
                warn!("macOS: KVHD post_report failed: {e}");
            }
        }
    });
}

// ── Accessibility permission ──────────────────────────────────────────────────

/// Check for Accessibility permission; prompt via the system dialog only if
/// not yet granted.  The prompt surfaces Dualie in System Settings →
/// Accessibility so the user doesn't have to find the binary manually.
fn request_accessibility_if_needed() {
    // Fast path: already trusted — don't disturb the user.
    if unsafe { AXIsProcessTrusted() } {
        info!("macOS: Accessibility permission already granted");
        return;
    }

    // Not trusted yet — prompt.  This registers Dualie in System Settings and
    // shows the authorisation dialog.
    const kCFStringEncodingUTF8: u32 = 0x0800_0100;
    let granted = unsafe {
        let key = CFStringCreateWithCString(
            kCFAllocatorDefault,
            b"AXTrustedCheckOptionPrompt\0".as_ptr(),
            kCFStringEncodingUTF8,
        );
        let opts = CFDictionaryCreateMutable(
            kCFAllocatorDefault, 1,
            &kCFTypeDictionaryKeyCallBacks as *const _ as *const c_void,
            &kCFTypeDictionaryValueCallBacks as *const _ as *const c_void,
        );
        CFDictionaryAddValue(opts, key as *const c_void, kCFBooleanTrue);
        CFRelease(key as *mut c_void);
        let result = AXIsProcessTrustedWithOptions(opts);
        CFRelease(opts as *mut c_void);
        result
    };
    if granted {
        info!("macOS: Accessibility permission granted");
    } else {
        warn!("macOS: Accessibility permission not yet granted — approve in System Settings → Privacy & Security → Accessibility");
    }
}

// ── Public entry point ────────────────────────────────────────────────────────

/// Run the macOS keyboard interception loop.  Blocks until an error occurs.
pub fn run(
    cfg_rx:        watch::Receiver<DualieConfig>,
    serial:        SerialClient,
    active_output: ActiveOutput,
) -> Result<()> {
    // Register with the Accessibility system and prompt the user if not yet
    // granted.  This surfaces Dualie in System Settings → Privacy & Security →
    // Accessibility without requiring the user to find the binary manually.
    request_accessibility_if_needed();

    let kvhd = KvhdHandle::open()?;
    info!("macOS: Karabiner VirtualHIDKeyboard connected");

    let cfg_snapshot = cfg_rx.borrow().clone();
    let output_idx   = active_output.load(Ordering::Relaxed);
    let compiled     = super::recompile(&cfg_snapshot, &active_output);

    MACOS_STATE.with(|cell| {
        *cell.borrow_mut() = Some(MacosState {
            compiled,
            last_output_idx: output_idx,
            cfg_snapshot,
            cfg_rx,
            layer:           LayerState::default(),
            virtual_pressed: HashSet::new(),
            kvhd,
            serial,
            active_output,
        });
    });

    // Build a matching dictionary for keyboards (Usage Page 0x01, Usage 0x06).
    let _manager = unsafe {
        let mgr = IOHIDManagerCreate(kCFAllocatorDefault, kIOHIDOptionsTypeNone);
        if mgr.is_null() { bail!("IOHIDManagerCreate returned NULL"); }

        let matching = CFDictionaryCreateMutable(
            kCFAllocatorDefault, 2,
            &kCFTypeDictionaryKeyCallBacks as *const _ as *const c_void,
            &kCFTypeDictionaryValueCallBacks as *const _ as *const c_void,
        );
        let page_num  = kHIDPage_GenericDesktop as i32;
        let usage_num = kHIDUsage_GD_Keyboard as i32;
        let cf_page  = CFNumberCreate(kCFAllocatorDefault, 3, &page_num  as *const _ as *const c_void);
        let cf_usage = CFNumberCreate(kCFAllocatorDefault, 3, &usage_num as *const _ as *const c_void);
        const kCFStringEncodingUTF8: u32 = 0x0800_0100;
        let key_page  = CFStringCreateWithCString(kCFAllocatorDefault, kIOHIDDeviceUsagePageKey_str.as_ptr(), kCFStringEncodingUTF8);
        let key_usage = CFStringCreateWithCString(kCFAllocatorDefault, kIOHIDDeviceUsageKey_str.as_ptr(),     kCFStringEncodingUTF8);
        CFDictionaryAddValue(matching, key_page  as *const c_void, cf_page);
        CFDictionaryAddValue(matching, key_usage as *const c_void, cf_usage);
        CFRelease(key_page as *mut c_void);
        CFRelease(key_usage as *mut c_void);
        CFRelease(cf_page);
        CFRelease(cf_usage);

        IOHIDManagerSetDeviceMatching(mgr, matching);
        CFRelease(matching);

        IOHIDManagerRegisterDeviceMatchingCallback(mgr, device_added, std::ptr::null_mut());
        IOHIDManagerRegisterInputValueCallback(mgr, value_available, std::ptr::null_mut());

        IOHIDManagerScheduleWithRunLoop(mgr, CFRunLoopGetCurrent(), kCFRunLoopDefaultMode);

        let ret = IOHIDManagerOpen(mgr, kIOHIDOptionsTypeSeizeDevice);
        if ret != kIOReturnSuccess {
            bail!("IOHIDManagerOpen failed: {ret:#x}");
        }
        mgr
    };

    info!("macOS: IOHIDManager open — watching for keyboards");
    unsafe { CFRunLoopRun(); }

    bail!("CFRunLoopRun returned unexpectedly");
}
