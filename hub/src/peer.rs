/// peer.rs — TCP listener that accepts daemon connections.
///
/// Each daemon runs two concurrent tasks:
///   • `rx_task` – reads incoming `HubMessage`s and dispatches them
///   • `tx_task` – drains the per-daemon `mpsc` channel and writes to the socket
///
/// This separation means a slow-sending daemon can never block message receipt.

use anyhow::Result;
use dualie_proto::{HubMessage, TcpPeer, PROTOCOL_VERSION};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tracing::{error, info, instrument, warn};

use crate::state::{DaemonTx, SharedState};

// ── Channel depth ─────────────────────────────────────────────────────────────

/// Outbound queue depth per daemon.  Frames beyond this are dropped.
const TX_QUEUE: usize = 64;

// ── TX task ───────────────────────────────────────────────────────────────────

/// Drains the per-daemon mpsc channel and writes frames to the socket.
async fn tx_task(mut writer: dualie_proto::TcpPeerWriter, mut rx: mpsc::Receiver<HubMessage>) {
    while let Some(msg) = rx.recv().await {
        if let Err(e) = writer.send(&msg).await {
            info!("tx task ending: {e}");
            break;
        }
    }
}

// ── RX dispatch ───────────────────────────────────────────────────────────────

async fn rx_task(
    mut reader:   dualie_proto::TcpPeerReader,
    slot:         u8,
    machine_id:   String,
    self_tx:      DaemonTx,
    state:        SharedState,
) {
    loop {
        let msg = match reader.recv().await {
            Ok(m)  => m,
            Err(e) => {
                info!(machine_id, "connection closed: {e}");
                break;
            }
        };

        if let Err(e) = dispatch(msg, slot, &machine_id, &self_tx, &state).await {
            warn!(machine_id, "dispatch error: {e:#}");
        }
    }

    // Clean up slot on disconnect.
    state.lock().await.release_slot(slot);
    info!(machine_id, slot, "daemon disconnected, slot released");
}

async fn dispatch(
    msg:        HubMessage,
    slot:       u8,
    machine_id: &str,
    self_tx:    &DaemonTx,
    state:      &SharedState,
) -> Result<()> {
    match msg {
        HubMessage::Ping => {
            let _ = self_tx.try_send(HubMessage::Pong);
        }

        HubMessage::VirtualAction { slot: action_slot } => {
            info!(machine_id, action_slot, "virtual action");
            // TODO: interpret action_slot against config to decide what to do
            // (switch output, relay clipboard, etc.)
        }

        HubMessage::ActiveOutput { output } => {
            // Daemon is requesting a switch (e.g. from a caps-layer jump key).
            let mut st = state.lock().await;
            st.active_output = output;
            let broadcast = HubMessage::ActiveOutput { output };
            // Notify all daemons (including the requester so it can update its UI).
            st.broadcast(&broadcast).await;
            info!(machine_id, output, "active output changed");
        }

        HubMessage::ClipboardPush(content) => {
            info!(machine_id, len = content.text.len(), "clipboard push");
            let mut st = state.lock().await;
            st.clipboard = Some(content.text.clone());
            // Relay to the other daemon.
            if let Some(tx) = st.other_tx(slot) {
                let _ = tx.try_send(HubMessage::ClipboardPush(content));
            }
        }

        HubMessage::ClipboardPull => {
            let clipboard = state.lock().await.clipboard.clone();
            match clipboard {
                Some(text) => {
                    let _ = self_tx.try_send(HubMessage::ClipboardPush(
                        dualie_proto::ClipboardText { text },
                    ));
                }
                None => {
                    let _ = self_tx.try_send(HubMessage::Error {
                        message: "no clipboard content available".into(),
                    });
                }
            }
        }

        HubMessage::SyncList { files } => {
            info!(machine_id, count = files.len(), "sync list received");
            // TODO: reconcile with other machine's file list and send decisions
        }

        HubMessage::SyncChunk(chunk) => {
            info!(machine_id, path = %chunk.rel_path, offset = chunk.offset, "sync chunk");
            // TODO: write chunk to staging area; send SyncAck on final chunk
        }

        HubMessage::ConfigRequest => {
            let cbor = state.lock().await.config_cbor.clone();
            if cbor.is_empty() {
                let _ = self_tx.try_send(HubMessage::Error {
                    message: "hub has no config loaded yet".into(),
                });
            } else {
                let _ = self_tx.try_send(HubMessage::ConfigPush { cbor });
            }
        }

        HubMessage::Error { message } => {
            warn!(machine_id, "daemon error: {message}");
        }

        other => {
            warn!(machine_id, "unexpected message from daemon: {:?}", other);
        }
    }
    Ok(())
}

// ── Connection setup ──────────────────────────────────────────────────────────

#[instrument(skip(peer, state), fields(peer = %addr))]
async fn handle_connection(
    peer:  TcpPeer,
    addr:  std::net::SocketAddr,
    state: SharedState,
) -> Result<()> {
    let mut peer = peer;

    // Expect Hello as the very first message.
    let (machine_id, proto_ver) = match peer.recv().await? {
        HubMessage::Hello { machine_id, protocol_version } => (machine_id, protocol_version),
        other => anyhow::bail!("expected Hello, got {:?}", other),
    };

    if proto_ver != PROTOCOL_VERSION {
        peer.send(&HubMessage::Error {
            message: format!(
                "protocol version mismatch: hub={PROTOCOL_VERSION}, daemon={proto_ver}"
            ),
        }).await?;
        anyhow::bail!("version mismatch from {machine_id}");
    }

    // Assign output slot.
    let (tx, rx) = mpsc::channel::<HubMessage>(TX_QUEUE);
    let (active_output, slot) = {
        let mut st = state.lock().await;
        match st.assign_slot(tx.clone()) {
            Some(s) => (st.active_output, s),
            None => {
                peer.send(&HubMessage::Error {
                    message: "hub already has two daemons connected".into(),
                }).await?;
                anyhow::bail!("no slot available for {machine_id}");
            }
        }
    };

    info!(machine_id, slot, "handshake ok");

    peer.send(&HubMessage::Welcome { output_slot: slot, active_output }).await?;

    // Push config if we have one.
    let cbor = state.lock().await.config_cbor.clone();
    if !cbor.is_empty() {
        let _ = tx.try_send(HubMessage::ConfigPush { cbor });
    }

    // Split and run read/write concurrently.
    let (writer, reader) = peer.into_split();
    tokio::spawn(tx_task(writer, rx));
    rx_task(reader, slot, machine_id, tx, state).await;

    Ok(())
}

// ── Server entry point ────────────────────────────────────────────────────────

pub async fn run_peer_server(bind_addr: &str, state: SharedState) -> Result<()> {
    let listener = TcpListener::bind(bind_addr).await?;
    info!("listening for daemons on {bind_addr}");

    loop {
        let (stream, addr) = listener.accept().await?;
        let state = state.clone();
        tokio::spawn(async move {
            let peer = TcpPeer::new(stream);
            if let Err(e) = handle_connection(peer, addr, state).await {
                error!("connection error from {addr}: {e:#}");
            }
        });
    }
}
