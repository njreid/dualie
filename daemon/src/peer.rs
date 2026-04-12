/// peer.rs — serial peer client.
///
/// Runs as a background task that owns the CDC-ACM serial connection to the
/// local RP2040.  Reconnects automatically when the device disappears or an
/// error occurs.
///
/// Responsibilities:
///   1. Open the CDC-ACM device and keep it open.
///   2. Dispatch inbound `DualieMessage` frames from the RP2040.
///   3. Expose a `SerialClient` handle so other modules can send messages.
///
/// On disconnect it waits `RECONNECT_DELAY` then retries indefinitely.

use anyhow::Result;
use dualie_proto::DualieMessage;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use tokio::time::{Duration, sleep};
use tracing::{error, info, warn};

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
        *self.inner.lock().await = tx;
    }

    #[allow(dead_code)]
    pub async fn send(&self, msg: DualieMessage) {
        if let Some(tx) = self.inner.lock().await.as_ref() {
            let _ = tx.try_send(msg);
        }
    }
}

// ── Inbound dispatch ──────────────────────────────────────────────────────────

#[allow(dead_code)]
async fn dispatch(msg: DualieMessage) {
    match msg {
        DualieMessage::Ping => {}

        DualieMessage::VirtualAction { slot } => {
            info!(slot, "virtual action from RP2040");
            // TODO: look up slot in config and execute action
        }

        DualieMessage::ActiveOutput { output } => {
            info!(output, "active output changed by RP2040");
            // TODO: notify intercept layer
        }

        DualieMessage::ClipboardPush(content) => {
            info!(len = content.text.len(), "clipboard received from RP2040");
            // TODO: write to OS clipboard via arboard
        }

        DualieMessage::Error { message } => {
            warn!("RP2040 error: {message}");
        }

        other => {
            warn!("unhandled message: {:?}", other);
        }
    }
}

// ── Single connection lifecycle ───────────────────────────────────────────────

async fn run_once(
    serial_path: &str,
    _client:     &SerialClient,
) -> Result<()> {
    // TODO (Phase 1.2/1.3): replace with SerialPeer::open(serial_path).
    // For now we just log that we would connect, keeping the reconnect loop
    // structure in place so it compiles and runs.
    info!(serial_path, "opening serial connection to RP2040");
    anyhow::bail!("SerialPeer not yet implemented — implement in Phase 1.2");
    // The code below will be restored once SerialPeer exists:
    #[allow(unreachable_code)]
    {
        let (tx, _rx) = mpsc::channel::<DualieMessage>(TX_QUEUE);
        _client.set_sender(Some(tx)).await;
        loop {
            // recv from peer → dispatch
            sleep(Duration::from_secs(60)).await;
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
            info!("reconnecting in {}s", RECONNECT_DELAY.as_secs());
            sleep(RECONNECT_DELAY).await;
        }
    });

    client
}
