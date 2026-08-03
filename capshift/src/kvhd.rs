#![allow(non_upper_case_globals, non_camel_case_types)]

/// kvhd.rs — IOKit bindings for Karabiner-VirtualHIDDevice.
///
/// Provides a handle to the Karabiner DriverKit VirtualHIDKeyboard user client
/// so the daemon can post 8-byte HID boot-protocol keyboard reports. Ported
/// from `daemon/src/intercept/macos_kvhd.rs` — unchanged, since this file has
/// no KVM-specific logic to strip.
///
/// # Karabiner-DriverKit-VirtualHIDDevice
///
/// Service class name (DriverKit, Karabiner-Elements >= 14):
///   `org_pqrs_Karabiner_DriverKit_VirtualHIDKeyboard`
///
/// Legacy kext name (Karabiner-Elements <= 13):
///   `org_pqrs_driver_Karabiner_VirtualHIDDevice_VirtualHIDKeyboard`
///
/// User client IOKit selector:
///   0 = `postReport` — accepts an 8-byte keyboard input report buffer
///
/// # Report format (USB HID Boot Protocol Keyboard)
///
/// ```text
///   byte 0   modifier flags
///   byte 1   reserved        (0x00)
///   bytes 2-7  up to 6 simultaneous HID keycodes (pad with 0x00)
/// ```

use std::ffi::c_void;

use anyhow::{bail, Result};

type IOReturn = i32;
type io_object_t = u32;
type io_service_t = io_object_t;
type io_connect_t = io_object_t;
type mach_port_t = u32;

const kIOReturnSuccess: IOReturn = 0;

const KVHD_DRIVERKIT_SERVICE: &[u8] = b"org_pqrs_Karabiner_DriverKit_VirtualHIDKeyboard\0";
const KVHD_KEXT_SERVICE: &[u8] = b"org_pqrs_driver_Karabiner_VirtualHIDDevice_VirtualHIDKeyboard\0";

const SELECTOR_POST_REPORT: u32 = 0;

#[link(name = "IOKit", kind = "framework")]
extern "C" {
    fn IOServiceGetMatchingService(master_port: mach_port_t, matching: *mut c_void) -> io_service_t;
    fn IOServiceMatching(name: *const u8) -> *mut c_void;
    fn IOServiceOpen(
        service: io_service_t,
        owning_task: mach_port_t,
        connect_type: u32,
        connect: *mut io_connect_t,
    ) -> IOReturn;
    fn IOServiceClose(connect: io_connect_t) -> IOReturn;
    fn IOObjectRelease(object: io_object_t) -> IOReturn;
    fn IOConnectCallStructMethod(
        connection: io_connect_t,
        selector: u32,
        input_struct: *const c_void,
        input_struct_count: usize,
        output_struct: *mut c_void,
        output_struct_count_p: *mut usize,
    ) -> IOReturn;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn mach_task_self_() -> mach_port_t;
}

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
            let ret = unsafe { IOServiceOpen(service, mach_task_self_(), 0, &mut connect) };
            unsafe {
                IOObjectRelease(service);
            }
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
        unsafe {
            IOServiceClose(self.connect);
        }
    }
}

/// Build an 8-byte HID boot-protocol keyboard report from the current state.
pub fn build_report(modifier_bits: u8, pressed: &std::collections::HashSet<u8>) -> [u8; 8] {
    let mut report = [0u8; 8];
    report[0] = modifier_bits;
    for (i, &kc) in pressed.iter().take(6).enumerate() {
        report[2 + i] = kc;
    }
    report
}
