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
    // Prefer XDG_RUNTIME_DIR (Linux standard; not set on macOS by default).
    if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
        let p = PathBuf::from(dir).join("dualie");
        std::fs::create_dir_all(&p).ok();
        return p.join("daemon.sock");
    }

    // Fixed fallback — a well-known path so dua can always find us without
    // guessing among stale /tmp/dualie-<pid> directories from past runs.
    PathBuf::from("/tmp/dualie.sock")
}

// ── Status payload ────────────────────────────────────────────────────────────

fn status_json() -> String {
    let serial = if crate::peer::CONNECTED.load(std::sync::atomic::Ordering::Relaxed) {
        "connected"
    } else {
        "disconnected"
    };
    let git_pending = crate::git_sync::GIT_PENDING.load(std::sync::atomic::Ordering::Relaxed);
    let repo_dir = crate::git_sync::REPO_DIR
        .get()
        .map(|p| p.display().to_string())
        .unwrap_or_default();

    format!(
        "{{\"version\":{version:?},\"config\":{config:?},\"serial\":{serial:?},\
         \"git_pending\":{git_pending},\"repo_dir\":{repo_dir:?},\"pid\":{pid}}}\n",
        version     = env!("CARGO_PKG_VERSION"),
        config      = kdl_config_path().display().to_string(),
        serial      = serial,
        git_pending = git_pending,
        repo_dir    = repo_dir,
        pid         = std::process::id(),
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

        // Allow any local user to connect (daemon may run as root via sudo).
        if let Err(e) = std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o666)) {
            warn!("status socket chmod: {e}");
        }

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
