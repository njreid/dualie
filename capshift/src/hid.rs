#![allow(non_upper_case_globals, non_camel_case_types)]

/// hid.rs — macOS local keyboard interception.
///
/// Uses IOHIDManager to exclusively seize physical keyboards, runs each key
/// event through `chord::ChordState`, and posts the resulting HID
/// boot-protocol report to the Karabiner VirtualHIDDevice. Trimmed port of
/// `daemon/src/intercept/macos.rs` — layers/outputs/serial removed, replaced
/// by a single caps-lock `ChordState`.
///
/// # Requirements
///
/// - Karabiner-Elements must be installed and running (provides the virtual
///   HID keyboard driver).
/// - The binary must have Accessibility permission (System Settings ->
///   Privacy & Security -> Accessibility) for the exclusive device seize.
///
/// # Thread model
///
/// `run()` blocks on `CFRunLoopRun()` — call from a dedicated OS thread.
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::ffi::{c_char, c_void};

use anyhow::{bail, Result};
use tokio::sync::watch;
use tracing::{debug, info, warn};

use crate::chord::{Binding, ChordState, KeyOutcome};
use crate::keycodes::{hid_modifier_bit, CAPS_LOCK_HID};
use crate::kvhd::{build_report, KvhdHandle};

type IOReturn = i32;
type IOHIDManagerRef = *mut c_void;
type IOHIDDeviceRef = *mut c_void;
type IOHIDValueRef = *mut c_void;
type IOHIDElementRef = *mut c_void;
type CFRunLoopRef = *mut c_void;
type CFStringRef = *const c_void;
type CFAllocatorRef = *mut c_void;
type CFDictionaryRef = *mut c_void;

const kIOReturnSuccess: IOReturn = 0;
const kIOHIDOptionsTypeNone: u32 = 0x0;
const kIOHIDOptionsTypeSeizeDevice: u32 = 0x1;
const kHIDPage_GenericDesktop: u32 = 0x01;
const kHIDPage_KeyboardOrKeypad: u32 = 0x07;
const kHIDUsage_GD_Keyboard: u32 = 0x06;

type IOHIDDeviceCallback =
    unsafe extern "C" fn(context: *mut c_void, result: IOReturn, sender: *mut c_void, device: IOHIDDeviceRef);
type IOHIDValueCallback =
    unsafe extern "C" fn(context: *mut c_void, result: IOReturn, sender: *mut c_void, value: IOHIDValueRef);

#[link(name = "IOKit", kind = "framework")]
extern "C" {
    fn IOHIDManagerCreate(allocator: CFAllocatorRef, options: u32) -> IOHIDManagerRef;
    fn IOHIDManagerSetDeviceMatching(manager: IOHIDManagerRef, matching: CFDictionaryRef);
    fn IOHIDManagerOpen(manager: IOHIDManagerRef, options: u32) -> IOReturn;
    fn IOHIDManagerRegisterDeviceMatchingCallback(
        manager: IOHIDManagerRef,
        callback: IOHIDDeviceCallback,
        context: *mut c_void,
    );
    fn IOHIDManagerRegisterInputValueCallback(
        manager: IOHIDManagerRef,
        callback: IOHIDValueCallback,
        context: *mut c_void,
    );
    fn IOHIDManagerScheduleWithRunLoop(manager: IOHIDManagerRef, run_loop: CFRunLoopRef, mode: CFStringRef);
    fn IOHIDDeviceClose(device: IOHIDDeviceRef, options: u32) -> IOReturn;
    fn IOHIDValueGetIntegerValue(value: IOHIDValueRef) -> i64;
    fn IOHIDValueGetElement(value: IOHIDValueRef) -> IOHIDElementRef;
    fn IOHIDElementGetDevice(element: IOHIDElementRef) -> IOHIDDeviceRef;
    fn IOHIDElementGetUsage(element: IOHIDElementRef) -> u32;
    fn IOHIDElementGetUsagePage(element: IOHIDElementRef) -> u32;
    fn IOHIDDeviceGetProperty(device: IOHIDDeviceRef, key: CFStringRef) -> *const c_void;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFRunLoopGetCurrent() -> CFRunLoopRef;
    fn CFRunLoopRun();
    fn CFDictionaryCreateMutable(
        allocator: CFAllocatorRef,
        capacity: isize,
        key_callbacks: *const c_void,
        value_callbacks: *const c_void,
    ) -> CFDictionaryRef;
    fn CFDictionaryAddValue(dict: CFDictionaryRef, key: *const c_void, value: *const c_void);
    fn CFNumberCreate(allocator: CFAllocatorRef, the_type: i32, value_ptr: *const c_void) -> *mut c_void;
    fn CFStringCreateWithCString(
        allocator: CFAllocatorRef,
        c_str: *const c_char,
        encoding: u32,
    ) -> CFStringRef;
    fn CFNumberGetValue(number: *const c_void, the_type: i32, value_ptr: *mut c_void) -> u8;
    fn CFRelease(cf: *mut c_void);

    static kCFAllocatorDefault: CFAllocatorRef;
    static kCFRunLoopDefaultMode: CFStringRef;
    static kCFTypeDictionaryKeyCallBacks: c_void;
    static kCFTypeDictionaryValueCallBacks: c_void;
}

struct HidState {
    chord: ChordState,
    cfg_rx: watch::Receiver<HashMap<u8, Binding>>,
    virtual_pressed: HashSet<u8>,
    modifier_bits: u8,
    ignored_devices: HashSet<usize>,
    kvhd: KvhdHandle,
}

thread_local! {
    static HID_STATE: RefCell<Option<HidState>> = RefCell::new(None);
}

unsafe extern "C" fn device_added(
    _context: *mut c_void,
    _result: IOReturn,
    _sender: *mut c_void,
    device: IOHIDDeviceRef,
) {
    if is_karabiner_virtual_keyboard(device) {
        // The manager is opened with SeizeDevice so physical input cannot
        // leak through. Release its automatic open on our output device;
        // otherwise capshift would also suppress its own injected reports.
        let ret = IOHIDDeviceClose(device, kIOHIDOptionsTypeNone);
        HID_STATE.with(|cell| {
            if let Some(state) = cell.borrow_mut().as_mut() {
                state.ignored_devices.insert(device as usize);
            }
        });
        if ret == kIOReturnSuccess {
            info!("capshift: ignoring Karabiner virtual keyboard");
        } else {
            warn!("capshift: failed to release Karabiner virtual keyboard: {ret:#x}");
        }
        return;
    }

    // IOHIDManagerOpen performed the exclusive open for every matched device.
    info!("capshift: keyboard device seized");
}

unsafe fn is_karabiner_virtual_keyboard(device: IOHIDDeviceRef) -> bool {
    const K_ENCODING_UTF8: u32 = 0x0800_0100;
    const K_CF_NUMBER_SINT32_TYPE: i32 = 3;

    let vendor_key =
        CFStringCreateWithCString(kCFAllocatorDefault, c"VendorID".as_ptr(), K_ENCODING_UTF8);
    let product_key =
        CFStringCreateWithCString(kCFAllocatorDefault, c"ProductID".as_ptr(), K_ENCODING_UTF8);

    let mut vendor = 0i32;
    let mut product = 0i32;
    let vendor_number = IOHIDDeviceGetProperty(device, vendor_key);
    let product_number = IOHIDDeviceGetProperty(device, product_key);
    let vendor_ok = !vendor_number.is_null()
        && CFNumberGetValue(
            vendor_number,
            K_CF_NUMBER_SINT32_TYPE,
            &mut vendor as *mut _ as *mut c_void,
        ) != 0;
    let product_ok = !product_number.is_null()
        && CFNumberGetValue(
            product_number,
            K_CF_NUMBER_SINT32_TYPE,
            &mut product as *mut _ as *mut c_void,
        ) != 0;

    CFRelease(vendor_key as *mut c_void);
    CFRelease(product_key as *mut c_void);

    vendor_ok && product_ok && vendor == 0x16c0 && product == 0x27db
}

unsafe extern "C" fn value_available(
    _context: *mut c_void,
    _result: IOReturn,
    _sender: *mut c_void,
    value: IOHIDValueRef,
) {
    let element = IOHIDValueGetElement(value);
    let device = IOHIDElementGetDevice(element);
    let usage_page = IOHIDElementGetUsagePage(element);
    if usage_page != kHIDPage_KeyboardOrKeypad {
        return;
    }

    let usage = IOHIDElementGetUsage(element) as u8;
    let int_value = IOHIDValueGetIntegerValue(value);
    let down = int_value != 0;

    HID_STATE.with(|cell| {
        let mut borrow = cell.borrow_mut();
        let Some(state) = borrow.as_mut() else { return };

        if state.ignored_devices.contains(&(device as usize)) {
            return;
        }

        if state.cfg_rx.has_changed().unwrap_or(false) {
            let bindings = state.cfg_rx.borrow_and_update().clone();
            state.chord.set_bindings(bindings);
        }

        let modifier_bit = hid_modifier_bit(usage);
        if modifier_bit != 0 {
            if down {
                state.modifier_bits |= modifier_bit;
            } else {
                state.modifier_bits &= !modifier_bit;
            }
            post(state);
            return;
        }

        match state.chord.process(usage, down) {
            KeyOutcome::Swallow => {}
            KeyOutcome::Fire(action) => crate::actions::fire(&action),
            KeyOutcome::Passthrough => forward(state, usage, down),
            KeyOutcome::Forward(target) => {
                debug!(source = usage, target, down, "capshift: forwarding remapped key");
                forward(state, target, down)
            }
        }
    });
}

fn forward(state: &mut HidState, hid: u8, down: bool) {
    if down {
        state.virtual_pressed.insert(hid);
    } else {
        state.virtual_pressed.remove(&hid);
    }
    post(state);
}

fn post(state: &HidState) {
    let report = build_report(state.modifier_bits, &state.virtual_pressed);
    if let Err(e) = state.kvhd.post_report(&report) {
        warn!("capshift: KVHD post_report failed: {e}");
    }
}

/// Run the macOS keyboard interception loop. Blocks until an error occurs.
pub fn run(cfg_rx: watch::Receiver<HashMap<u8, Binding>>) -> Result<()> {
    let kvhd = KvhdHandle::open()?;
    info!("capshift: Karabiner VirtualHIDKeyboard connected");

    let bindings = cfg_rx.borrow().clone();
    let chord = ChordState::new(CAPS_LOCK_HID, bindings);

    HID_STATE.with(|cell| {
        *cell.borrow_mut() = Some(HidState {
            chord,
            cfg_rx,
            virtual_pressed: HashSet::new(),
            modifier_bits: 0,
            ignored_devices: HashSet::new(),
            kvhd,
        });
    });

    let _manager = unsafe {
        let mgr = IOHIDManagerCreate(kCFAllocatorDefault, kIOHIDOptionsTypeNone);
        if mgr.is_null() {
            bail!("IOHIDManagerCreate returned NULL");
        }

        let matching = CFDictionaryCreateMutable(
            kCFAllocatorDefault,
            2,
            &kCFTypeDictionaryKeyCallBacks as *const _ as *const c_void,
            &kCFTypeDictionaryValueCallBacks as *const _ as *const c_void,
        );
        let page_num = kHIDPage_GenericDesktop as i32;
        let usage_num = kHIDUsage_GD_Keyboard as i32;
        let cf_page = CFNumberCreate(kCFAllocatorDefault, 3, &page_num as *const _ as *const c_void);
        let cf_usage = CFNumberCreate(kCFAllocatorDefault, 3, &usage_num as *const _ as *const c_void);

        // kIOHIDDeviceUsagePageKey / kIOHIDDeviceUsageKey are #define'd C string
        // literals in IOKit's IOHIDKeys.h, not exported CFStringRef symbols — they
        // can't be linked via `extern "C" static`, so build the CFStrings ourselves.
        const K_ENCODING_UTF8: u32 = 0x0800_0100;
        let usage_page_key =
            CFStringCreateWithCString(kCFAllocatorDefault, c"DeviceUsagePage".as_ptr(), K_ENCODING_UTF8);
        let usage_key =
            CFStringCreateWithCString(kCFAllocatorDefault, c"DeviceUsage".as_ptr(), K_ENCODING_UTF8);

        CFDictionaryAddValue(matching, usage_page_key as *const c_void, cf_page);
        CFDictionaryAddValue(matching, usage_key as *const c_void, cf_usage);
        CFRelease(cf_page);
        CFRelease(cf_usage);
        CFRelease(usage_page_key as *mut c_void);
        CFRelease(usage_key as *mut c_void);

        IOHIDManagerSetDeviceMatching(mgr, matching);
        CFRelease(matching);

        IOHIDManagerRegisterDeviceMatchingCallback(mgr, device_added, std::ptr::null_mut());
        IOHIDManagerRegisterInputValueCallback(mgr, value_available, std::ptr::null_mut());
        IOHIDManagerScheduleWithRunLoop(mgr, CFRunLoopGetCurrent(), kCFRunLoopDefaultMode);

        // Seize on the manager's initial open. Opening the manager normally
        // and then calling IOHIDDeviceOpen(SeizeDevice) from device_added can
        // leave the manager's original non-exclusive open in place, allowing
        // the physical keystroke to leak alongside the injected remap.
        let ret = IOHIDManagerOpen(mgr, kIOHIDOptionsTypeSeizeDevice);
        if ret != kIOReturnSuccess {
            bail!("IOHIDManagerOpen failed: {ret:#x}");
        }
        mgr
    };

    info!("capshift: IOHIDManager open — watching for keyboards");
    unsafe {
        CFRunLoopRun();
    }

    bail!("CFRunLoopRun returned unexpectedly");
}
