/// Virtual-key interceptor.
///
/// The Pico sends virtual keycodes (F13-F24, Execute-Cut, Intl1-8, Lang1-4)
/// to represent user-defined actions.  This module watches for those keycodes
/// at the OS level and dispatches the configured `VirtualAction` without
/// letting the keycode reach any other application.
///
/// # Platform notes
/// - **macOS**  – uses rdev's `grab` API (backed by CGEventTap).  The callback
///   returns `None` to suppress matched events.
/// - **Linux**  – uses rdev's `listen` API; Linux rdev cannot suppress events,
///   so vkeys will also reach other apps.  Users should configure the WM to
///   ignore them (assign them to nothing).
use std::sync::Arc;
use tracing::{debug, error, warn};

use crate::config::VirtualAction;
use crate::platform;
use crate::AppState;

mod vkey_map;
use vkey_map::rdev_key_to_vslot;

// ── Entry point ───────────────────────────────────────────────────────────────

/// Block forever, intercepting virtual key events.
/// Must be called on a dedicated OS thread (rdev blocks the thread).
pub fn run(state: Arc<AppState>) -> anyhow::Result<()> {
    #[cfg(target_os = "macos")]
    return run_macos(state);

    #[cfg(target_os = "linux")]
    return run_linux(state);

    #[allow(unreachable_code)]
    {
        warn!("Key interception not supported on this platform");
        Ok(())
    }
}

// ── Shared dispatch ───────────────────────────────────────────────────────────

/// Dispatch a virtual action for slot `vslot` on the given output.
/// Returns true if an action was taken (and the keycode should be suppressed).
fn dispatch(state: &AppState, vslot: usize) -> bool {
    // Read config synchronously — we're on the interceptor thread.
    // `blocking_read` is valid because this is not a Tokio context.
    let cfg = state.config.blocking_read();
    let output_idx = state.device.active_output();

    let action = cfg
        .outputs
        .get(output_idx)
        .and_then(|o| o.virtual_actions.get(vslot))
        .cloned()
        .unwrap_or(VirtualAction::Unset);

    match action {
        VirtualAction::Unset => {
            debug!("vslot {vslot} on output {output_idx}: unset, passing through");
            false
        }
        VirtualAction::AppLaunch { app_id, label } => {
            debug!("vslot {vslot}: launching '{label}' ({app_id})");
            if let Err(e) = platform::launch_app(&app_id) {
                error!("launch_app({app_id}): {e}");
            }
            true
        }
        VirtualAction::ShellCommand { command, label } => {
            debug!("vslot {vslot}: running shell cmd '{label}'");
            run_shell(&command);
            true
        }
    }
}

fn run_shell(command: &str) {
    let cmd = command.to_owned();
    std::thread::spawn(move || {
        let status = std::process::Command::new("sh")
            .arg("-c")
            .arg(&cmd)
            .status();
        if let Err(e) = status {
            error!("shell command '{cmd}': {e}");
        }
    });
}

// ── macOS (rdev grab – suppresses matched events) ─────────────────────────────

#[cfg(target_os = "macos")]
fn run_macos(state: Arc<AppState>) -> anyhow::Result<()> {
    use rdev::{grab, Event, EventType};

    tracing::info!("Starting key interceptor (macOS CGEventTap)");

    // rdev's grab callback must be 'static.  We keep the Arc alive for the
    // process lifetime by converting to a raw pointer — the interceptor thread
    // never exits, so this is safe.
    let ptr: *const AppState = Arc::into_raw(state);
    // SAFETY: pointer is valid for the process lifetime.
    let state: &'static AppState = unsafe { &*ptr };

    grab(move |event: Event| -> Option<Event> {
        if let EventType::KeyPress(key) = &event.event_type {
            if let Some(slot) = rdev_key_to_vslot(key) {
                if dispatch(state, slot) {
                    return None; // suppress
                }
            }
        }
        Some(event)
    })
    .map_err(|e| anyhow::anyhow!("rdev grab failed: {e:?}"))?;

    Ok(())
}

// ── Linux (rdev listen – cannot suppress, just dispatch) ──────────────────────

#[cfg(target_os = "linux")]
fn run_linux(state: Arc<AppState>) -> anyhow::Result<()> {
    use rdev::{listen, Event, EventType};

    tracing::info!("Starting key interceptor (Linux rdev/evdev — note: suppression not supported)");

    let cb = move |event: Event| {
        if let EventType::KeyPress(key) = &event.event_type {
            if let Some(slot) = rdev_key_to_vslot(&key) {
                dispatch(&state, slot);
            }
        }
    };

    listen(cb).map_err(|e| anyhow::anyhow!(
        "rdev listen failed: {e:?}\n\
         \n\
         On Linux, rdev needs read access to /dev/input/event* devices.\n\
         Fix: add your user to the 'input' group, then log out and back in:\n\
         \n\
           sudo usermod -aG input $USER\n\
         \n\
         Or add a udev rule:\n\
           echo 'KERNEL==\"event*\", SUBSYSTEM==\"input\", GROUP=\"input\", MODE=\"0660\"' \\\n\
             | sudo tee /etc/udev/rules.d/99-input.rules\n\
           sudo udevadm control --reload && sudo udevadm trigger"
    ))?;
    Ok(())
}
