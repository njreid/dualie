use anyhow::Result;
use clap::Parser;
use tracing::info;

mod config;
mod intercept;
mod peer;
mod serialize;
mod status;

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

    // ── Load config (KDL, with JSON legacy fallback) ──────────────────────────
    let cfg_rx = DualieConfig::watch()?;
    info!("config: {}", config::kdl_config_path().display());

    // ── Serial peer ───────────────────────────────────────────────────────────
    let serial_path = args.serial.unwrap_or_else(|| "/dev/ttyACM0".to_owned());
    info!("serial: {serial_path}");
    // Kept alive for process lifetime; will be passed to intercept layer in Phase 3.
    let _serial_client = peer::spawn(serial_path);

    // ── Status socket ─────────────────────────────────────────────────────────
    status::spawn();

    // ── Key interceptor (dedicated OS thread — rdev blocks) ───────────────────
    let cfg_for_intercept = cfg_rx.clone();
    std::thread::spawn(move || {
        if let Err(e) = intercept::run(cfg_for_intercept) {
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
