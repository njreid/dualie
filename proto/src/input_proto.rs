/// input_proto.rs — Protocol between the user daemon and the root input daemon.
///
/// Wire format: 4-byte LE length prefix + CBOR body, same as the peer transport.
///
/// Flow:
///   user daemon → input daemon:  ConfigSnapshot, SetActiveOutput
///   input daemon → user daemon:  SwitchOutput, FireAction, ClipPull

use serde::{Deserialize, Serialize};

/// Path of the Unix socket served by the root input daemon.
pub const INPUT_SOCKET: &str = "/var/run/dualie-input.sock";

/// Messages sent from the user daemon to the root input daemon.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ToInput {
    /// Full serialised config pushed on startup and on every hot-reload.
    ConfigSnapshot(Vec<u8>),  // CBOR-encoded DualieConfig
    /// Notify the input daemon that the active output has changed
    /// (e.g. because the serial peer received an ActiveOutput message).
    SetActiveOutput(u8),
}

/// Messages sent from the root input daemon to the user daemon.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FromInput {
    /// User pressed a caps-layer binding that swaps/jumps outputs.
    SwitchOutput(u8),
    /// User pressed a caps-layer binding that fires a virtual action slot.
    FireAction(u8),
    /// User pressed a caps-layer binding that requests a clipboard pull.
    ClipPull,
}

// ── Framing (identical to transport.rs but for our own enums) ─────────────────

const MAX_BYTES: u32 = 256 * 1024;

pub fn encode_to_input(msg: &ToInput) -> anyhow::Result<Vec<u8>> {
    encode(msg)
}

pub fn encode_from_input(msg: &FromInput) -> anyhow::Result<Vec<u8>> {
    encode(msg)
}

fn encode<T: Serialize>(msg: &T) -> anyhow::Result<Vec<u8>> {
    let mut body = Vec::new();
    ciborium::into_writer(msg, &mut body)?;
    anyhow::ensure!(body.len() as u32 <= MAX_BYTES, "message too large");
    let mut frame = Vec::with_capacity(4 + body.len());
    frame.extend_from_slice(&(body.len() as u32).to_le_bytes());
    frame.extend_from_slice(&body);
    Ok(frame)
}

pub fn decode_to_input(body: &[u8]) -> anyhow::Result<ToInput> {
    Ok(ciborium::from_reader(body)?)
}

pub fn decode_from_input(body: &[u8]) -> anyhow::Result<FromInput> {
    Ok(ciborium::from_reader(body)?)
}

/// Read one length-prefixed frame from a sync `Read`.
pub fn read_frame<R: std::io::Read>(r: &mut R) -> anyhow::Result<Vec<u8>> {
    let mut len_buf = [0u8; 4];
    r.read_exact(&mut len_buf)?;
    let len = u32::from_le_bytes(len_buf);
    anyhow::ensure!(len <= MAX_BYTES, "frame too large: {len}");
    let mut body = vec![0u8; len as usize];
    r.read_exact(&mut body)?;
    Ok(body)
}

/// Write one length-prefixed frame to a sync `Write`.
pub fn write_frame<W: std::io::Write>(w: &mut W, frame: &[u8]) -> anyhow::Result<()> {
    w.write_all(frame)?;
    w.flush()?;
    Ok(())
}
