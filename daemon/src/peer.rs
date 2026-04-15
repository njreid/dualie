/// peer.rs — CDC-ACM serial peer to the local RP2040.
///
/// Runs as a background task that owns the serial connection.  Reconnects
/// automatically when the device disappears or an error occurs.
///
/// Responsibilities:
///   1. Open the CDC-ACM device via `SerialPeer::open` (explicit path or
///      auto-detected via `detect_path`).
///   2. Split into read/write halves; run a TX task and an RX dispatch loop
///      concurrently without any shared locking on the stream.
///   3. Expose a `SerialClient` handle so other modules can send messages.

use anyhow::Result;
use dualie_proto::{DualieMessage, SerialPeer, SerialPeerWriter};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use tokio::time::{Duration, sleep};
use tracing::{error, info, warn};

/// Delay used when the device exists but the connection attempt fails
/// (e.g. permissions, protocol error) — avoids a busy-retry.
const RECONNECT_DELAY: Duration = Duration::from_secs(2);

/// Minimum firmware version the daemon considers compatible.
/// If the RP2040 reports a lower version, the daemon logs a warning and
/// suggests running `just flash` to upgrade.
pub const FIRMWARE_MIN_COMPATIBLE: u32 = 1;

/// Global connection state — set true while the serial peer is connected.
/// Read by the status server without going through a channel.
pub static CONNECTED: AtomicBool = AtomicBool::new(false);

const TX_QUEUE: usize = 64;

// ── SerialClient handle ───────────────────────────────────────────────────────

/// Cloneable handle for sending messages to the RP2040 over CDC-ACM serial.
///
/// Messages sent while disconnected are silently dropped.
#[derive(Clone)]
pub struct SerialClient {
    inner: Arc<Mutex<Option<mpsc::Sender<DualieMessage>>>>,
}

impl SerialClient {
    fn new() -> Self {
        Self { inner: Arc::new(Mutex::new(None)) }
    }

    async fn set_sender(&self, tx: Option<mpsc::Sender<DualieMessage>>) {
        CONNECTED.store(tx.is_some(), Ordering::Relaxed);
        *self.inner.lock().await = tx;
    }

    /// Send a message to the RP2040.  Silently drops if disconnected or queue full.
    /// Used by the intercept layer (Phase 3+) to push ActiveOutput changes.
    pub async fn send(&self, msg: DualieMessage) {
        if let Some(tx) = self.inner.lock().await.as_ref() {
            let _ = tx.try_send(msg);
        }
    }
}

// ── TX writer task ────────────────────────────────────────────────────────────

async fn tx_task(mut writer: SerialPeerWriter, mut rx: mpsc::Receiver<DualieMessage>) {
    while let Some(msg) = rx.recv().await {
        if let Err(e) = writer.send(&msg).await {
            info!("serial tx task ending: {e}");
            break;
        }
    }
}

// ── Inbound dispatch ──────────────────────────────────────────────────────────

async fn dispatch(msg: DualieMessage, client: &SerialClient) {
    match msg {
        DualieMessage::Ping => {}

        DualieMessage::FirmwareInfo { version } => {
            info!(version, "RP2040 firmware version");
            if version < FIRMWARE_MIN_COMPATIBLE {
                warn!(
                    version,
                    min = FIRMWARE_MIN_COMPATIBLE,
                    "RP2040 firmware is outdated — run `just flash` to upgrade"
                );
            }
        }

        DualieMessage::VirtualAction { slot } => {
            info!(slot, "virtual action from RP2040");
        }

        DualieMessage::ActiveOutput { output } => {
            info!(output, "active output changed by RP2040");
        }

        DualieMessage::ClipboardPush(content) => {
            info!(len = content.text.len(), "clipboard received — writing to OS clipboard");
            let text = content.text.clone();
            if let Err(e) = tokio::task::spawn_blocking(move || crate::clipboard::write_text(&text)).await {
                warn!("clipboard write error: {e}");
            }
        }

        DualieMessage::ClipboardPull => {
            info!("clipboard pull requested — reading OS clipboard");
            let client = client.clone();
            tokio::task::spawn_blocking(move || {
                match crate::clipboard::read_text() {
                    Ok(text) => {
                        let msg = DualieMessage::ClipboardPush(dualie_proto::ClipboardText { text });
                        tokio::runtime::Handle::current().block_on(client.send(msg));
                    }
                    Err(e) => warn!("clipboard read error: {e}"),
                }
            });
        }

        DualieMessage::SyncChunk(_) | DualieMessage::SyncAck { .. } => {
            crate::file_sync::handle_incoming(msg);
        }

        DualieMessage::Error { message } => {
            warn!("RP2040 error: {message}");
        }

        other => {
            warn!("unhandled message from RP2040: {:?}", other);
        }
    }
}

// ── Single connection lifecycle ───────────────────────────────────────────────

async fn run_once(serial_path: &str, client: &SerialClient) -> Result<()> {
    info!(serial_path, "opening CDC-ACM serial connection");

    let peer = SerialPeer::open(std::path::Path::new(serial_path))?;
    let (writer, mut reader) = peer.into_split();

    // Arm the outbound channel so SerialClient::send works while connected.
    let (tx, rx) = mpsc::channel::<DualieMessage>(TX_QUEUE);
    client.set_sender(Some(tx)).await;

    // TX runs in its own task; RX drives the current task.
    tokio::spawn(tx_task(writer, rx));

    loop {
        let msg = reader.recv().await?;
        dispatch(msg, client).await;
    }
}

// ── Device wait (udev on Linux, poll fallback elsewhere) ──────────────────────

/// Wait until `path` is present on the filesystem.
///
/// On Linux: subscribes to udev `tty` add events and returns as soon as the
/// exact devnode appears — no polling, zero CPU while the device is absent.
///
/// On other platforms: falls back to a 5-second sleep retry.
async fn wait_for_device(path: &str) {
    if std::path::Path::new(path).exists() {
        // Device is present but connection failed — brief delay before retry.
        sleep(RECONNECT_DELAY).await;
        return;
    }

    #[cfg(target_os = "linux")]
    {
        info!("waiting for {path} to appear…");
        let path_owned = path.to_owned();
        let result = tokio::task::spawn_blocking(move || {
            wait_udev_tty(&path_owned)
        }).await;
        match result {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                warn!("udev wait failed ({e:#}), retrying in 5s");
                sleep(Duration::from_secs(5)).await;
            }
            Err(e) => {
                warn!("udev task panicked ({e}), retrying in 5s");
                sleep(Duration::from_secs(5)).await;
            }
        }
        return;
    }

    #[cfg(not(target_os = "linux"))]
    {
        info!("device absent — retrying in 5s");
        sleep(Duration::from_secs(5)).await;
    }
}

/// Blocking: watch udev for a `tty` ADD event matching `path`, then return.
///
/// The udev socket is non-blocking, so we use `poll(2)` to wait for readability
/// then drain events with `socket.iter()` (which returns `None` when empty).
#[cfg(target_os = "linux")]
fn wait_udev_tty(path: &str) -> Result<()> {
    use std::os::unix::io::AsRawFd;
    use udev::MonitorBuilder;

    let monitor = MonitorBuilder::new()?
        .match_subsystem("tty")?
        .listen()?;

    // Re-check after arming the monitor to close the TOCTOU window.
    if std::path::Path::new(path).exists() {
        return Ok(());
    }

    let fd = monitor.as_raw_fd();
    loop {
        // Block until the socket has data.
        let mut fds = [libc::pollfd { fd, events: libc::POLLIN, revents: 0 }];
        let ret = unsafe { libc::poll(fds.as_mut_ptr(), 1, -1) };
        if ret < 0 {
            let err = std::io::Error::last_os_error();
            if err.kind() == std::io::ErrorKind::Interrupted {
                continue; // EINTR — retry
            }
            return Err(err.into());
        }

        // Drain all pending events (non-blocking reads).
        for event in monitor.iter() {
            if event.event_type() == udev::EventType::Add {
                if let Some(node) = event.devnode() {
                    if node == std::path::Path::new(path) {
                        info!("{path} appeared");
                        // Brief settle: give the serial driver time to finish init.
                        std::thread::sleep(std::time::Duration::from_millis(100));
                        return Ok(());
                    }
                }
            }
        }
    }
}

// ── Background reconnect loop ─────────────────────────────────────────────────

/// Spawn the serial peer as a background task.  Returns a `SerialClient` handle.
pub fn spawn(serial_path: String) -> SerialClient {
    let client = SerialClient::new();
    let client_bg = client.clone();

    tokio::spawn(async move {
        loop {
            match run_once(&serial_path, &client_bg).await {
                Ok(_)  => info!("serial connection closed cleanly"),
                Err(e) => error!("serial connection error: {e:#}"),
            }
            client_bg.set_sender(None).await;
            wait_for_device(&serial_path).await;
        }
    });

    client
}
