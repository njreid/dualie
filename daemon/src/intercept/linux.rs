/// intercept/linux.rs — Linux evdev grab + uinput injection.
///
/// Enumerates keyboards via udev, grabs each device exclusively with
/// `EVIOCGRAB`, and re-injects remapped events through a uinput virtual
/// keyboard.  A udev monitor task dynamically grabs new keyboards as they
/// connect.
///
/// # Architecture
///
///   udev_monitor_task          keyboard_task (one per physical device)
///        │                            │
///        │  new keyboard added        │
///        ├──────────────────────────> │  evdev::Device::grab()
///        │                            │  loop { read event → process_key → uinput }
///        │                            │
///
/// All keyboard tasks share a single uinput virtual device (wrapped in
/// `Arc<Mutex<VirtualDevice>>`).

use std::os::unix::io::AsRawFd;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use evdev::{Device, EventType, Key};
use evdev::uinput::VirtualDeviceBuilder;
use tokio::sync::watch;
use tracing::{debug, error, info, warn};
use udev::MonitorBuilder;   // listen() → udev::Socket

use super::keycodes::{evdev_modifier_bit, evdev_to_hid, hid_to_evdev};
use super::remap::{process_key, LayerState, SyntheticKey};
use super::ActiveOutput;
use crate::config::DualieConfig;
use crate::peer::SerialClient;

// ── Entry point ───────────────────────────────────────────────────────────────

/// Spawn the Linux key intercept layer.  Runs forever; call from a dedicated
/// OS thread (evdev blocking I/O doesn't play well with async).
pub fn run(
    cfg_rx: watch::Receiver<DualieConfig>,
    serial: SerialClient,
    active_output: ActiveOutput,
) -> Result<()> {
    // Build the shared uinput virtual device.
    let uinput = Arc::new(Mutex::new(build_uinput()?));
    info!("uinput virtual keyboard created");

    // Enumerate and grab all currently attached keyboards.
    let keyboards = enumerate_keyboards()?;
    info!("found {} keyboard(s)", keyboards.len());

    for path in keyboards {
        spawn_keyboard_task(
            path,
            Arc::clone(&uinput),
            cfg_rx.clone(),
            Arc::clone(&active_output),
            serial.clone(),
        );
    }

    // Watch for new keyboards via udev.
    run_udev_monitor(uinput, cfg_rx, active_output, serial)?;

    Ok(())
}

// ── uinput device ────────────────────────────────────────────────────────────

fn build_uinput() -> Result<evdev::uinput::VirtualDevice> {
    // Advertise all standard keyboard keys.
    let mut keys = evdev::AttributeSet::<Key>::new();
    // Add the full range of keys (KEY_ESC through KEY_MAX that matter).
    for code in 1u16..=255 {
        keys.insert(Key::new(code));
    }

    VirtualDeviceBuilder::new()
        .context("uinput: VirtualDeviceBuilder::new")?
        .name("Dualie Virtual Keyboard")
        .with_keys(&keys)
        .context("uinput: with_keys")?
        .build()
        .context("uinput: build")
}

// ── Keyboard enumeration ──────────────────────────────────────────────────────

/// Return paths to all `/dev/input/event*` devices that udev marks as keyboards,
/// excluding any virtual (uinput) devices we created.
fn enumerate_keyboards() -> Result<Vec<PathBuf>> {
    let mut enumerator = udev::Enumerator::new()?;
    enumerator.match_subsystem("input")?;
    enumerator.match_property("ID_INPUT_KEYBOARD", "1")?;

    let mut paths = Vec::new();
    for dev in enumerator.scan_devices()? {
        if let Some(devnode) = dev.devnode() {
            let path = devnode.to_owned();
            // Skip our own uinput virtual device (if we can detect it).
            if is_virtual_device(&dev) {
                debug!("skipping virtual device {}", path.display());
                continue;
            }
            paths.push(path);
        }
    }
    Ok(paths)
}

fn is_virtual_device(dev: &udev::Device) -> bool {
    // uinput devices have DEVPATH containing "virtual"
    dev.devpath().to_string_lossy().contains("virtual")
}

// ── Per-keyboard task ─────────────────────────────────────────────────────────

fn spawn_keyboard_task(
    path: PathBuf,
    uinput: Arc<Mutex<evdev::uinput::VirtualDevice>>,
    cfg_rx: watch::Receiver<DualieConfig>,
    active_output: ActiveOutput,
    serial: SerialClient,
) {
    std::thread::spawn(move || {
        if let Err(e) = keyboard_task(path.clone(), uinput, cfg_rx, active_output, serial) {
            error!("keyboard task {}: {e:#}", path.display());
        }
    });
}

fn keyboard_task(
    path: PathBuf,
    uinput: Arc<Mutex<evdev::uinput::VirtualDevice>>,
    mut cfg_rx: watch::Receiver<DualieConfig>,
    active_output: ActiveOutput,
    serial: SerialClient,
) -> Result<()> {
    let mut device = Device::open(&path)
        .with_context(|| format!("opening {}", path.display()))?;

    if let Err(e) = device.grab() {
        let hint = match e.raw_os_error() {
            Some(libc::EBUSY) =>
                " — another process has exclusive grab; is dualie already running? \
                 (`systemctl --user stop dualie`)",
            Some(libc::EACCES) | Some(libc::EPERM) =>
                " — permission denied; are you in the 'input' group? \
                 (`sudo usermod -aG input $USER` then re-login, or run `just install`)",
            _ => "",
        };
        anyhow::bail!("grabbing {}{hint}", path.display());
    }

    info!("grabbed keyboard: {} ({})", device.name().unwrap_or("?"), path.display());

    let mut state = LayerState::default();
    let mut cfg_snapshot = cfg_rx.borrow().clone();

    loop {
        // Reload config if it changed.
        if cfg_rx.has_changed().unwrap_or(false) {
            cfg_snapshot = cfg_rx.borrow_and_update().clone();
        }

        let compiled = super::recompile(&cfg_snapshot, &active_output);

        let events = match device.fetch_events() {
            Ok(e) => e,
            Err(e) => {
                warn!("read error on {}: {e}", path.display());
                break;
            }
        };

        for ev in events {
            if ev.event_type() != EventType::KEY {
                // Forward non-key events (sync, etc.) unchanged.
                if let Ok(mut u) = uinput.lock() {
                    let _ = u.emit(&[ev]);
                }
                continue;
            }

            let evdev_code = ev.code();
            let value = ev.value();
            let modifier_bit = evdev_modifier_bit(evdev_code);
            let hid = if modifier_bit != 0 { 0 } else { evdev_to_hid(evdev_code) };

            let result = process_key(hid, modifier_bit, value, &compiled, &mut state);

            super::dispatch_result(&result, &active_output, &serial);

            // Inject synthetic key events into uinput.
            if !result.events.is_empty() {
                if let Ok(mut u) = uinput.lock() {
                    emit_synthetic(&mut *u, &result.events);
                }
            }
        }
    }

    info!("releasing keyboard: {}", path.display());
    Ok(())
}

// ── Emit synthetic events ─────────────────────────────────────────────────────

fn emit_synthetic(uinput: &mut evdev::uinput::VirtualDevice, events: &[SyntheticKey]) {
    for syn in events {
        let mut batch: Vec<evdev::InputEvent> = Vec::new();

        if syn.hid == 0 {
            // Modifier-only report: synthesise individual modifier key events.
            // We just emit a modifier sync — the modifier bits are already tracked
            // by the OS via the individual key events we emit for modifiers.
            // Nothing to do here; modifier state is implicit.
        } else {
            // Convert HID → evdev code.
            let evdev_code = hid_to_evdev(syn.hid);
            if evdev_code == 0 {
                debug!("no evdev code for HID 0x{:02X}", syn.hid);
                continue;
            }
            batch.push(evdev::InputEvent::new(
                EventType::KEY,
                evdev_code,
                syn.value,
            ));
            batch.push(evdev::InputEvent::new(EventType::SYNCHRONIZATION, 0, 0));
        }

        if !batch.is_empty() {
            if let Err(e) = uinput.emit(&batch) {
                warn!("uinput emit error: {e}");
            }
        }
    }
}

// ── udev monitor ─────────────────────────────────────────────────────────────

fn run_udev_monitor(
    uinput: Arc<Mutex<evdev::uinput::VirtualDevice>>,
    cfg_rx: watch::Receiver<DualieConfig>,
    active_output: ActiveOutput,
    serial: SerialClient,
) -> Result<()> {
    let socket = MonitorBuilder::new()?
        .match_subsystem("input")?
        .listen()?;

    info!("udev monitor started");

    let fd = socket.as_raw_fd();
    loop {
        // Block until the udev socket is readable (1 s timeout so we stay interruptible).
        let ready = unsafe {
            let mut pfd = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
            libc::poll(&mut pfd, 1, 1000)
        };
        if ready <= 0 {
            continue;
        }

        // socket.iter() is non-blocking; we've already confirmed readiness via poll.
        if let Some(event) = socket.iter().next() {
            // event Derefs to Device — action() and property_value() are on Device.
            if event.action() != Some(std::ffi::OsStr::new("add")) {
                continue;
            }
            if event.property_value("ID_INPUT_KEYBOARD") != Some(std::ffi::OsStr::new("1")) {
                continue;
            }
            if let Some(devnode) = event.device().devnode() {
                // Brief delay so the device node is accessible.
                std::thread::sleep(Duration::from_millis(300));
                let path = devnode.to_owned();
                info!("new keyboard detected: {}", path.display());
                spawn_keyboard_task(
                    path,
                    Arc::clone(&uinput),
                    cfg_rx.clone(),
                    Arc::clone(&active_output),
                    serial.clone(),
                );
            }
        }
    }
}
