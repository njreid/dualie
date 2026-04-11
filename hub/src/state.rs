/// Shared runtime state for the hub, protected by a `Mutex`.
///
/// Each connected daemon gets a clone of `Arc<Mutex<HubState>>` so it can
/// read the active output, update the clipboard, etc.  Outbound messages to
/// peer daemons are sent via per-daemon `mpsc` channels stored here.

use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use dualie_proto::HubMessage;

// ── Per-daemon relay handle ───────────────────────────────────────────────────

/// A send handle that lets any task push a message to a specific daemon.
pub type DaemonTx = mpsc::Sender<HubMessage>;

// ── Slot assignment ───────────────────────────────────────────────────────────

pub const OUTPUT_A: u8 = 0;
pub const OUTPUT_B: u8 = 1;

// ── Hub state ─────────────────────────────────────────────────────────────────

#[derive(Default)]
pub struct HubState {
    /// Currently active output (0 = A, 1 = B).
    pub active_output: u8,

    /// Outbound channel to the daemon on output A (None if not connected).
    pub daemon_a: Option<DaemonTx>,
    /// Outbound channel to the daemon on output B (None if not connected).
    pub daemon_b: Option<DaemonTx>,

    /// Most-recently-pushed clipboard text from either machine.
    pub clipboard: Option<String>,

    /// Raw CBOR of the authoritative `DualieConfig`.
    /// Loaded from disk on startup; pushed to daemons on connect and on save.
    pub config_cbor: Vec<u8>,
}

pub type SharedState = Arc<Mutex<HubState>>;

impl HubState {
    /// Return the `DaemonTx` for the slot that is *not* `slot`.
    pub fn other_tx(&self, slot: u8) -> Option<&DaemonTx> {
        match slot {
            OUTPUT_A => self.daemon_b.as_ref(),
            OUTPUT_B => self.daemon_a.as_ref(),
            _        => None,
        }
    }

    /// Register a new daemon connection; returns the slot assigned.
    /// Assigns A first, then B; rejects a third connection.
    pub fn assign_slot(&mut self, tx: DaemonTx) -> Option<u8> {
        if self.daemon_a.is_none() {
            self.daemon_a = Some(tx);
            Some(OUTPUT_A)
        } else if self.daemon_b.is_none() {
            self.daemon_b = Some(tx);
            Some(OUTPUT_B)
        } else {
            None
        }
    }

    /// Unregister a daemon when its connection drops.
    pub fn release_slot(&mut self, slot: u8) {
        match slot {
            OUTPUT_A => self.daemon_a = None,
            OUTPUT_B => self.daemon_b = None,
            _        => {}
        }
    }

    /// Broadcast a message to all connected daemons.
    pub async fn broadcast(&self, msg: &HubMessage) {
        for tx in [&self.daemon_a, &self.daemon_b].into_iter().flatten() {
            // Best-effort; a full channel means the daemon is slow — drop the frame.
            let _ = tx.try_send(msg.clone());
        }
    }
}
