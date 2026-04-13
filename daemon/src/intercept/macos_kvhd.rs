/// intercept/macos_kvhd.rs — IOKit bindings for Karabiner-VirtualHIDDevice.
///
/// Provides a handle to the Karabiner DriverKit VirtualHIDKeyboard user client
/// so the daemon can post 8-byte HID boot-protocol keyboard reports.
///
/// # Karabiner-DriverKit-VirtualHIDDevice
///
/// Service class name (DriverKit, Karabiner-Elements ≥ 14):
///   `org_pqrs_Karabiner_DriverKit_VirtualHIDKeyboard`
///
/// Legacy kext name (Karabiner-Elements ≤ 13):
///   `org_pqrs_driver_Karabiner_VirtualHIDDevice_VirtualHIDKeyboard`
///
/// User client IOKit selector (from karabiner-driverkit/src/Extension/.../UserClient):
///   0 = `postReport` — accepts an 8-byte keyboard input report buffer
///
/// # Report format (USB HID Boot Protocol Keyboard)
///
/// ```text
///   byte 0   modifier flags  (same bitmask as our LayerState::modifier_bits)
///   byte 1   reserved        (0x00)
///   bytes 2–7  up to 6 simultaneous HID keycodes (pad with 0x00)
/// ```
///
/// Source references:
///   https://github.com/pqrs-org/Karabiner-DriverKit-VirtualHIDDevice
///   src/Extension/KarabinerDriverKitVirtualHIDKeyboard/UserClient.cpp

#![allow(non_upper_case_globals, non_camel_case_types)]

use std::ffi::c_void;

use anyhow::{bail, Context, Result};

// ── IOKit types ───────────────────────────────────────────────────────────────

type IOReturn     = i32;
type io_object_t  = u32;
type io_service_t = io_object_t;
type io_connect_t = io_object_t;
type mach_port_t  = u32;

const kIOReturnSuccess: IOReturn = 0;

// ── IOKit service class names (try DriverKit first, fall back to kext) ────────

const KVHD_DRIVERKIT_SERVICE: &[u8] =
    b"org_pqrs_Karabiner_DriverKit_VirtualHIDKeyboard\0";
const KVHD_KEXT_SERVICE: &[u8] =
    b"org_pqrs_driver_Karabiner_VirtualHIDDevice_VirtualHIDKeyboard\0";

// IOKit user client selector for `postReport`.
const SELECTOR_POST_REPORT: u32 = 0;

// ── FFI ───────────────────────────────────────────────────────────────────────

#[link(name = "IOKit", kind = "framework")]
extern "C" {
    fn IOServiceGetMatchingService(
        master_port: mach_port_t,
        matching: *mut c_void,  // CFDictionaryRef consumed
    ) -> io_service_t;

    fn IOServiceOpen(
        service:      io_service_t,
        owning_task:  mach_port_t,
        connect_type: u32,
        connect:      *mut io_connect_t,
    ) -> IOReturn;

    fn IOServiceClose(connect: io_connect_t) -> IOReturn;
    fn IOObjectRelease(object: io_object_t) -> IOReturn;

    fn IOConnectCallStructMethod(
        connection:   io_connect_t,
        selector:     u32,
        input_struct: *const c_void,
        input_struct_count: usize,
        output_struct: *mut c_void,
        output_struct_count_p: *mut usize,
    ) -> IOReturn;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn IOServiceMatching(name: *const u8) -> *mut c_void; // returns CFMutableDictionaryRef
    fn mach_task_self_() -> mach_port_t;
}

// ── Public API ────────────────────────────────────────────────────────────────

/// An open connection to the Karabiner VirtualHIDKeyboard IOKit user client.
pub struct KvhdHandle {
    connect: io_connect_t,
}

impl KvhdHandle {
    /// Open a connection to the Karabiner VirtualHIDKeyboard service.
    /// Tries the DriverKit service name first, then the legacy kext name.
    pub fn open() -> Result<Self> {
        for name in [KVHD_DRIVERKIT_SERVICE, KVHD_KEXT_SERVICE] {
            let matching = unsafe { IOServiceMatching(name.as_ptr()) };
            if matching.is_null() {
                continue;
            }
            let service = unsafe { IOServiceGetMatchingService(0, matching) };
            if service == 0 {
                continue;
            }
            let mut connect: io_connect_t = 0;
            let ret = unsafe {
                IOServiceOpen(service, mach_task_self_(), 0, &mut connect)
            };
            unsafe { IOObjectRelease(service); }
            if ret != kIOReturnSuccess {
                bail!("IOServiceOpen failed: {ret:#x}");
            }
            return Ok(Self { connect });
        }
        bail!(
            "Karabiner VirtualHIDKeyboard service not found — \
             is Karabiner-Elements installed and running?"
        );
    }

    /// Post an 8-byte HID boot-protocol keyboard report to the virtual device.
    ///
    /// `report[0]` = modifier flags (HID bitmask)
    /// `report[1]` = reserved (0x00)
    /// `report[2..8]` = up to 6 simultaneous keycodes (pad with 0x00)
    pub fn post_report(&self, report: &[u8; 8]) -> Result<()> {
        let ret = unsafe {
            IOConnectCallStructMethod(
                self.connect,
                SELECTOR_POST_REPORT,
                report.as_ptr() as *const c_void,
                8,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if ret != kIOReturnSuccess {
            bail!("IOConnectCallStructMethod(postReport) failed: {ret:#x}");
        }
        Ok(())
    }
}

impl Drop for KvhdHandle {
    fn drop(&mut self) {
        unsafe { IOServiceClose(self.connect); }
    }
}

// ── Report builder ────────────────────────────────────────────────────────────

/// Build an 8-byte HID boot-protocol keyboard report from the current state.
///
/// `modifier_bits` — HID modifier bitmask (LayerState::modifier_bits).
/// `pressed`       — set of HID keycodes currently held down (up to 6).
pub fn build_report(modifier_bits: u8, pressed: &std::collections::HashSet<u8>) -> [u8; 8] {
    let mut report = [0u8; 8];
    report[0] = modifier_bits;
    // report[1] = 0x00 (reserved)
    for (i, &kc) in pressed.iter().take(6).enumerate() {
        report[2 + i] = kc;
    }
    report
}
