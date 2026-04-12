/// peer.rs — TCP listener that accepts daemon connections.
///
/// NOTE: The hub design is archived.  This code compiles but is not actively
/// developed.  The Hello/Welcome handshake variants have been removed from
/// `DualieMessage`; the hub now uses a simplified connection model where
/// slots are assigned at connect time without a protocol handshake.
///
/// Each daemon runs two concurrent tasks:
///   • `rx_task` – reads incoming `DualieMessage`s and dispatches them
///   • `tx_task` – drains the per-daemon `mpsc` channel and writes to the socket

use anyhow::Result;
use dualie_proto::{DualieMessage, TcpPeer, PROTOCOL_VERSION};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tracing::{error, info, instrument, warn};

use crate::state::{DaemonTx, SharedState};

// ── Channel depth ─────────────────────────────────────────────────────────────

const TX_QUEUE: usize = 64;

// ── TX task ───────────────────────────────────────────────────────────────────

async fn tx_task(mut writer: dualie_proto::TcpPeerWriter, mut rx: mpsc::Receiver<DualieMessage>) {
    while let Some(msg) = rx.recv().await {
        if let Err(e) = writer.send(&msg).await {
            info!("tx task ending: {e}");
            break;
        }
    }
}

// ── RX dispatch ───────────────────────────────────────────────────────────────

async fn rx_task(
    mut reader: dualie_proto::TcpPeerReader,
    slot:       u8,
    self_tx:    DaemonTx,
    state:      SharedState,
) {
    loop {
        let msg = match reader.recv().await {
            Ok(m)  => m,
            Err(e) => {
                info!(slot, "connection closed: {e}");
                break;
            }
        };

        if let Err(e) = dispatch(msg, slot, &self_tx, &state).await {
            warn!(slot, "dispatch error: {e:#}");
        }
    }

    state.lock().await.release_slot(slot);
    info!(slot, "daemon disconnected, slot released");
}

async fn dispatch(
    msg:     DualieMessage,
    slot:    u8,
    self_tx: &DaemonTx,
    state:   &SharedState,
) -> Result<()> {
    match msg {
        DualieMessage::Ping => {
            let _ = self_tx.try_send(DualieMessage::Pong);
        }

        DualieMessage::VirtualAction { slot: action_slot } => {
            info!(slot, action_slot, "virtual action");
        }

        DualieMessage::ActiveOutput { output } => {
            let mut st = state.lock().await;
            st.active_output = output;
            let broadcast = DualieMessage::ActiveOutput { output };
            st.broadcast(&broadcast).await;
            info!(slot, output, "active output changed");
        }

        DualieMessage::ClipboardPush(content) => {
            info!(slot, len = content.text.len(), "clipboard push");
            let mut st = state.lock().await;
            st.clipboard = Some(content.text.clone());
            if let Some(tx) = st.other_tx(slot) {
                let _ = tx.try_send(DualieMessage::ClipboardPush(content));
            }
        }

        DualieMessage::ClipboardPull => {
            let clipboard = state.lock().await.clipboard.clone();
            match clipboard {
                Some(text) => {
                    let _ = self_tx.try_send(DualieMessage::ClipboardPush(
                        dualie_proto::ClipboardText { text },
                    ));
                }
                None => {
                    let _ = self_tx.try_send(DualieMessage::Error {
                        message: "no clipboard content available".into(),
                    });
                }
            }
        }

        DualieMessage::SyncList { files } => {
            info!(slot, count = files.len(), "sync list received");
        }

        DualieMessage::SyncChunk(chunk) => {
            info!(slot, path = %chunk.rel_path, offset = chunk.offset, "sync chunk");
        }

        DualieMessage::ConfigRequest => {
            let cbor = state.lock().await.config_cbor.clone();
            if cbor.is_empty() {
                let _ = self_tx.try_send(DualieMessage::Error {
                    message: "hub has no config loaded yet".into(),
                });
            } else {
                let _ = self_tx.try_send(DualieMessage::ConfigPush { cbor });
            }
        }

        DualieMessage::Error { message } => {
            warn!(slot, "daemon error: {message}");
        }

        other => {
            warn!(slot, "unexpected message: {:?}", other);
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
    let (tx, rx) = mpsc::channel::<DualieMessage>(TX_QUEUE);
    let slot = {
        let mut st = state.lock().await;
        match st.assign_slot(tx.clone()) {
            Some(s) => s,
            None => {
                // No Hello/Welcome variants any more — just drop the connection.
                anyhow::bail!("no slot available for {addr}");
            }
        }
    };

    info!(addr = %addr, slot, "daemon connected");

    let (writer, reader) = peer.into_split();
    tokio::spawn(tx_task(writer, rx));
    rx_task(reader, slot, tx, state).await;

    Ok(())
}

// ── Server entry point ────────────────────────────────────────────────────────

pub async fn run_peer_server(bind_addr: &str, state: SharedState) -> Result<()> {
    // Silence unused import warning — PROTOCOL_VERSION kept for future use.
    let _ = PROTOCOL_VERSION;

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
