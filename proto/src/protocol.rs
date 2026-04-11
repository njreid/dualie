use serde::{Deserialize, Serialize};

// ── Clipboard content ─────────────────────────────────────────────────────────

/// Text-only clipboard payload (image sync is a future milestone).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClipboardText {
    pub text: String,
}

// ── File-sync primitives ──────────────────────────────────────────────────────

/// Metadata for one watched file sent during a sync negotiation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncEntry {
    /// Path relative to the sync root (UTF-8, forward-slash separated).
    pub rel_path:    String,
    /// Last-modified epoch-ms (used for last-writer-wins resolution).
    pub modified_ms: u64,
    /// SHA-256 of the current file content.
    pub sha256:      [u8; 32],
    /// Byte length of the file.
    pub size:        u64,
}

/// One block of file data — transfers may be split into multiple chunks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileChunk {
    pub rel_path:    String,
    /// Absolute byte offset of this chunk within the final file.
    pub offset:      u64,
    pub data:        Vec<u8>,
    /// When `offset + data.len() == total_size` this is the final chunk.
    pub total_size:  u64,
}

// ── Hub↔Daemon message ────────────────────────────────────────────────────────

/// All messages that flow over the TCP link between hub and daemon.
///
/// Framing: each message is serialised to CBOR and prefixed by a
/// little-endian u32 byte-length.  The transport layer handles framing;
/// protocol code only sees `HubMessage` values.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HubMessage {
    // ── Session ──────────────────────────────────────────────────────────────

    /// First message sent by the daemon after TCP connect.
    Hello {
        /// Opaque stable identifier for the machine (e.g. hostname or UUID).
        machine_id: String,
        /// Protocol version — reject mismatches early.
        protocol_version: u32,
    },

    /// Hub reply to `Hello`; carries initial state.
    Welcome {
        /// Which output slot the hub has assigned this daemon (0 = A, 1 = B).
        output_slot: u8,
        /// Currently active output slot across the whole switch.
        active_output: u8,
    },

    /// Application-level keepalive.
    Ping,
    Pong,

    // ── KVM switching ────────────────────────────────────────────────────────

    /// Daemon → Hub: user triggered a virtual action (vkey slot fired).
    /// Hub interprets the action (switch output, relay clipboard, etc.).
    VirtualAction {
        slot: u8,
    },

    /// Hub → all daemons: the active output has changed.
    ActiveOutput {
        output: u8,
    },

    // ── Clipboard ────────────────────────────────────────────────────────────

    /// Either direction: push text clipboard content.
    ClipboardPush(ClipboardText),

    /// Daemon → Hub: request the clipboard from the other machine.
    ClipboardPull,

    // ── File sync ────────────────────────────────────────────────────────────

    /// Daemon → Hub (or Hub → Daemon): announce local file inventory.
    SyncList {
        files: Vec<SyncEntry>,
    },

    /// Either direction: transfer a chunk of a file.
    SyncChunk(FileChunk),

    /// Hub → Daemon: acknowledge receipt of the final chunk.
    SyncAck {
        rel_path: String,
    },

    // ── Config ───────────────────────────────────────────────────────────────

    /// Daemon → Hub: request the current config.
    ConfigRequest,

    /// Hub → Daemon: deliver config as a CBOR blob (avoids `proto` depending
    /// on the daemon's `DualieConfig` type).
    ConfigPush {
        cbor: Vec<u8>,
    },

    // ── Errors ───────────────────────────────────────────────────────────────

    /// Either direction: report a non-fatal error.
    Error {
        message: String,
    },
}

/// Current protocol version — both sides must agree.
pub const PROTOCOL_VERSION: u32 = 1;
