/// status.rs — Unix-domain socket status endpoint.
///
/// Listens at `$XDG_RUNTIME_DIR/dualie/daemon.sock` (or `/tmp/dualie-NNNN/daemon.sock`
/// as a fallback when XDG_RUNTIME_DIR is unset).
///
/// Protocol: on each accepted connection the daemon writes a single JSON line
/// and closes the socket.  Read it with: `socat - UNIX-CONNECT:/path/daemon.sock`
///
/// Response fields:
///   version    – daemon version string
///   config     – path of the active config file (may not exist yet)
///   serial     – "connected" | "disconnected"
///   pid        – daemon process ID (for kill/restart scripts)

use std::path::PathBuf;
use tokio::io::AsyncWriteExt;
use tokio::net::UnixListener;
use tracing::{error, info, warn};

use crate::config::kdl_config_path;

// ── Socket path resolution ────────────────────────────────────────────────────

pub fn socket_path() -> PathBuf {
    // Prefer XDG_RUNTIME_DIR so the socket is on a tmpfs and auto-cleaned on logout.
    if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
        let p = PathBuf::from(dir).join("dualie");
        std::fs::create_dir_all(&p).ok();
        return p.join("daemon.sock");
    }

    // Fallback: /tmp/dualie-<pid>/daemon.sock (unique enough for a daemon)
    let p = PathBuf::from(format!("/tmp/dualie-{}", std::process::id()));
    std::fs::create_dir_all(&p).ok();
    p.join("daemon.sock")
}

// ── Status payload ────────────────────────────────────────────────────────────

fn status_json() -> String {
    let serial = if crate::peer::CONNECTED.load(std::sync::atomic::Ordering::Relaxed) {
        "connected"
    } else {
        "disconnected"
    };

    format!(
        "{{\"version\":{version:?},\"config\":{config:?},\"serial\":{serial:?},\"pid\":{pid}}}\n",
        version = env!("CARGO_PKG_VERSION"),
        config  = kdl_config_path().display().to_string(),
        serial  = serial,
        pid     = std::process::id(),
    )
}

// ── Listener task ─────────────────────────────────────────────────────────────

/// Spawn the status socket listener as a background task.
pub fn spawn() {
    let path = socket_path();

    // Remove stale socket from a previous run (bind fails if it exists).
    let _ = std::fs::remove_file(&path);

    tokio::spawn(async move {
        let listener = match UnixListener::bind(&path) {
            Ok(l) => l,
            Err(e) => {
                error!("status socket bind {}: {e}", path.display());
                return;
            }
        };

        info!("status socket at {}", path.display());

        loop {
            match listener.accept().await {
                Ok((mut stream, _)) => {
                    let json = status_json();
                    tokio::spawn(async move {
                        let _ = stream.write_all(json.as_bytes()).await;
                    });
                }
                Err(e) => {
                    warn!("status socket accept: {e}");
                }
            }
        }
    });
}
