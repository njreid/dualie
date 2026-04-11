/// peer.rs — daemon-side hub client.
///
/// Runs as a background task.  If no `--hub` address is given, the daemon
/// operates standalone (no clipboard or file sync).
///
/// When the hub is reachable this task:
///   1. Connects and performs the Hello/Welcome handshake.
///   2. Requests the current config and saves it on disk.
///   3. Exposes a `HubClient` handle so other modules can send messages.
///   4. Dispatches inbound hub messages (config push, clipboard, active output).
///
/// On disconnect it waits `RECONNECT_DELAY` then retries indefinitely.

use anyhow::Result;
use dualie_proto::{HubMessage, TcpPeer, TcpPeerWriter, PROTOCOL_VERSION};
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use tokio::time::{Duration, sleep};
use tracing::{error, info, warn};

const RECONNECT_DELAY: Duration = Duration::from_secs(5);
const TX_QUEUE: usize = 64;

// ── HubClient handle ──────────────────────────────────────────────────────────

/// Cloneable handle for sending messages to the hub.
///
/// Messages sent while disconnected are silently dropped.
#[derive(Clone)]
pub struct HubClient {
    /// Replaced with a live sender each time we (re)connect.
    inner: Arc<Mutex<Option<mpsc::Sender<HubMessage>>>>,
}

impl HubClient {
    fn new() -> Self {
        Self { inner: Arc::new(Mutex::new(None)) }
    }

    async fn set_sender(&self, tx: Option<mpsc::Sender<HubMessage>>) {
        *self.inner.lock().await = tx;
    }

    #[allow(dead_code)]
    pub async fn send(&self, msg: HubMessage) {
        if let Some(tx) = self.inner.lock().await.as_ref() {
            let _ = tx.try_send(msg);
        }
    }
}

// ── TX writer task ────────────────────────────────────────────────────────────

async fn tx_task(mut writer: TcpPeerWriter, mut rx: mpsc::Receiver<HubMessage>) {
    while let Some(msg) = rx.recv().await {
        if let Err(e) = writer.send(&msg).await {
            info!("hub tx task ending: {e}");
            break;
        }
    }
}

// ── Inbound dispatch ──────────────────────────────────────────────────────────

async fn dispatch(msg: HubMessage, _output_slot: u8) {
    match msg {
        HubMessage::Ping => {}  // hub pings us — Pong is sent via self_tx below

        HubMessage::ActiveOutput { output } => {
            info!(output, "active output changed by hub");
            // TODO: notify intercept layer so it knows if it's the active machine
        }

        HubMessage::ClipboardPush(content) => {
            info!(len = content.text.len(), "clipboard received from hub");
            // TODO: write to OS clipboard via arboard
        }

        HubMessage::ConfigPush { cbor } => {
            info!(bytes = cbor.len(), "config push from hub");
            if let Err(e) = save_config_cbor(&cbor) {
                warn!("failed to save hub config: {e:#}");
            }
        }

        HubMessage::Error { message } => {
            warn!("hub error: {message}");
        }

        other => {
            warn!("unhandled hub message: {:?}", other);
        }
    }
}

fn save_config_cbor(cbor: &[u8]) -> Result<()> {
    let cfg: crate::config::DualieConfig = ciborium::from_reader(cbor)?;
    cfg.save()?;
    info!("config updated from hub");
    Ok(())
}

// ── Single connection lifecycle ───────────────────────────────────────────────

async fn run_once(
    hub_addr:   &str,
    machine_id: &str,
    client:     &HubClient,
) -> Result<()> {
    let mut peer = TcpPeer::connect(hub_addr).await?;
    info!(hub_addr, "connected to hub");

    peer.send(&HubMessage::Hello {
        machine_id:       machine_id.to_owned(),
        protocol_version: PROTOCOL_VERSION,
    }).await?;

    let (output_slot, active_output) = match peer.recv().await? {
        HubMessage::Welcome { output_slot, active_output } => (output_slot, active_output),
        HubMessage::Error { message } => anyhow::bail!("hub rejected us: {message}"),
        other => anyhow::bail!("expected Welcome, got {:?}", other),
    };
    info!(output_slot, active_output, "hub handshake complete");

    // Arm the outbound channel so HubClient::send works while connected.
    let (tx, rx) = mpsc::channel::<HubMessage>(TX_QUEUE);
    client.set_sender(Some(tx.clone())).await;

    // Ask for the current config.
    let _ = tx.try_send(HubMessage::ConfigRequest);

    let (writer, mut reader) = peer.into_split();
    tokio::spawn(tx_task(writer, rx));

    // Read loop — exits when the connection drops.
    loop {
        let msg = reader.recv().await?;
        // Handle Ping inline so we don't need to pass tx into dispatch.
        if matches!(msg, HubMessage::Ping) {
            let _ = tx.try_send(HubMessage::Pong);
            continue;
        }
        dispatch(msg, output_slot).await;
    }
}

// ── Background reconnect loop ─────────────────────────────────────────────────

/// Spawn the hub client as a background task.  Returns a `HubClient` handle.
pub fn spawn(hub_addr: String, machine_id: String) -> HubClient {
    let client = HubClient::new();
    let client_bg = client.clone();

    tokio::spawn(async move {
        loop {
            match run_once(&hub_addr, &machine_id, &client_bg).await {
                Ok(_)  => info!("hub connection closed cleanly"),
                Err(e) => error!("hub connection error: {e:#}"),
            }
            client_bg.set_sender(None).await;
            info!("reconnecting in {}s", RECONNECT_DELAY.as_secs());
            sleep(RECONNECT_DELAY).await;
        }
    });

    client
}
