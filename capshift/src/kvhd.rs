/// kvhd.rs — Karabiner-VirtualHIDDevice client.
///
/// # Why this isn't IOKit anymore
///
/// The previous version of this file (ported from `daemon/src/intercept/macos_kvhd.rs`)
/// opened the DriverKit user client directly via IOKit
/// (`IOServiceMatching`/`IOServiceOpen`/`IOConnectCallStructMethod`). That
/// only works for apps signed with pqrs.org's own code-signing identity —
/// per the driver's own docs, unsigned third-party apps like capshift are
/// expected to go through `Karabiner-VirtualHIDDevice-Daemon` instead, over
/// a UNIX domain stream socket. Direct IOKit connections from capshift will
/// never find the service, no matter how the driver is installed/activated.
///
/// # Protocol (reverse-engineered — not an official public spec)
///
/// Pieced together from the pqrs-org C++ source, not from documented wire
/// format. This is a best-effort port; treat it as unverified until
/// confirmed against a live daemon on real hardware.
///
///   - github.com/pqrs-org/Karabiner-DriverKit-VirtualHIDDevice —
///     `virtual_hid_device_service/{request,response,parameters,constants}.hpp`,
///     `virtual_hid_device_driver/hid_report/{keyboard_input,modifier,modifiers,keys}.hpp`
///   - github.com/pqrs-org/cpp-unix_domain_stream —
///     `impl/protocol.hpp` (the outer frame format)
///   - github.com/pqrs-org/cpp-hid —
///     `vendor_id.hpp`/`product_id.hpp`/`country_code.hpp` (each a
///     `uint64_t`-backed strong typedef, not `u16`/`u8` as USB VID/PID might
///     suggest)
///
/// Wire frame: `[4-byte big-endian body length][1-byte message type][8-byte
/// big-endian request id, request/response messages only][payload, in the
/// host's native byte order — little-endian on every real macOS host]`.
///
/// This implementation only ever sends (never reads) frames: it fires
/// `virtual_hid_keyboard_initialize` once at startup and
/// `post_keyboard_input_report` per report, the same fire-and-forget shape
/// as the old IOKit `post_report`. It does not parse the daemon's responses
/// or the `driver_connected`/`virtual_hid_keyboard_ready` state pushes the
/// reference C++ client reacts to — known simplification, see the module
/// doc above. A background thread sends a heartbeat frame periodically so
/// the daemon doesn't consider the connection dead during idle stretches
/// (no keystrokes); the exact heartbeat cadence the daemon expects is
/// unconfirmed, so `HEARTBEAT_INTERVAL` is a conservative guess.
use std::collections::HashSet;
use std::io::Write;
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use tracing::{info, warn};

const SOCKET_PATH: &str =
    "/Library/Application Support/org.pqrs/tmp/rootonly/karabiner_virtual_hid_device_service.sock";

/// Well under the ~30s `heartbeat_timeout` the reference C++ client library
/// configures on its side; unconfirmed against a live daemon.
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(10);

// ── cpp-unix_domain_stream wire framing ─────────────────────────────────────

#[repr(u8)]
enum MessageType {
    Heartbeat = 0,
    Request = 4,
}

fn frame_header(body_size: usize) -> [u8; 4] {
    (body_size as u32).to_be_bytes()
}

fn make_heartbeat_frame() -> Vec<u8> {
    let mut frame = Vec::with_capacity(5);
    frame.extend_from_slice(&frame_header(1));
    frame.push(MessageType::Heartbeat as u8);
    frame
}

fn make_request_frame(request_id: u64, payload: &[u8]) -> Vec<u8> {
    let body_size = 1 + 8 + payload.len();
    let mut frame = Vec::with_capacity(4 + body_size);
    frame.extend_from_slice(&frame_header(body_size));
    frame.push(MessageType::Request as u8);
    frame.extend_from_slice(&request_id.to_be_bytes());
    frame.extend_from_slice(payload);
    frame
}

// ── virtual_hid_device_service request payloads ─────────────────────────────

#[repr(u8)]
enum RequestType {
    VirtualHidKeyboardInitialize = 0,
    PostKeyboardInputReport = 6,
}

/// `virtual_hid_keyboard_parameters`: vendor_id/product_id/country_code,
/// each a `uint64_t`-backed pqrs::hid strong typedef (24 bytes total, native
/// byte order). Vendor/product IDs match Karabiner-Elements' own defaults;
/// country_code 0 = "not supported" (no specific keyboard layout).
fn virtual_hid_keyboard_initialize_payload() -> Vec<u8> {
    const VENDOR_ID: u64 = 0x16c0;
    const PRODUCT_ID: u64 = 0x27db;
    const COUNTRY_CODE_NOT_SUPPORTED: u64 = 0;

    let mut payload = Vec::with_capacity(1 + 24);
    payload.push(RequestType::VirtualHidKeyboardInitialize as u8);
    payload.extend_from_slice(&VENDOR_ID.to_le_bytes());
    payload.extend_from_slice(&PRODUCT_ID.to_le_bytes());
    payload.extend_from_slice(&COUNTRY_CODE_NOT_SUPPORTED.to_le_bytes());
    payload
}

/// `hid_report::keyboard_input`: packed `report_id(1)=1, modifiers(1),
/// reserved(1)=0, keys[32]` as `u16` (native byte order) — richer than the
/// legacy 8-byte boot-protocol report (32 simultaneous keys instead of 6),
/// but `build_report`'s `[u8; 8]` shape only ever gives us up to 6 non-zero
/// keycodes, so the remaining slots stay zero.
fn post_keyboard_input_report_payload(modifier_bits: u8, keys: &[u8]) -> Vec<u8> {
    let mut payload = Vec::with_capacity(1 + 3 + 64);
    payload.push(RequestType::PostKeyboardInputReport as u8);
    payload.push(1); // report_id
    payload.push(modifier_bits);
    payload.push(0); // reserved
    for i in 0..32 {
        let key = keys.get(i).copied().unwrap_or(0) as u16;
        payload.extend_from_slice(&key.to_le_bytes());
    }
    payload
}

// ── Public API (unchanged shape from the old IOKit version) ────────────────

/// A connection to `Karabiner-VirtualHIDDevice-Daemon` over its UNIX domain
/// stream socket.
pub struct KvhdHandle {
    stream: Arc<Mutex<UnixStream>>,
    next_request_id: AtomicU64,
}

impl KvhdHandle {
    /// Connect to the daemon and initialize a virtual keyboard.
    pub fn open() -> Result<Self> {
        let stream = UnixStream::connect(SOCKET_PATH).with_context(|| {
            format!(
                "connecting to {SOCKET_PATH} — is Karabiner-VirtualHIDDevice-Daemon running? \
                 Start it with: sudo '/Library/Application Support/org.pqrs/Karabiner-DriverKit-VirtualHIDDevice/Applications/Karabiner-VirtualHIDDevice-Daemon.app/Contents/MacOS/Karabiner-VirtualHIDDevice-Daemon'"
            )
        })?;
        let stream = Arc::new(Mutex::new(stream));

        let handle = Self {
            stream: stream.clone(),
            next_request_id: AtomicU64::new(1),
        };
        handle
            .send_request(&virtual_hid_keyboard_initialize_payload())
            .context("sending virtual_hid_keyboard_initialize")?;
        info!("kvhd: connected, sent virtual_hid_keyboard_initialize (vendor=0x16c0 product=0x27db)");

        thread::spawn(move || loop {
            thread::sleep(HEARTBEAT_INTERVAL);
            let frame = make_heartbeat_frame();
            let mut guard = stream.lock().unwrap();
            if let Err(e) = guard.write_all(&frame) {
                warn!("kvhd: heartbeat write failed: {e}");
            }
        });

        Ok(handle)
    }

    fn send_request(&self, payload: &[u8]) -> Result<()> {
        let request_id = self.next_request_id.fetch_add(1, Ordering::Relaxed);
        let frame = make_request_frame(request_id, payload);
        let mut guard = self.stream.lock().unwrap();
        guard.write_all(&frame).context("writing request frame")?;
        Ok(())
    }

    /// Post an 8-byte HID boot-protocol keyboard report to the virtual
    /// device. Fire-and-forget: this only reports a failure to write to the
    /// socket (e.g. the daemon died); it cannot detect an application-level
    /// rejection by the daemon, since responses aren't read.
    pub fn post_report(&self, report: &[u8; 8]) -> Result<()> {
        let modifier_bits = report[0];
        let keys = &report[2..8];
        self.send_request(&post_keyboard_input_report_payload(modifier_bits, keys))
    }
}

/// Build an 8-byte HID boot-protocol keyboard report from the current state.
pub fn build_report(modifier_bits: u8, pressed: &HashSet<u8>) -> [u8; 8] {
    let mut report = [0u8; 8];
    report[0] = modifier_bits;
    for (i, &kc) in pressed.iter().take(6).enumerate() {
        report[2 + i] = kc;
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heartbeat_frame_has_1_byte_body_and_correct_type() {
        let frame = make_heartbeat_frame();
        // 4-byte big-endian length prefix + 1-byte message type, body_size == 1.
        assert_eq!(frame.len(), 5);
        assert_eq!(&frame[0..4], &[0, 0, 0, 1]);
        assert_eq!(frame[4], MessageType::Heartbeat as u8);
    }

    #[test]
    fn request_frame_header_covers_type_plus_request_id_plus_payload() {
        let payload = [0xAAu8, 0xBB, 0xCC];
        let frame = make_request_frame(0x0102030405060708, &payload);
        // body_size = 1 (type) + 8 (request id) + 3 (payload) = 12
        assert_eq!(&frame[0..4], &[0, 0, 0, 12]);
        assert_eq!(frame[4], MessageType::Request as u8);
        assert_eq!(&frame[5..13], &[1, 2, 3, 4, 5, 6, 7, 8]); // request id, big-endian
        assert_eq!(&frame[13..16], &payload);
        assert_eq!(frame.len(), 4 + 12);
    }

    #[test]
    fn virtual_hid_keyboard_initialize_payload_is_little_endian_u64_triple() {
        let payload = virtual_hid_keyboard_initialize_payload();
        assert_eq!(payload.len(), 1 + 24);
        assert_eq!(payload[0], RequestType::VirtualHidKeyboardInitialize as u8);
        assert_eq!(&payload[1..9], &0x16c0u64.to_le_bytes());
        assert_eq!(&payload[9..17], &0x27dbu64.to_le_bytes());
        assert_eq!(&payload[17..25], &0u64.to_le_bytes());
    }

    #[test]
    fn keyboard_input_report_payload_matches_packed_struct_layout() {
        let keys = [0x04, 0x05, 0, 0, 0, 0]; // 'a', 'b', then unused
        let payload = post_keyboard_input_report_payload(0x02, &keys); // left_shift
        assert_eq!(payload.len(), 1 + 3 + 64);
        assert_eq!(payload[0], RequestType::PostKeyboardInputReport as u8);
        assert_eq!(payload[1], 1); // report_id
        assert_eq!(payload[2], 0x02); // modifiers
        assert_eq!(payload[3], 0); // reserved
        // keys[0] = 0x0004 little-endian, keys[1] = 0x0005 little-endian
        assert_eq!(&payload[4..6], &0x0004u16.to_le_bytes());
        assert_eq!(&payload[6..8], &0x0005u16.to_le_bytes());
        // remaining 30 key slots are zero
        assert!(payload[8..].iter().all(|&b| b == 0));
    }

    #[test]
    fn keyboard_input_report_payload_handles_fewer_than_6_keys() {
        let keys = [0x16, 0, 0, 0, 0, 0]; // just 's'
        let payload = post_keyboard_input_report_payload(0, &keys);
        assert_eq!(&payload[4..6], &0x0016u16.to_le_bytes());
        assert!(payload[6..].iter().all(|&b| b == 0));
    }
}
