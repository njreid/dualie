/// input_bridge.rs — User-daemon side of the root-input socket protocol.
///
/// Connects to the Unix socket served by `dualie-input` (root daemon), pushes
/// the current config on startup and on every hot-reload, receives `FromInput`
/// events (SwitchOutput, FireAction, ClipPull) and dispatches them.

use std::os::unix::net::UnixStream;
use std::sync::atomic::Ordering;
use std::time::Duration;

use tokio::sync::watch;
use tracing::{info, warn};

use crate::config::DualieConfig;
use crate::intercept::ActiveOutput;
use dualie_proto::input_proto::{
    FromInput, INPUT_SOCKET,
    decode_from_input, encode_to_input, read_frame, write_frame, ToInput,
};

/// Spawn a background task that maintains the connection to `dualie-input`.
///
/// On each connect:
///   1. Push the current config as `ConfigSnapshot`.
///   2. Spawn a reader thread for `FromInput` events.
///   3. Watch for config changes and push `ConfigSnapshot` on each reload.
///   4. If the read thread reports a disconnect, close and reconnect.
pub fn spawn(
    cfg_rx:        watch::Receiver<DualieConfig>,
    active_output: ActiveOutput,
    serial:        crate::peer::SerialClient,
) {
    tokio::spawn(bridge_loop(cfg_rx, active_output, serial));
}

async fn bridge_loop(
    mut cfg_rx:    watch::Receiver<DualieConfig>,
    active_output: ActiveOutput,
    serial:        crate::peer::SerialClient,
) {
    loop {
        match try_connect(&mut cfg_rx, &active_output, &serial).await {
            Ok(()) => info!("input bridge: disconnected, reconnecting in 2s…"),
            Err(e) => warn!("input bridge: {e}, reconnecting in 2s…"),
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

async fn try_connect(
    cfg_rx:        &mut watch::Receiver<DualieConfig>,
    active_output: &ActiveOutput,
    serial:        &crate::peer::SerialClient,
) -> anyhow::Result<()> {
    let stream = tokio::task::spawn_blocking(|| {
        UnixStream::connect(INPUT_SOCKET)
    }).await??;

    info!("input bridge: connected to {INPUT_SOCKET}");

    let mut writer = stream.try_clone()?;

    // Push initial config snapshot.
    push_config(&mut writer, &cfg_rx.borrow())?;

    // Push current active output.
    push_active_output(&mut writer, active_output.load(Ordering::Relaxed))?;

    // Spawn blocking read loop in a thread; signal disconnects via oneshot.
    let (disc_tx, mut disc_rx) = tokio::sync::oneshot::channel::<()>();
    let mut reader = stream;
    let cfg_rx_for_dispatch = cfg_rx.clone();
    let active_for_dispatch = active_output.clone();
    let serial_for_dispatch = serial.clone();

    std::thread::spawn(move || {
        loop {
            match read_frame(&mut reader) {
                Ok(body) => {
                    match decode_from_input(&body) {
                        Ok(ev) => dispatch(ev, &cfg_rx_for_dispatch, &active_for_dispatch, &serial_for_dispatch),
                        Err(e) => warn!("input bridge: decode: {e}"),
                    }
                }
                Err(_) => {
                    let _ = disc_tx.send(());
                    break;
                }
            }
        }
    });

    // Main loop: watch for config changes, push them, or stop on disconnect.
    loop {
        tokio::select! {
            changed = cfg_rx.changed() => {
                if changed.is_err() { break; }
                push_config(&mut writer, &cfg_rx.borrow())?;
            }
            _ = &mut disc_rx => {
                break;
            }
        }
    }

    Ok(())
}

fn push_config(writer: &mut UnixStream, cfg: &DualieConfig) -> anyhow::Result<()> {
    let mut cbor = Vec::new();
    ciborium::into_writer(cfg, &mut cbor)?;
    let frame = encode_to_input(&ToInput::ConfigSnapshot(cbor))?;
    write_frame(writer, &frame)?;
    info!("input bridge: pushed config snapshot ({} bytes)", frame.len());
    Ok(())
}

fn push_active_output(writer: &mut UnixStream, idx: u8) -> anyhow::Result<()> {
    let frame = encode_to_input(&ToInput::SetActiveOutput(idx))?;
    write_frame(writer, &frame)?;
    Ok(())
}

fn dispatch(
    event:         FromInput,
    cfg_rx:        &watch::Receiver<DualieConfig>,
    active_output: &ActiveOutput,
    serial:        &crate::peer::SerialClient,
) {
    match event {
        FromInput::SwitchOutput(target) => {
            info!("input bridge: switch output → {target}");
            active_output.store(target, Ordering::Relaxed);
            let serial = serial.clone();
            if let Ok(handle) = tokio::runtime::Handle::try_current() {
                handle.spawn(async move {
                    serial.send(dualie_proto::DualieMessage::ActiveOutput { output: target }).await;
                });
            }
        }
        FromInput::FireAction(slot) => {
            info!("input bridge: fire action slot {slot}");
            let cfg     = cfg_rx.borrow();
            let port    = active_output.load(Ordering::Relaxed) as usize;
            let machine = cfg.resolve_port(port)
                .unwrap_or_else(|| cfg.default_machine.clone());
            if let Some(action) = machine.virtual_actions.get(slot as usize) {
                crate::launch::fire(action);
            } else {
                warn!("input bridge: action slot {slot} out of range");
            }
        }
        FromInput::ClipPull => {
            info!("input bridge: clipboard pull");
            let serial = serial.clone();
            if let Ok(handle) = tokio::runtime::Handle::try_current() {
                handle.spawn(async move {
                    serial.send(dualie_proto::DualieMessage::ClipboardPull).await;
                });
            }
        }
    }
}
