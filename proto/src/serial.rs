/// serial.rs — CDC-ACM serial transport for the RP2040 ↔ daemon link.
///
/// # Framing
///
/// Each `DualieMessage` is:
///   1. Serialised to CBOR
///   2. COBS-encoded (eliminates `0x00` bytes from the payload)
///   3. Written to the serial device followed by a `0x00` delimiter byte
///
/// On receipt, bytes are accumulated until a `0x00` is seen, then
/// COBS-decoded and CBOR-deserialised.
///
/// # Auto-detection
///
/// `SerialPeer::detect()` finds the first RP2040 CDC-ACM device:
///   - Linux: first `/dev/ttyACM*` owned by USB VID 0x2E8A (Raspberry Pi)
///   - macOS: first `/dev/tty.usbmodem*`

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use tokio::io::{AsyncReadExt, AsyncWriteExt, ReadHalf, WriteHalf};
use tokio_serial::SerialStream;
use tracing::debug;

use crate::protocol::DualieMessage;

// ── Constants ─────────────────────────────────────────────────────────────────

/// USB Vendor ID for Raspberry Pi / RP2040.
pub const RP2040_VID: u16 = 0x2E8A;

/// CDC-ACM baud rate setting.  For USB virtual serial this value is ignored
/// by the device, but the host driver requires a non-zero value.
const BAUD_RATE: u32 = 115_200;

/// `0x00` is the COBS frame delimiter.
const FRAME_DELIM: u8 = 0x00;

// ── Split halves ──────────────────────────────────────────────────────────────

/// Write-only half of a `SerialPeer`.
pub struct SerialPeerWriter(WriteHalf<SerialStream>);

/// Read-only half of a `SerialPeer`.
pub struct SerialPeerReader(ReadHalf<SerialStream>);

impl SerialPeerWriter {
    pub async fn send(&mut self, msg: &DualieMessage) -> Result<()> {
        let mut cbor = Vec::new();
        ciborium::into_writer(msg, &mut cbor).context("CBOR serialise")?;
        let encoded = cobs::encode_vec(&cbor);
        self.0.write_all(&encoded).await.context("serial write")?;
        self.0.write_all(&[FRAME_DELIM]).await.context("serial write delimiter")?;
        debug!(msg = ?msg, "serial → RP2040");
        Ok(())
    }
}

impl SerialPeerReader {
    pub async fn recv(&mut self) -> Result<DualieMessage> {
        let mut frame_bytes: Vec<u8> = Vec::with_capacity(64);
        loop {
            let b = self.0.read_u8().await.context("serial read")?;
            if b == FRAME_DELIM {
                break;
            }
            frame_bytes.push(b);
        }
        let cbor = cobs::decode_vec(&frame_bytes)
            .map_err(|_| anyhow::anyhow!("COBS decode failed ({} bytes)", frame_bytes.len()))?;
        let msg: DualieMessage = ciborium::from_reader(cbor.as_slice())
            .context("CBOR deserialise")?;
        debug!(msg = ?msg, "serial ← RP2040");
        Ok(msg)
    }
}

// ── SerialPeer ────────────────────────────────────────────────────────────────

/// Framed, COBS-encoded serial peer wrapping a CDC-ACM device.
pub struct SerialPeer {
    stream: SerialStream,
}

impl SerialPeer {
    /// Open a specific CDC-ACM device path.
    pub fn open(path: &Path) -> Result<Self> {
        let builder = tokio_serial::new(path.to_string_lossy(), BAUD_RATE);
        let stream = SerialStream::open(&builder)
            .with_context(|| format!("opening serial device {}", path.display()))?;
        Ok(Self { stream })
    }

    /// Find and open the first RP2040 CDC-ACM device.
    ///
    /// Searches platform-specific device paths; returns an error if none found.
    pub fn detect() -> Result<Self> {
        let path = detect_path().context("no RP2040 serial device found")?;
        Self::open(&path)
    }

    /// Open a device at `path`, or auto-detect if `path` is `None`.
    pub fn open_or_detect(path: Option<&Path>) -> Result<Self> {
        match path {
            Some(p) => Self::open(p),
            None    => Self::detect(),
        }
    }

    /// Split into independent read/write halves for concurrent use in separate tasks.
    pub fn into_split(self) -> (SerialPeerWriter, SerialPeerReader) {
        let (rd, wr) = tokio::io::split(self.stream);
        (SerialPeerWriter(wr), SerialPeerReader(rd))
    }
}

// ── Auto-detection ────────────────────────────────────────────────────────────

/// Return the path of the first RP2040 CDC-ACM device, or `None`.
pub fn detect_path() -> Option<PathBuf> {
    #[cfg(target_os = "linux")]
    return detect_path_linux();

    #[cfg(target_os = "macos")]
    return detect_path_macos();

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    None
}

#[cfg(target_os = "linux")]
fn detect_path_linux() -> Option<PathBuf> {
    // Walk udev to find a ttyACM* device with the RP2040 VID.
    let mut enumerator = udev::Enumerator::new().ok()?;
    enumerator.match_subsystem("tty").ok()?;
    enumerator.match_property("ID_USB_VENDOR_ID",
        &format!("{:04x}", RP2040_VID)).ok()?;

    enumerator.scan_devices().ok()?
        .next()
        .and_then(|dev| dev.devnode().map(|p| p.to_path_buf()))
}

#[cfg(target_os = "macos")]
fn detect_path_macos() -> Option<PathBuf> {
    // On macOS RP2040 CDC-ACM devices appear as /dev/tty.usbmodem*.
    let entries = std::fs::read_dir("/dev").ok()?;
    let mut paths: Vec<PathBuf> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name();
            let s = name.to_string_lossy();
            if s.starts_with("tty.usbmodem") {
                Some(e.path())
            } else {
                None
            }
        })
        .collect();
    paths.sort();
    paths.into_iter().next()
}
