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
    /// Last-modified epoch-ms of the source file (sender's mtime).
    /// Used by the receiver for LWW conflict resolution.
    pub modified_ms: u64,
}

// ── RP2040 ↔ Daemon message ───────────────────────────────────────────────────

/// All messages exchanged between the RP2040 firmware and the daemon over the
/// CDC-ACM serial channel.
///
/// Framing: COBS-encoded, `0x00`-delimited.  Each message body is CBOR.
/// The `serial` module handles framing; protocol code only sees
/// `DualieMessage` values.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DualieMessage {
    // ── Keepalive ────────────────────────────────────────────────────────────

    /// Application-level keepalive.
    Ping,
    Pong,

    // ── Firmware version ─────────────────────────────────────────────────────

    /// RP2040 → Daemon: sent once when the daemon raises DTR on the serial port.
    /// The daemon compares `version` against `FIRMWARE_MIN_COMPATIBLE` and
    /// logs a warning (or prompts to run `just flash`) if the firmware is older.
    FirmwareInfo {
        version: u32,
    },

    // ── KVM switching ────────────────────────────────────────────────────────

    /// RP2040 → Daemon: a caps-layer virtual action was triggered.
    /// The daemon looks up `slot` in its config and executes the action.
    VirtualAction {
        slot: u8,
    },

    /// Daemon → RP2040 (or RP2040 → Daemon): active output changed.
    ActiveOutput {
        output: u8,
    },

    // ── Clipboard ────────────────────────────────────────────────────────────

    /// Either direction: push text clipboard content.
    ClipboardPush(ClipboardText),

    /// Daemon → RP2040: request clipboard from the other machine.
    ClipboardPull,

    // ── File sync ────────────────────────────────────────────────────────────

    /// Either direction: announce local file inventory.
    SyncList {
        files: Vec<SyncEntry>,
    },

    /// Either direction: transfer a chunk of a file.
    SyncChunk(FileChunk),

    /// Acknowledge receipt of the final chunk.
    SyncAck {
        rel_path: String,
    },

    // ── Config ───────────────────────────────────────────────────────────────

    /// Daemon → RP2040: request the current config.
    ConfigRequest,

    /// RP2040 → Daemon: deliver config as a CBOR blob.
    ConfigPush {
        cbor: Vec<u8>,
    },

    // ── Firmware management ──────────────────────────────────────────────────

    /// Daemon → RP2040: reboot into USB MSC bootloader (RPI-RP2 drive).
    /// The RP2040 calls `reset_usb_boot(0, 0)` from the bootrom.
    RebootToBootloader,

    // ── Errors ───────────────────────────────────────────────────────────────

    /// Either direction: report a non-fatal error.
    Error {
        message: String,
    },
}

/// Current protocol version — both sides must agree.
pub const PROTOCOL_VERSION: u32 = 1;
