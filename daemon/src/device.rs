/// Lightweight device-state sidecar.
///
/// The heavy HID I/O is done in the browser via WebHID; the daemon just needs
/// to know which output is currently active so it can look up the right
/// VirtualAction table when a vkey fires.
///
/// The browser (or any API caller) pushes the active output index via
/// `PUT /api/v1/device/output`.  Until then we default to output 0.
use std::sync::atomic::{AtomicU8, Ordering};

/// Shared, lock-free active-output tracker.
/// Embed directly in `AppState` (which is already behind an `Arc`).
#[derive(Debug, Default)]
pub struct DeviceState {
    /// 0 = OUTPUT_A, 1 = OUTPUT_B
    active_output: AtomicU8,
}

impl DeviceState {
    /// Which output is currently active (0 or 1).
    pub fn active_output(&self) -> usize {
        self.active_output.load(Ordering::Relaxed) as usize
    }

    /// Called by the web layer when the browser reports an output switch.
    pub fn set_active_output(&self, idx: usize) {
        self.active_output
            .store((idx & 1) as u8, Ordering::Relaxed);
    }
}
