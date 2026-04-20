#![allow(non_upper_case_globals, non_camel_case_types)]

/// intercept/macos.rs — macOS keyboard remapping via IOHIDManager + CGEventPost.
///
/// # Architecture
///
/// ```text
/// Physical key press
///   → IOKit HID driver
///   → IOHIDManager (kIOHIDOptionsTypeSeizeDevice)  ← our exclusive grab
///       ↓ raw HID usage codes
///   → remap.rs (process_key)
///       ↓ SyntheticKey events
///   → CGEventPost(kCGSessionEventTap, ...)
///       ↓ virtual key event
///   → IOHIDSystem → apps
/// ```
///
/// We seize physical keyboards so raw events never reach the OS.
/// CGEventPost re-injects remapped events at the HID event tap level,
/// which requires Accessibility permission.
///
/// Caps Lock is fully suppressed as a physical key — it acts only as
/// a layer-shift key and never toggles the system caps-lock state.

use std::cell::RefCell;
use std::ffi::c_void;
use std::sync::atomic::Ordering;
use std::time::Instant;

use anyhow::{bail, Result};
use tokio::sync::watch;
use tracing::{info, warn};

use crate::config::DualieConfig;
use crate::peer::SerialClient;
use super::ActiveOutput;
use super::keycodes::hid_modifier_bit;
use super::remap::{CompiledOutputConfig, LayerState, process_key, VALUE_DOWN, VALUE_UP};

// ── Raw FFI ────────────────────────────────────────────────────────────────────

type IOReturn        = i32;
type IOHIDManagerRef = *mut c_void;
type IOHIDDeviceRef  = *mut c_void;
type IOHIDValueRef   = *mut c_void;
type IOHIDElementRef = *mut c_void;
type CFRunLoopRef    = *mut c_void;
type CFStringRef     = *const c_void;
type CFAllocatorRef  = *mut c_void;
type CFDictionaryRef = *mut c_void;
type CGEventRef      = *mut c_void;
type CGEventSourceRef = *mut c_void;
type CGKeyCode       = u16;
type CGEventFlags    = u64;
type CGEventTapLocation = u32;

const kIOReturnSuccess: IOReturn = 0;
const kIOHIDOptionsTypeNone:        u32 = 0x0;
const kIOHIDOptionsTypeSeizeDevice: u32 = 0x1;
const kHIDPage_KeyboardOrKeypad:    u32 = 0x07;

// kCGSessionEventTap injects into the active GUI session and works with
// Accessibility permission regardless of whether the process is root.
// kCGSessionEventTap (0) requires the process to own the window server session,
// which breaks under `sudo` in a terminal.
const kCGSessionEventTap: CGEventTapLocation = 1;

// CGEventFlags modifier bits
const kCGEventFlagMaskShift:    CGEventFlags = 0x0002_0000;
const kCGEventFlagMaskControl:  CGEventFlags = 0x0004_0000;
const kCGEventFlagMaskAlternate: CGEventFlags = 0x0008_0000;
const kCGEventFlagMaskCommand:  CGEventFlags = 0x0010_0000;
const kCGEventFlagMaskAlphaShift: CGEventFlags = 0x0001_0000; // caps lock

type IOHIDDeviceCallback = unsafe extern "C" fn(*mut c_void, IOReturn, *mut c_void, IOHIDDeviceRef);
type IOHIDValueCallback  = unsafe extern "C" fn(*mut c_void, IOReturn, *mut c_void, IOHIDValueRef);

#[link(name = "IOKit", kind = "framework")]
extern "C" {
    fn IOHIDManagerCreate(allocator: CFAllocatorRef, options: u32) -> IOHIDManagerRef;
    fn IOHIDManagerSetDeviceMatching(manager: IOHIDManagerRef, matching: CFDictionaryRef);
    fn IOHIDManagerOpen(manager: IOHIDManagerRef, options: u32) -> IOReturn;
    fn IOHIDManagerRegisterDeviceMatchingCallback(manager: IOHIDManagerRef, callback: IOHIDDeviceCallback, context: *mut c_void);
    fn IOHIDManagerRegisterInputValueCallback(manager: IOHIDManagerRef, callback: IOHIDValueCallback, context: *mut c_void);
    fn IOHIDManagerScheduleWithRunLoop(manager: IOHIDManagerRef, run_loop: CFRunLoopRef, mode: CFStringRef);
    fn IOHIDDeviceOpen(device: IOHIDDeviceRef, options: u32) -> IOReturn;
    fn IOHIDDeviceGetProperty(device: IOHIDDeviceRef, key: CFStringRef) -> *mut c_void;
    fn IOHIDValueGetIntegerValue(value: IOHIDValueRef) -> i64;
    fn IOHIDValueGetElement(value: IOHIDValueRef) -> IOHIDElementRef;
    fn IOHIDElementGetUsage(element: IOHIDElementRef) -> u32;
    fn IOHIDElementGetUsagePage(element: IOHIDElementRef) -> u32;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFRunLoopGetCurrent() -> CFRunLoopRef;
    fn CFRunLoopRun();
    fn CFStringCreateWithCString(alloc: CFAllocatorRef, c_str: *const u8, encoding: u32) -> CFStringRef;
    fn CFDictionaryCreateMutable(allocator: CFAllocatorRef, capacity: isize, key_cbs: *const c_void, val_cbs: *const c_void) -> CFDictionaryRef;
    fn CFDictionaryAddValue(dict: CFDictionaryRef, key: *const c_void, value: *const c_void);
    fn CFNumberCreate(allocator: CFAllocatorRef, the_type: i32, value_ptr: *const c_void) -> *mut c_void;
    fn CFNumberGetValue(number: *const c_void, the_type: i32, value_ptr: *mut c_void) -> bool;
    fn CFRelease(cf: *mut c_void);
    static kCFAllocatorDefault:             CFAllocatorRef;
    static kCFRunLoopDefaultMode:           CFStringRef;
    static kCFTypeDictionaryKeyCallBacks:   c_void;
    static kCFTypeDictionaryValueCallBacks: c_void;
    static kCFBooleanTrue: *const c_void;
}

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrusted() -> bool;
    fn AXIsProcessTrustedWithOptions(options: CFDictionaryRef) -> bool;
    fn CGEventCreateKeyboardEvent(source: CGEventSourceRef, keycode: CGKeyCode, key_down: bool) -> CGEventRef;
    fn CGEventSetFlags(event: CGEventRef, flags: CGEventFlags);
    fn CGEventPost(tap: CGEventTapLocation, event: CGEventRef);
}

const kIOHIDVendorIDKey_str:        &[u8] = b"VendorID\0";
const kIOHIDProductIDKey_str:       &[u8] = b"ProductID\0";
const kIOHIDDeviceUsagePageKey_str: &[u8] = b"DeviceUsagePage\0";
const kIOHIDDeviceUsageKey_str:     &[u8] = b"DeviceUsage\0";
const kCFStringEncodingUTF8:        u32   = 0x0800_0100;
const kCFNumberSInt32Type:          i32   = 3;
const kHIDPage_GenericDesktop:      u32   = 0x01;
const kHIDUsage_GD_Keyboard:        u32   = 0x06;

// ── Escape hatch state ────────────────────────────────────────────────────────

/// Tracks Ctrl+Shift+Esc presses for the hard-exit escape hatch.
struct EscapeHatch {
    count:    u8,
    last_at:  Option<Instant>,
}

impl EscapeHatch {
    fn new() -> Self { Self { count: 0, last_at: None } }

    /// Call on every Ctrl+Shift+Esc key-down. Returns true when triggered (3×).
    fn register(&mut self) -> bool {
        let now = Instant::now();
        let stale = self.last_at
            .map(|t| now.duration_since(t).as_secs() >= 2)
            .unwrap_or(true);
        if stale { self.count = 0; }
        self.count += 1;
        self.last_at = Some(now);
        if self.count >= 3 {
            return true;
        }
        false
    }
}

// ── Thread-local state ────────────────────────────────────────────────────────

struct MacosState {
    compiled:        CompiledOutputConfig,
    last_output_idx: u8,
    cfg_snapshot:    DualieConfig,
    cfg_rx:          watch::Receiver<DualieConfig>,
    layer:           LayerState,
    active_output:   ActiveOutput,
    serial:          SerialClient,
    escape:          EscapeHatch,
}

thread_local! {
    static MACOS_STATE: RefCell<Option<MacosState>> = RefCell::new(None);
}

// ── HID → CGKeyCode table ─────────────────────────────────────────────────────

/// Map a USB HID keycode (usage page 0x07) to a macOS CGKeyCode (virtual key).
/// Returns None for keys that have no CGKeyCode equivalent (e.g. modifier keys,
/// which are handled separately via CGEventFlagsChanged + kCGEventFlagMask*).
fn hid_to_cg(hid: u8) -> Option<CGKeyCode> {
    // Reference: IOHIDUsageTables.h + Carbon HIToolbox Events.h
    Some(match hid {
        // Letters a–z
        0x04 => 0,   // a
        0x05 => 11,  // b
        0x06 => 8,   // c
        0x07 => 2,   // d
        0x08 => 14,  // e
        0x09 => 3,   // f
        0x0A => 5,   // g
        0x0B => 4,   // h
        0x0C => 34,  // i
        0x0D => 38,  // j
        0x0E => 40,  // k
        0x0F => 37,  // l
        0x10 => 46,  // m
        0x11 => 45,  // n
        0x12 => 31,  // o
        0x13 => 35,  // p
        0x14 => 12,  // q
        0x15 => 15,  // r
        0x16 => 1,   // s
        0x17 => 17,  // t
        0x18 => 32,  // u
        0x19 => 9,   // v
        0x1A => 13,  // w
        0x1B => 7,   // x
        0x1C => 16,  // y
        0x1D => 6,   // z
        // Digits 1–0
        0x1E => 18,  // 1
        0x1F => 19,  // 2
        0x20 => 20,  // 3
        0x21 => 21,  // 4
        0x22 => 23,  // 5
        0x23 => 22,  // 6
        0x24 => 26,  // 7
        0x25 => 28,  // 8
        0x26 => 25,  // 9
        0x27 => 29,  // 0
        // Control cluster
        0x28 => 36,  // enter
        0x29 => 53,  // esc
        0x2A => 51,  // backspace/delete
        0x2B => 48,  // tab
        0x2C => 49,  // space
        0x2D => 27,  // -
        0x2E => 24,  // =
        0x2F => 33,  // [
        0x30 => 30,  // ]
        0x31 => 42,  // backslash
        0x33 => 41,  // ;
        0x34 => 39,  // '
        0x35 => 50,  // `
        0x36 => 43,  // ,
        0x37 => 47,  // .
        0x38 => 44,  // /
        // F-keys
        0x3A => 122, // F1
        0x3B => 120, // F2
        0x3C => 99,  // F3
        0x3D => 118, // F4
        0x3E => 96,  // F5
        0x3F => 97,  // F6
        0x40 => 98,  // F7
        0x41 => 100, // F8
        0x42 => 101, // F9
        0x43 => 109, // F10
        0x44 => 103, // F11
        0x45 => 111, // F12
        // Navigation
        0x49 => 114, // insert (fn+enter on Apple keyboards)
        0x4A => 115, // home
        0x4B => 116, // page up
        0x4C => 117, // delete forward
        0x4D => 119, // end
        0x4E => 121, // page down
        0x4F => 124, // right
        0x50 => 123, // left
        0x51 => 125, // down
        0x52 => 126, // up
        _ => return None,
    })
}

/// Convert HID modifier bitmask to CGEventFlags.
/// Bit layout: 0x01=lctrl 0x02=lshift 0x04=lalt 0x08=lmeta
///             0x10=rctrl 0x20=rshift 0x40=ralt 0x80=rmeta
fn modifier_bits_to_cg_flags(bits: u8) -> CGEventFlags {
    let mut flags: CGEventFlags = 0;
    if bits & 0x01 != 0 { flags |= kCGEventFlagMaskControl; }
    if bits & 0x10 != 0 { flags |= kCGEventFlagMaskControl; }
    if bits & 0x02 != 0 { flags |= kCGEventFlagMaskShift; }
    if bits & 0x20 != 0 { flags |= kCGEventFlagMaskShift; }
    if bits & 0x04 != 0 { flags |= kCGEventFlagMaskAlternate; }
    if bits & 0x40 != 0 { flags |= kCGEventFlagMaskAlternate; }
    if bits & 0x08 != 0 { flags |= kCGEventFlagMaskCommand; }
    if bits & 0x80 != 0 { flags |= kCGEventFlagMaskCommand; }
    flags
}

// ── Event injection ───────────────────────────────────────────────────────────

/// CGKeyCode for each modifier (used for kCGEventFlagsChanged events).
fn modifier_bit_to_cg_keycode(bit: u8) -> CGKeyCode {
    match bit {
        0x01 => 59,  // lctrl
        0x02 => 56,  // lshift
        0x04 => 58,  // lalt/option
        0x08 => 55,  // lmeta/command
        0x10 => 62,  // rctrl
        0x20 => 60,  // rshift
        0x40 => 61,  // ralt/option
        0x80 => 54,  // rmeta/command
        _    => 0,
    }
}

/// Inject a single synthetic key event via CGEventPost.
unsafe fn inject(hid: u8, modifier_bits: u8, value: i32) {
    let flags = modifier_bits_to_cg_flags(modifier_bits);

    if hid == 0 {
        // Modifier-only report: post a FlagsChanged event for each active modifier.
        // We synthesize a FlagsChanged for whichever modifiers are now live.
        for bit in [0x01u8, 0x02, 0x04, 0x08, 0x10, 0x20, 0x40, 0x80u8] {
            if modifier_bits & bit != 0 {
                let cg_kc = modifier_bit_to_cg_keycode(bit);
                let ev = CGEventCreateKeyboardEvent(std::ptr::null_mut(), cg_kc, true);
                if ev.is_null() { continue; }
                CGEventSetFlags(ev, flags);
                CGEventPost(kCGSessionEventTap, ev);
                CFRelease(ev);
            }
        }
        return;
    }

    let Some(cg_kc) = hid_to_cg(hid) else {
        warn!("macOS: no CGKeyCode for HID {hid:#04x} — dropping event");
        return;
    };

    let key_down = value == VALUE_DOWN;
    let ev = CGEventCreateKeyboardEvent(std::ptr::null_mut(), cg_kc, key_down);
    if ev.is_null() {
        warn!("macOS: CGEventCreateKeyboardEvent returned null for HID {hid:#04x}");
        return;
    }
    // Caps lock flag must never be set — we own caps as a layer key.
    let flags = flags & !kCGEventFlagMaskAlphaShift;
    CGEventSetFlags(ev, flags);
    CGEventPost(kCGSessionEventTap, ev);
    CFRelease(ev);
}

// ── IOHIDManager helpers ──────────────────────────────────────────────────────

unsafe fn hid_device_u32(device: IOHIDDeviceRef, key_str: &[u8]) -> Option<u32> {
    let cf_key = CFStringCreateWithCString(kCFAllocatorDefault, key_str.as_ptr(), kCFStringEncodingUTF8);
    let cf_val = IOHIDDeviceGetProperty(device, cf_key as CFStringRef);
    CFRelease(cf_key as *mut c_void);
    if cf_val.is_null() { return None; }
    let mut v: i32 = 0;
    CFNumberGetValue(cf_val, kCFNumberSInt32Type, &mut v as *mut _ as *mut c_void);
    Some(v as u32)
}

// ── IOHIDManager callbacks ────────────────────────────────────────────────────

unsafe extern "C" fn device_added(
    _context: *mut c_void,
    _result:  IOReturn,
    _sender:  *mut c_void,
    device:   IOHIDDeviceRef,
) {
    let vendor  = hid_device_u32(device, kIOHIDVendorIDKey_str).unwrap_or(0);
    let product = hid_device_u32(device, kIOHIDProductIDKey_str).unwrap_or(0);
    let ret = IOHIDDeviceOpen(device, kIOHIDOptionsTypeSeizeDevice);
    if ret == kIOReturnSuccess {
        info!("macOS: keyboard seized (vendor={vendor:#06x} product={product:#06x})");
    } else {
        warn!("macOS: seize failed: {ret:#x} (vendor={vendor:#06x} product={product:#06x})");
    }
}

unsafe extern "C" fn value_available(
    _context: *mut c_void,
    _result:  IOReturn,
    _sender:  *mut c_void,
    value:    IOHIDValueRef,
) {
    let element    = IOHIDValueGetElement(value);
    let usage_page = IOHIDElementGetUsagePage(element);
    if usage_page != kHIDPage_KeyboardOrKeypad { return; }

    let usage     = IOHIDElementGetUsage(element) as u8;

    // HID keyboard error/reserved codes — not real keys, skip silently.
    // 0x00 = No Event, 0x01 = ErrorRollOver, 0x02 = POSTFail, 0x03 = ErrorUndefined, 0xff = Reserved.
    if usage == 0x00 || usage == 0x01 || usage == 0x02 || usage == 0x03 || usage == 0xff {
        return;
    }

    let int_value = IOHIDValueGetIntegerValue(value);
    let ev_value  = if int_value != 0 { VALUE_DOWN } else { VALUE_UP };

    let modifier_bit = hid_modifier_bit(usage);
    let hid = if modifier_bit != 0 { 0 } else { usage };

    MACOS_STATE.with(|cell| {
        let mut borrow = cell.borrow_mut();
        let Some(state) = borrow.as_mut() else { return };

        // Recompile on config change or output switch.
        let output_now  = state.active_output.load(Ordering::Relaxed);
        let cfg_changed = state.cfg_rx.has_changed().unwrap_or(false);
        if cfg_changed {
            state.cfg_snapshot = state.cfg_rx.borrow_and_update().clone();
        }
        if cfg_changed || output_now != state.last_output_idx {
            state.last_output_idx = output_now;
            state.compiled = super::recompile(&state.cfg_snapshot, &state.active_output);
        }

        // Escape hatch: Ctrl+Shift+Esc × 3 within 2s exits the daemon.
        // HID 0x29 = Esc; modifier bits 0x01=lctrl, 0x02=lshift (or right equivalents).
        if hid == 0x29 && ev_value == VALUE_DOWN {
            let mods = state.layer.modifier_bits;
            let ctrl  = mods & (0x01 | 0x10) != 0;
            let shift = mods & (0x02 | 0x20) != 0;
            if ctrl && shift {
                if state.escape.register() {
                    info!("macOS: escape hatch triggered — exiting");
                    std::process::exit(0);
                }
                info!("macOS: escape hatch {}/3", state.escape.count);
                return; // consume the key, don't pass through
            }
        }

        let result = process_key(hid, modifier_bit, ev_value, &state.compiled, &mut state.layer);

        super::dispatch_result(&result, &state.cfg_snapshot, &state.active_output, &state.serial);

        for syn in &result.events {
            inject(syn.hid, syn.modifiers, syn.value);
        }
    });
}

// ── Accessibility permission ──────────────────────────────────────────────────

fn request_accessibility_if_needed() {
    if unsafe { AXIsProcessTrusted() } {
        info!("macOS: Accessibility permission already granted");
        return;
    }
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
        warn!("macOS: Accessibility not yet granted — approve in System Settings → Privacy & Security → Accessibility");
    }
}

// ── Public entry point ────────────────────────────────────────────────────────

pub fn run(
    cfg_rx:        watch::Receiver<DualieConfig>,
    serial:        SerialClient,
    active_output: ActiveOutput,
) -> Result<()> {
    request_accessibility_if_needed();

    let cfg_snapshot = cfg_rx.borrow().clone();
    let output_idx   = active_output.load(Ordering::Relaxed);
    let compiled     = super::recompile(&cfg_snapshot, &active_output);

    MACOS_STATE.with(|cell| {
        *cell.borrow_mut() = Some(MacosState {
            compiled,
            last_output_idx: output_idx,
            cfg_snapshot,
            cfg_rx,
            layer:         LayerState::default(),
            active_output,
            serial,
            escape:        EscapeHatch::new(),
        });
    });

    let _manager = unsafe {
        const kHIDPage_GenericDesktop_i32: i32 = kHIDPage_GenericDesktop as i32;
        const kHIDUsage_GD_Keyboard_i32:   i32 = kHIDUsage_GD_Keyboard   as i32;

        let mgr = IOHIDManagerCreate(kCFAllocatorDefault, kIOHIDOptionsTypeNone);
        if mgr.is_null() { bail!("IOHIDManagerCreate returned NULL"); }

        let matching = CFDictionaryCreateMutable(
            kCFAllocatorDefault, 2,
            &kCFTypeDictionaryKeyCallBacks as *const _ as *const c_void,
            &kCFTypeDictionaryValueCallBacks as *const _ as *const c_void,
        );
        let cf_page  = CFNumberCreate(kCFAllocatorDefault, kCFNumberSInt32Type, &kHIDPage_GenericDesktop_i32 as *const _ as *const c_void);
        let cf_usage = CFNumberCreate(kCFAllocatorDefault, kCFNumberSInt32Type, &kHIDUsage_GD_Keyboard_i32   as *const _ as *const c_void);
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

    info!("macOS: IOHIDManager open — intercepting keyboards (CGEventPost injection)");
    unsafe { CFRunLoopRun(); }
    bail!("CFRunLoopRun returned unexpectedly");
}
