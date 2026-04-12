/// Concrete `PeerTransport` backed by a `TcpStream`.
///
/// The stream is split into owned halves so the read and write sides can
/// be used independently (useful when the caller wraps them in separate tasks).
/// `TcpPeer` keeps both halves together for the simple request/response case.

use anyhow::{Context, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::protocol::DualieMessage;
use crate::transport::{MAX_FRAME_BYTES, decode_frame, encode_frame};

// ── TcpPeer ───────────────────────────────────────────────────────────────────

pub struct TcpPeer {
    stream: TcpStream,
}

impl TcpPeer {
    pub fn new(stream: TcpStream) -> Self {
        Self { stream }
    }

    /// Attempt a TCP connect (no retry — callers do their own retry loop).
    pub async fn connect(addr: &str) -> Result<Self> {
        let stream = TcpStream::connect(addr)
            .await
            .with_context(|| format!("connecting to hub at {addr}"))?;
        // Disable Nagle — we want low-latency framed messages, not batching.
        stream.set_nodelay(true)?;
        Ok(Self::new(stream))
    }

    pub async fn send(&mut self, msg: &DualieMessage) -> Result<()> {
        let frame = encode_frame(msg)?;
        self.stream.write_all(&frame).await?;
        Ok(())
    }

    pub async fn recv(&mut self) -> Result<DualieMessage> {
        let mut len_buf = [0u8; 4];
        self.stream
            .read_exact(&mut len_buf)
            .await
            .context("reading frame length")?;

        let len = u32::from_le_bytes(len_buf);
        anyhow::ensure!(len <= MAX_FRAME_BYTES, "incoming frame too large ({len} bytes)");

        let mut body = vec![0u8; len as usize];
        self.stream
            .read_exact(&mut body)
            .await
            .context("reading frame body")?;

        decode_frame(&body)
    }

    /// Split into owned read/write halves for concurrent use.
    pub fn into_split(self) -> (TcpPeerWriter, TcpPeerReader) {
        let (rd, wr) = self.stream.into_split();
        (TcpPeerWriter(wr), TcpPeerReader(rd))
    }
}

// ── Split halves ──────────────────────────────────────────────────────────────

pub struct TcpPeerWriter(tokio::net::tcp::OwnedWriteHalf);
pub struct TcpPeerReader(tokio::net::tcp::OwnedReadHalf);

impl TcpPeerWriter {
    pub async fn send(&mut self, msg: &DualieMessage) -> Result<()> {
        let frame = encode_frame(msg)?;
        self.0.write_all(&frame).await?;
        Ok(())
    }
}

impl TcpPeerReader {
    pub async fn recv(&mut self) -> Result<DualieMessage> {
        let mut len_buf = [0u8; 4];
        self.0
            .read_exact(&mut len_buf)
            .await
            .context("reading frame length")?;

        let len = u32::from_le_bytes(len_buf);
        anyhow::ensure!(len <= MAX_FRAME_BYTES, "incoming frame too large ({len} bytes)");

        let mut body = vec![0u8; len as usize];
        self.0
            .read_exact(&mut body)
            .await
            .context("reading frame body")?;

        decode_frame(&body)
    }
}
