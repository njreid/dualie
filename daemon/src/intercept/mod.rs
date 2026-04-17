/// intercept/mod.rs — Local keyboard remapping.
///
/// Intercepts events from all keyboards attached to the machine (built-in,
/// USB direct) and applies the remap config from the daemon.
///
/// Virtual action dispatch routes through `peer.rs` which receives
/// `DualieMessage::VirtualAction { slot }` over CDC-ACM from the RP2040.
/// The intercept layer fires actions directly for locally attached keyboards.
///
/// Platform implementations:
///   linux.rs  — evdev EVIOCGRAB + uinput
///   macos.rs  — IOHIDManager kIOHIDOptionsTypeSeizeDevice + Karabiner VirtualHIDDevice

use std::sync::{atomic::{AtomicU8, Ordering}, Arc};

use tokio::sync::watch;
use tracing::info;

use crate::config::DualieConfig;
use crate::peer::SerialClient;

pub mod keycodes;
pub mod remap;

#[cfg(target_os = "linux")]
mod linux;

#[cfg(target_os = "macos")]
mod macos_kvhd;
#[cfg(target_os = "macos")]
mod macos;

// ── Shared active-output state ────────────────────────────────────────────────

/// Index of the currently active output, shared between the serial peer
/// (which receives `ActiveOutput` messages from the RP2040) and the intercept
/// layer (which switches output locally on caps+jump/swap).
pub type ActiveOutput = Arc<AtomicU8>;

/// Create the shared active-output state (starts at output 0 = A).
pub fn new_active_output() -> ActiveOutput {
    Arc::new(AtomicU8::new(0))
}

// ── Shared intercept helpers ──────────────────────────────────────────────────

/// Build a `CompiledOutputConfig` for whichever output is currently active.
///
/// Called at the top of every event loop iteration (Linux) or on every key
/// event (macOS) so that output switches and config hot-reloads take effect
/// immediately without any extra bookkeeping.
pub fn recompile(cfg: &DualieConfig, active_output: &ActiveOutput) -> remap::CompiledOutputConfig {
    use crate::config::MachineConfig;
    let output_idx = active_output.load(Ordering::Relaxed);
    // Resolve active port → machine config; fall back to an empty config if
    // no machine is assigned to this port.
    let machine = cfg.resolve_port(output_idx as usize).unwrap_or_default();
    remap::CompiledOutputConfig::from_config(&machine, output_idx, 2)
}

/// Dispatch the side-effects of a `ProcessResult` — output switch, clipboard
/// pull, and virtual-action execution — identically on every platform.
pub fn dispatch_result(
    result: &remap::ProcessResult,
    cfg: &crate::config::DualieConfig,
    active_output: &ActiveOutput,
    serial: &SerialClient,
) {
    if let Some(target) = result.switch_output {
        info!("switching active output to {}", target);
        active_output.store(target, Ordering::Relaxed);
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let serial = serial.clone();
            handle.spawn(async move {
                serial.send(dualie_proto::DualieMessage::ActiveOutput { output: target }).await;
            });
        }
    }

    if result.clip_pull {
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let serial = serial.clone();
            handle.spawn(async move {
                serial.send(dualie_proto::DualieMessage::ClipboardPull).await;
            });
        }
    }

    if let Some(slot) = result.fire_action {
        info!(slot, "firing virtual action");
        let port_idx = active_output.load(Ordering::Relaxed) as usize;
        if let Some(machine) = cfg.resolve_port(port_idx) {
            if let Some(action) = machine.virtual_actions.get(slot as usize) {
                crate::launch::fire(action);
            } else {
                tracing::warn!(slot, "virtual action slot out of range");
            }
        }
    }
}

// ── Entry point ───────────────────────────────────────────────────────────────

/// Spawn the local keyboard interceptor.
///
/// On Linux: grabs all keyboard `/dev/input/event*` devices with `EVIOCGRAB`
/// and writes remapped events to a uinput virtual device.
///
/// On macOS: installs a CGEventTap and re-injects remapped events via the
/// Karabiner-VirtualHIDDevice driver.  (Phase 4 — not yet implemented.)
pub fn run(
    cfg_rx: watch::Receiver<DualieConfig>,
    serial: SerialClient,
    active_output: ActiveOutput,
) -> anyhow::Result<()> {
    #[cfg(target_os = "linux")]
    {
        linux::run(cfg_rx, serial, active_output)
    }

    #[cfg(target_os = "macos")]
    {
        macos::run(cfg_rx, serial, active_output)
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = (cfg_rx, serial, active_output);
        tracing::info!("local keyboard intercept: not yet implemented on this platform");
        Ok(())
    }
}
