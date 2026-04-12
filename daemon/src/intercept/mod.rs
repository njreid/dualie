/// intercept/mod.rs — Local keyboard remapping.
///
/// This module intercepts events from all keyboards attached to the machine
/// (built-in, USB direct) and applies the same remap config as the firmware
/// applies to the hardware-connected keyboard.
///
/// Virtual action dispatch has been removed from here: the RP2040 now sends
/// `DualieMessage::VirtualAction { slot }` over CDC-ACM serial directly to
/// `peer.rs`, bypassing the HID layer entirely.
///
/// # Phase 2: evdev + uinput (Linux) / Karabiner driver (macOS)
///
/// Platform implementations go in `linux.rs` and `macos.rs` respectively.
/// This module is a stub until Phase 2 is implemented.

use std::sync::Arc;
use tracing::info;

use crate::AppState;

// ── Entry point ───────────────────────────────────────────────────────────────

/// Spawn the local keyboard interceptor.
///
/// On Linux: grabs all keyboard `/dev/input/event*` devices with `EVIOCGRAB`
/// and writes remapped events to a uinput virtual device.
///
/// On macOS: installs a CGEventTap and re-injects remapped events via the
/// Karabiner-VirtualHIDDevice driver.
///
/// Currently a no-op stub — implementation in Phase 2/3.
pub fn run(_state: Arc<AppState>) -> anyhow::Result<()> {
    info!("Local keyboard intercept: not yet implemented (Phase 2/3)");
    // TODO Phase 3 (Linux):  intercept/linux.rs  — evdev EVIOCGRAB + uinput
    // TODO Phase 4 (macOS):  intercept/macos.rs  — CGEventTap + Karabiner driver
    Ok(())
}
