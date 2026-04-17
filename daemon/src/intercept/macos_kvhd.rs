/// intercept/macos_kvhd.rs — Client for Karabiner-DriverKit-VirtualHIDDevice daemon.
///
/// # Architecture
///
/// The DriverKit extension (`org.pqrs.Karabiner-DriverKit-VirtualHIDDevice`) requires
/// the `com.apple.developer.driverkit.userclient-access` entitlement to open an IOKit
/// user client — unsigned/locally-built binaries cannot connect directly.
///
/// Instead, Karabiner ships `Karabiner-VirtualHIDDevice-Daemon` (runs as root), which
/// holds the privileged IOKit connection and proxies reports from any local client via
/// Unix datagram sockets.
///
/// # Protocol (pqrs virtual_hid_device_service, protocol version 5)
///
/// Server socket: `/Library/Application Support/org.pqrs/tmp/rootonly/vhidd_server/*.sock`
///   (glob, use the lexicographically last = newest)
///
/// Client socket: a unique path we create in `.../vhidd_client/<hex_nanos>.sock`
///
/// Message wire format:
///   byte 0:   'c'
///   byte 1:   'p'
///   bytes 2-3: protocol_version = 5 (u16 LE)
///   byte 4:   request (u8 enum)
///   bytes 5+: packed payload
///
/// Keyboard report payload (request = 7 = post_keyboard_input_report):
///   byte 0:    report_id = 1
///   byte 1:    modifiers (HID bitmask, same as boot-protocol)
///   byte 2:    reserved = 0
///   bytes 3-66: keys[32] (u16 LE each — HID usage codes)
///   total payload: 67 bytes → message: 72 bytes
///
/// Initialize sequence:
///   1. Send virtual_hid_keyboard_initialize (request=1) with parameters payload
///   2. Receive virtual_hid_keyboard_ready (response=4, value=1) from daemon
///   3. Ready to send reports
///
/// Response format (received on our client socket):
///   byte 0: response enum
///   byte 1: bool value (for ready/activated/connected responses)

use std::os::unix::io::RawFd;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Result};
use tracing::info;

// ── Socket paths ──────────────────────────────────────────────────────────────

const ROOTONLY_DIR: &str = "/Library/Application Support/org.pqrs/tmp/rootonly";

fn server_socket_dir() -> PathBuf {
    PathBuf::from(ROOTONLY_DIR).join("vhidd_server")
}

fn client_socket_dir() -> PathBuf {
    PathBuf::from(ROOTONLY_DIR).join("vhidd_client")
}

// ── Protocol constants ────────────────────────────────────────────────────────

const PROTOCOL_VERSION: u16 = 5;
const MAGIC: [u8; 2] = [b'c', b'p'];

#[repr(u8)]
enum Request {
    VirtualHidKeyboardInitialize = 1,
    PostKeyboardInputReport      = 7,
}

#[repr(u8)]
#[derive(PartialEq)]
enum Response {
    VirtualHidKeyboardReady = 4,
}

/// Default keyboard parameters (vendor=0x16c0, product=0x27db, country=0).
/// Matches `virtual_hid_keyboard_parameters` defaults in the pqrs library.
const KBD_PARAMS: [u8; 6] = [
    0xc0, 0x16,  // vendor_id  = 0x16c0 LE
    0xdb, 0x27,  // product_id = 0x27db LE
    0x00, 0x00,  // country_code = 0 LE
];

// ── KvhdHandle ────────────────────────────────────────────────────────────────

/// Handle to the Karabiner VirtualHIDDevice daemon connection.
pub struct KvhdHandle {
    /// The server socket path we're sending to.
    server_path: PathBuf,
    /// Our client socket fd (SOCK_DGRAM).
    client_fd:   RawFd,
    /// Our client socket path (must stay alive while fd is open).
    client_path: PathBuf,
}

impl KvhdHandle {
    /// Connect to the daemon and initialize the virtual keyboard.
    /// Retries until the keyboard is ready (up to ~5s).
    pub fn open() -> Result<Self> {
        let server_path = find_server_socket()?;
        let (client_fd, client_path) = create_client_socket()?;

        let handle = KvhdHandle { server_path, client_fd, client_path };
        handle.initialize_keyboard()?;
        Ok(handle)
    }

    /// Post a keyboard report to the virtual device.
    pub fn post_report(&self, report: &KvhdReport) -> Result<()> {
        let msg = build_message(Request::PostKeyboardInputReport, &report.as_bytes());
        self.send(&msg)
    }

    fn initialize_keyboard(&self) -> Result<()> {
        let msg = build_message(Request::VirtualHidKeyboardInitialize, &KBD_PARAMS);
        self.send(&msg)?;
        self.wait_keyboard_ready()
    }

    fn send(&self, msg: &[u8]) -> Result<()> {
        let server_cstr = path_to_cstring(&self.server_path)?;
        let ret = unsafe {
            let mut addr: libc::sockaddr_un = std::mem::zeroed();
            addr.sun_family = libc::AF_UNIX as _;
            let bytes = server_cstr.as_bytes_with_nul();
            if bytes.len() > addr.sun_path.len() {
                bail!("server socket path too long");
            }
            let dst = addr.sun_path.as_mut_ptr() as *mut u8;
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), dst, bytes.len());
            libc::sendto(
                self.client_fd,
                msg.as_ptr() as *const libc::c_void,
                msg.len(),
                0,
                &addr as *const _ as *const libc::sockaddr,
                std::mem::size_of::<libc::sockaddr_un>() as libc::socklen_t,
            )
        };
        if ret < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(())
    }

    /// Wait up to 3s for a `virtual_hid_keyboard_ready(true)` response.
    fn wait_keyboard_ready(&self) -> Result<()> {
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        let mut buf = [0u8; 64];
        loop {
            // Poll with a short timeout.
            let remaining_ms = deadline.saturating_duration_since(std::time::Instant::now()).as_millis();
            if remaining_ms == 0 {
                bail!("timed out waiting for virtual_hid_keyboard_ready");
            }
            let mut pfd = libc::pollfd {
                fd:      self.client_fd,
                events:  libc::POLLIN,
                revents: 0,
            };
            let ret = unsafe { libc::poll(&mut pfd, 1, remaining_ms.min(200) as libc::c_int) };
            if ret < 0 {
                let e = std::io::Error::last_os_error();
                if e.kind() == std::io::ErrorKind::Interrupted { continue; }
                return Err(e.into());
            }
            if ret == 0 {
                // Timeout slice — re-send initialize in case the daemon wasn't ready.
                let msg = build_message(Request::VirtualHidKeyboardInitialize, &KBD_PARAMS);
                let _ = self.send(&msg);
                continue;
            }
            let n = unsafe {
                libc::recv(self.client_fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len(), 0)
            };
            if n < 2 { continue; }
            // Response format: [response_type: u8][value: u8]
            if buf[0] == Response::VirtualHidKeyboardReady as u8 && buf[1] == 1 {
                info!("macOS: Karabiner VirtualHIDKeyboard ready");
                return Ok(());
            }
        }
    }
}

impl Drop for KvhdHandle {
    fn drop(&mut self) {
        unsafe {
            libc::close(self.client_fd);
            libc::unlink(path_to_cstring(&self.client_path)
                .map(|c| c.into_raw())
                .unwrap_or(std::ptr::null_mut()));
        }
    }
}

// ── KvhdReport ────────────────────────────────────────────────────────────────

/// A keyboard input report in the pqrs virtual_hid_device_driver format.
///
/// Layout (packed, 67 bytes):
///   [0]     report_id = 1
///   [1]     modifiers (HID boot-protocol bitmask)
///   [2]     reserved  = 0
///   [3..67] keys[32]  (u16 LE HID usage codes)
pub struct KvhdReport([u8; 67]);

impl KvhdReport {
    fn as_bytes(&self) -> [u8; 67] { self.0 }
}

/// Build a KvhdReport from the current modifier bits and pressed key set.
pub fn build_report(modifier_bits: u8, pressed: &std::collections::HashSet<u8>) -> KvhdReport {
    let mut r = [0u8; 67];
    r[0] = 1;               // report_id
    r[1] = modifier_bits;
    // r[2] = reserved = 0
    // keys[32] as u16 LE starting at offset 3
    for (i, &kc) in pressed.iter().take(32).enumerate() {
        let off = 3 + i * 2;
        r[off]     = kc;    // low byte of u16
        r[off + 1] = 0;     // high byte (HID keycodes fit in u8)
    }
    KvhdReport(r)
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn build_message(req: Request, payload: &[u8]) -> Vec<u8> {
    let mut msg = Vec::with_capacity(5 + payload.len());
    msg.extend_from_slice(&MAGIC);
    msg.extend_from_slice(&PROTOCOL_VERSION.to_le_bytes());
    msg.push(req as u8);
    msg.extend_from_slice(payload);
    msg
}

/// Find the newest server socket by globbing and sorting lexicographically.
fn find_server_socket() -> Result<PathBuf> {
    let dir = server_socket_dir();
    if !dir.exists() {
        bail!(
            "Karabiner VirtualHIDDevice daemon socket directory not found at {}\n\
             Install Karabiner-DriverKit-VirtualHIDDevice and ensure the daemon is running:\n\
             sudo /Library/Application\\ Support/org.pqrs/Karabiner-DriverKit-VirtualHIDDevice/\
Applications/Karabiner-VirtualHIDDevice-Daemon.app/Contents/MacOS/Karabiner-VirtualHIDDevice-Daemon",
            dir.display()
        );
    }
    let mut entries: Vec<PathBuf> = std::fs::read_dir(&dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("sock"))
        .collect();
    entries.sort();
    entries.pop().ok_or_else(|| anyhow::anyhow!(
        "Karabiner VirtualHIDDevice daemon has no server socket in {} — is it running?",
        dir.display()
    ))
}

/// Create our client datagram socket bound to a unique path in vhidd_client/.
fn create_client_socket() -> Result<(RawFd, PathBuf)> {
    let dir = client_socket_dir();
    std::fs::create_dir_all(&dir)?;

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    let pid = std::process::id();
    let name = format!("{pid:08x}{nanos:08x}.sock");
    let path = dir.join(&name);

    let fd: RawFd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_DGRAM, 0) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().into());
    }

    let cstr = path_to_cstring(&path)?;
    let ret = unsafe {
        let mut addr: libc::sockaddr_un = std::mem::zeroed();
        addr.sun_family = libc::AF_UNIX as _;
        let bytes = cstr.as_bytes_with_nul();
        if bytes.len() > addr.sun_path.len() {
            libc::close(fd);
            bail!("client socket path too long: {}", path.display());
        }
        let dst = addr.sun_path.as_mut_ptr() as *mut u8;
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), dst, bytes.len());
        libc::bind(
            fd,
            &addr as *const _ as *const libc::sockaddr,
            std::mem::size_of::<libc::sockaddr_un>() as libc::socklen_t,
        )
    };
    if ret < 0 {
        unsafe { libc::close(fd); }
        return Err(std::io::Error::last_os_error().into());
    }

    // Set permissions so the daemon (root) can write back to us.
    let path_cstr = path_to_cstring(&path)?;
    unsafe { libc::chmod(path_cstr.as_ptr(), 0o777); }

    Ok((fd, path))
}

fn path_to_cstring(path: &PathBuf) -> Result<std::ffi::CString> {
    let s = path.to_str().ok_or_else(|| anyhow::anyhow!("non-UTF8 path: {}", path.display()))?;
    Ok(std::ffi::CString::new(s)?)
}
