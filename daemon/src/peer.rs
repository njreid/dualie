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

/// Minimum firmware version the daemon considers compatible.
/// If the RP2040 reports a lower version, the daemon logs a warning and
/// suggests running `just flash` to upgrade.
pub const FIRMWARE_MIN_COMPATIBLE: u32 = 1;

/// Global connection state — set true while the serial peer is connected.
/// Read by the status server without going through a channel.
pub static CONNECTED: AtomicBool = AtomicBool::new(false);

const RECONNECT_DELAY: Duration = Duration::from_secs(5);
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

async fn dispatch(msg: DualieMessage) {
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
            // TODO (Phase 3): look up slot in config and execute action
        }

        DualieMessage::ActiveOutput { output } => {
            info!(output, "active output changed by RP2040");
            // TODO (Phase 3): notify intercept layer via SerialClient
        }

        DualieMessage::ClipboardPush(content) => {
            info!(len = content.text.len(), "clipboard received from RP2040");
            // TODO (Phase 6): write to OS clipboard via arboard
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
        dispatch(msg).await;
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
            info!("reconnecting in {}s …", RECONNECT_DELAY.as_secs());
            sleep(RECONNECT_DELAY).await;
        }
    });

    client
}
