/// file_sync.rs — Config-file sync over CDC-ACM serial.
///
/// # Flow (initiator side — file changed locally)
///
/// 1. `notify` watcher detects a change to a file owned by an enabled sync app.
/// 2. Daemon reads the file, computes mtime and SHA-256.
/// 3. Sends `SyncChunk { rel_path, offset: 0, data: full_content, total_size,
///    modified_ms }` over serial.  Config files are small (<1 MiB); single-chunk.
///
/// # Flow (responder side — chunk received from remote)
///
/// 1. `handle_chunk` receives the incoming `SyncChunk`.
/// 2. Resolves the absolute local path from `rel_path` (home-relative).
/// 3. Applies `sync_engine::apply_remote` (LWW + local-section preservation).
/// 4. Writes the result if the remote won or there is a conflict.
/// 5. On conflict, also writes a `.dualie-conflict` backup.
/// 6. Sends `SyncAck { rel_path }` back to the sender.
///
/// # rel_path convention
///
/// All paths use the home-relative form with `~/` stripped, e.g.
/// `.config/helix/config.toml`.  The receiver expands back to an absolute
/// path by prepending its own home directory.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::watch;
use tracing::{info, warn};

use crate::apps::AppRegistry;
use crate::config::DualieConfig;
use crate::peer::SerialClient;
use crate::sync_engine;
use dualie_proto::{DualieMessage, FileChunk};

// ── Public entry points ───────────────────────────────────────────────────────

/// Spawn the file-sync background task.
///
/// Watches all files belonging to apps enabled in the sync config.
/// On file change: sends a `SyncChunk` over serial.
/// On incoming `SyncChunk`: applies LWW, writes if needed, sends `SyncAck`.
pub fn spawn(cfg_rx: watch::Receiver<DualieConfig>, serial: SerialClient) {
    let serial_for_recv = serial.clone();

    // Channel for inbound SyncChunk / SyncAck messages from the peer dispatch loop.
    let (chunk_tx, chunk_rx) = tokio::sync::mpsc::channel::<DualieMessage>(64);
    CHUNK_SENDER.set(chunk_tx).ok();

    tokio::spawn(async move {
        run_watcher(cfg_rx, serial, chunk_rx).await;
    });

    let _ = serial_for_recv; // used via the global sender above
}

/// Called by `peer::dispatch` when a `SyncChunk` or `SyncAck` arrives.
pub fn handle_incoming(msg: DualieMessage) {
    if let Some(tx) = CHUNK_SENDER.get() {
        let _ = tx.try_send(msg);
    }
}

// ── Global inbound channel ────────────────────────────────────────────────────

static CHUNK_SENDER: once_cell::sync::OnceCell<tokio::sync::mpsc::Sender<DualieMessage>> =
    once_cell::sync::OnceCell::new();

// ── Watcher task ──────────────────────────────────────────────────────────────

async fn run_watcher(
    mut cfg_rx: watch::Receiver<DualieConfig>,
    serial:     SerialClient,
    mut chunk_rx: tokio::sync::mpsc::Receiver<DualieMessage>,
) {
    // Channel from the notify thread into the async task.
    let (fs_tx, mut fs_rx) = tokio::sync::mpsc::channel::<PathBuf>(256);
    let fs_tx = Arc::new(fs_tx);

    // Start with an empty watcher; rebuilt on config changes.
    let mut _watcher: Option<RecommendedWatcher> = None;
    let mut watched_paths: HashSet<PathBuf> = HashSet::new();

    loop {
        tokio::select! {
            // Config changed — rebuild watch set.
            _ = cfg_rx.changed() => {
                let cfg = cfg_rx.borrow_and_update().clone();
                let (watcher, paths) = build_watcher(&cfg, Arc::clone(&fs_tx));
                watched_paths = paths;
                _watcher = Some(watcher);
                info!(count = watched_paths.len(), "sync: watching files");
            }

            // Notify fired — a watched file changed.
            Some(path) = fs_rx.recv() => {
                if !watched_paths.contains(&path) {
                    continue;
                }
                match send_file(&path, &serial).await {
                    Ok(()) => info!("sync: sent {}", path.display()),
                    Err(e) => warn!("sync: failed to send {}: {e}", path.display()),
                }
            }

            // Inbound chunk from remote.
            Some(msg) = chunk_rx.recv() => {
                match msg {
                    DualieMessage::SyncChunk(chunk) => {
                        if let Err(e) = receive_chunk(chunk, &serial).await {
                            warn!("sync: receive_chunk error: {e}");
                        }
                    }
                    DualieMessage::SyncAck { rel_path } => {
                        info!("sync: ack for {rel_path}");
                    }
                    _ => {}
                }
            }
        }
    }
}

// ── Build notify watcher ──────────────────────────────────────────────────────

fn build_watcher(
    cfg:    &DualieConfig,
    fs_tx:  Arc<tokio::sync::mpsc::Sender<PathBuf>>,
) -> (RecommendedWatcher, HashSet<PathBuf>) {
    let mut paths: HashSet<PathBuf> = HashSet::new();

    let Ok(registry) = AppRegistry::load().map_err(|e| {
        warn!("sync: failed to load app registry: {e}");
        e
    }) else {
        return (make_noop_watcher(), paths);
    };

    for app_name in &cfg.sync.apps {
        if let Some(app) = registry.get(app_name) {
            for p in app.expand_globs() {
                paths.insert(p);
            }
        } else {
            warn!("sync: unknown app {app_name:?} in sync config");
        }
    }

    let tx = Arc::clone(&fs_tx);
    let cb = move |res: notify::Result<Event>| {
        if let Ok(ev) = res {
            if matches!(ev.kind, EventKind::Modify(_) | EventKind::Create(_)) {
                for path in ev.paths {
                    let _ = tx.try_send(path);
                }
            }
        }
    };

    let mut watcher = match notify::recommended_watcher(cb) {
        Ok(w) => w,
        Err(e) => {
            warn!("sync: failed to create watcher: {e}");
            return (make_noop_watcher(), paths);
        }
    };

    for p in &paths {
        if let Err(e) = watcher.watch(p, RecursiveMode::NonRecursive) {
            warn!("sync: failed to watch {}: {e}", p.display());
        }
    }

    (watcher, paths)
}

fn make_noop_watcher() -> RecommendedWatcher {
    notify::recommended_watcher(|_| {}).unwrap()
}

// ── Send a file to the remote ─────────────────────────────────────────────────

async fn send_file(path: &Path, serial: &SerialClient) -> Result<()> {
    let data = tokio::fs::read(path).await
        .with_context(|| format!("reading {}", path.display()))?;

    let mtime = tokio::fs::metadata(path).await
        .and_then(|m| m.modified())
        .unwrap_or(SystemTime::now());

    let modified_ms = mtime.duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    let total_size = data.len() as u64;
    let rel_path   = home_relative(path);

    let chunk = FileChunk {
        rel_path,
        offset: 0,
        data,
        total_size,
        modified_ms,
    };

    serial.send(DualieMessage::SyncChunk(chunk)).await;
    Ok(())
}

// ── Receive a file from the remote ───────────────────────────────────────────

async fn receive_chunk(chunk: FileChunk, serial: &SerialClient) -> Result<()> {
    // Only single-chunk transfers are supported; multi-chunk is ignored.
    if chunk.offset != 0 || chunk.data.len() as u64 != chunk.total_size {
        warn!("sync: multi-chunk transfer not supported for {}", chunk.rel_path);
        return Ok(());
    }

    let local_path = expand_home_relative(&chunk.rel_path);

    // Read local file if it exists.
    let (local_raw, local_mtime) = if local_path.is_file() {
        let raw   = tokio::fs::read(&local_path).await
            .with_context(|| format!("reading {}", local_path.display()))?;
        let mtime = tokio::fs::metadata(&local_path).await
            .and_then(|m| m.modified())
            .unwrap_or(UNIX_EPOCH);
        (raw, mtime)
    } else {
        // File doesn't exist locally — accept remote unconditionally.
        (Vec::new(), UNIX_EPOCH)
    };

    let local_str   = String::from_utf8_lossy(&local_raw);
    let remote_str  = String::from_utf8_lossy(&chunk.data);
    let remote_mtime = UNIX_EPOCH + Duration::from_millis(chunk.modified_ms);

    // Determine the comment char from the app registry (best effort).
    let comment_char = comment_char_for_path(&local_path);

    let outcome = sync_engine::apply_remote(
        &local_str,
        local_mtime,
        &remote_str,
        remote_mtime,
        &comment_char,
    );

    use sync_engine::Winner;
    match outcome.winner {
        Winner::Identical => {
            info!("sync: {} — identical, no write", chunk.rel_path);
        }
        Winner::Local => {
            info!("sync: {} — local is newer, no write", chunk.rel_path);
        }
        Winner::Remote => {
            write_file(&local_path, outcome.content.as_bytes()).await?;
            info!("sync: {} — remote wins, written", chunk.rel_path);
            crate::git_sync::trigger_commit();
        }
        Winner::Conflict => {
            // Write conflict backup.
            if let Some(backup) = &outcome.conflict_backup {
                let backup_path = conflict_backup_path(&local_path);
                write_file(&backup_path, backup.as_bytes()).await?;
                warn!("sync: {} — conflict, backup at {}", chunk.rel_path, backup_path.display());
            }
            write_file(&local_path, outcome.content.as_bytes()).await?;
            crate::git_sync::trigger_commit();
        }
    }

    serial.send(DualieMessage::SyncAck { rel_path: chunk.rel_path }).await;
    Ok(())
}

async fn write_file(path: &Path, data: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await
            .with_context(|| format!("creating parent dirs for {}", path.display()))?;
    }
    tokio::fs::write(path, data).await
        .with_context(|| format!("writing {}", path.display()))
}

// ── Path helpers ──────────────────────────────────────────────────────────────

/// Convert an absolute path to a home-relative string like `.config/helix/config.toml`.
fn home_relative(path: &Path) -> String {
    if let Some(home) = home_dir() {
        if let Ok(rel) = path.strip_prefix(&home) {
            return rel.to_string_lossy().to_string();
        }
    }
    path.to_string_lossy().to_string()
}

/// Expand a home-relative string back to an absolute path.
fn expand_home_relative(rel: &str) -> PathBuf {
    if let Some(home) = home_dir() {
        if !rel.starts_with('/') {
            return home.join(rel);
        }
    }
    PathBuf::from(rel)
}

fn home_dir() -> Option<PathBuf> {
    directories::UserDirs::new().map(|d| d.home_dir().to_path_buf())
}

/// Backup path for a conflict: `<original>.dualie-conflict`.
fn conflict_backup_path(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_os_string();
    s.push(".dualie-conflict");
    PathBuf::from(s)
}

/// Look up the comment char for a path by scanning the app registry.
/// Falls back to `"//"` if the app is not found.
fn comment_char_for_path(path: &Path) -> String {
    if let Ok(registry) = AppRegistry::load() {
        for app in registry.iter() {
            let globs = app.expand_globs();
            if globs.iter().any(|g| g == path) {
                return app.comment_char.clone();
            }
        }
    }
    "//".to_owned()
}
