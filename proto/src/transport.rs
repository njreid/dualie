use anyhow::Result;
use std::future::Future;
use crate::protocol::DualieMessage;

// ── Framing constants ─────────────────────────────────────────────────────────

/// Maximum encoded message size (4 MiB) — guards against runaway reads.
pub const MAX_FRAME_BYTES: u32 = 4 * 1024 * 1024;

// ── Transport trait ───────────────────────────────────────────────────────────

/// Bidirectional, ordered, reliable channel of `DualieMessage` values.
///
/// The blanket implementation for `TcpStream` (in `hub` and `daemon`) uses
/// CBOR encoding with a 4-byte little-endian length prefix per frame.
///
/// Using explicit associated `Future` types (Rust ≥ 1.75 async-fn-in-trait)
/// keeps this object-safe when boxed as `Box<dyn PeerTransport>`.
pub trait PeerTransport: Send {
    fn send(&mut self, msg: &DualieMessage) -> impl Future<Output = Result<()>> + Send;
    fn recv(&mut self)                   -> impl Future<Output = Result<DualieMessage>> + Send;
}

// ── Framing helpers (shared by hub and daemon implementations) ────────────────

/// Encode a `DualieMessage` as a length-prefixed CBOR frame.
pub fn encode_frame(msg: &DualieMessage) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    ciborium::into_writer(msg, &mut body)?;

    let len = body.len() as u32;
    anyhow::ensure!(
        len <= MAX_FRAME_BYTES,
        "message too large ({len} bytes)",
    );

    let mut frame = Vec::with_capacity(4 + body.len());
    frame.extend_from_slice(&len.to_le_bytes());
    frame.extend_from_slice(&body);
    Ok(frame)
}

/// Decode a `DualieMessage` from the body bytes of one frame (no length prefix).
pub fn decode_frame(body: &[u8]) -> Result<DualieMessage> {
    let msg: DualieMessage = ciborium::from_reader(body)?;
    Ok(msg)
}
