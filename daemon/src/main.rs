use anyhow::Result;
use clap::Parser;
use tracing::info;

mod apps;
mod clipboard;
mod config;
mod file_sync;
mod git_sync;
mod intercept;
mod peer;
mod serialize;
mod status;
mod sync_engine;

use config::DualieConfig;

/// Dualie daemon – local keyboard remapping and RP2040 serial peer.
#[derive(Parser, Debug)]
#[command(version, about)]
struct Args {
    /// Path to the CDC-ACM serial device for the local RP2040.
    /// Auto-detected from /dev/ttyACM* if omitted.
    #[arg(long)]
    serial: Option<String>,

    /// Send a one-shot serial command and exit.
    #[arg(long, value_name = "CMD")]
    serial_cmd: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "dualie=info".into()),
        )
        .init();

    let args = Args::parse();

    // ── One-shot serial command mode (used by `just flash`) ──────────────────
    if let Some(cmd) = args.serial_cmd {
        return run_serial_cmd(&cmd, args.serial.as_deref()).await;
    }

    // ── Guard against duplicate instances ────────────────────────────────────
    {
        let sock = status::socket_path();
        if std::os::unix::net::UnixStream::connect(&sock).is_ok() {
            eprintln!(
                "error: dualie is already running (socket {}).\n\
                 Stop the existing instance first:\n\
                 \n  systemctl --user stop dualie\n\nor kill the process and retry.",
                sock.display()
            );
            std::process::exit(1);
        }
    }

    // ── Load config (KDL, with JSON legacy fallback) ──────────────────────────
    let cfg_rx = DualieConfig::watch()?;
    info!("config: {}", config::kdl_config_path().display());

    // ── Git-backed config versioning (Phase 7) ────────────────────────────────
    {
        let local   = config::LocalConfig::load();
        let repo_dir = local.repo_path
            .unwrap_or_else(git_sync::default_repo_dir);
        let repo = std::sync::Arc::new(git_sync::GitRepo::new(
            repo_dir,
            config::kdl_config_path(),
            local.machine_name.clone(),
        ));
        match repo.open_or_init().await {
            Ok(()) => {
                if let Some(remote) = cfg_rx.borrow().git_sync.remote.clone() {
                    if let Err(e) = repo.set_remote(&remote).await {
                        tracing::warn!("git: set remote: {e}");
                    }
                }
                info!("git: repo at {} (machine: {})", repo.repo_dir().display(), local.machine_name);
                git_sync::spawn(repo);
            }
            Err(e) => tracing::warn!("git: init failed, sync disabled: {e}"),
        }
    }

    // ── Serial peer ───────────────────────────────────────────────────────────
    let serial_path = args.serial.unwrap_or_else(|| "/dev/ttyACM0".to_owned());
    info!("serial: {serial_path}");
    let serial_client = peer::spawn(serial_path);

    // ── Status socket ─────────────────────────────────────────────────────────
    status::spawn();

    // ── File sync (Phase 6) ───────────────────────────────────────────────────
    {
        let local_for_sync = config::LocalConfig::load();
        file_sync::spawn(cfg_rx.clone(), serial_client.clone(), local_for_sync.machine_name);
    }

    // ── Active output state (shared between serial peer and intercept) ────────
    let active_output = intercept::new_active_output();

    // ── Key interceptor (dedicated OS thread — evdev blocks) ─────────────────
    let cfg_for_intercept = cfg_rx.clone();
    let serial_for_intercept = serial_client.clone();
    let active_for_intercept = active_output.clone();
    std::thread::spawn(move || {
        if let Err(e) = intercept::run(cfg_for_intercept, serial_for_intercept, active_for_intercept) {
            tracing::error!("key interceptor: {e}");
        }
    });

    // Park the main task forever; all work is in spawned tasks/threads.
    std::future::pending::<()>().await;
    Ok(())
}

// ── One-shot serial command ───────────────────────────────────────────────────

async fn run_serial_cmd(cmd: &str, serial: Option<&str>) -> Result<()> {
    use dualie_proto::{DualieMessage, SerialPeer};

    let path_str = serial.unwrap_or("/dev/ttyACM0");
    let peer = SerialPeer::open(std::path::Path::new(path_str))?;
    let (mut writer, _reader) = peer.into_split();

    let msg = match cmd {
        "reboot-to-bootloader" => DualieMessage::RebootToBootloader,
        "ping"                 => DualieMessage::Ping,
        other => anyhow::bail!("unknown serial command: {other:?}"),
    };

    writer.send(&msg).await?;
    info!("sent {cmd} to {path_str}");
    Ok(())
}
