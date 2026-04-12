use anyhow::Result;
use clap::Parser;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::info;

mod config;
mod device;
mod intercept;
mod peer;
mod platform;
mod serialize;
mod web;

use config::DualieConfig;
use device::DeviceState;

/// Dualie daemon – local keyboard remapping and RP2040 serial peer.
#[derive(Parser, Debug)]
#[command(version, about)]
struct Args {
    /// Port to listen on for the config web UI
    #[arg(long, default_value_t = 7474)]
    port: u16,

    /// Enable verbose logging and Vite dev proxy (use with `just dev`)
    #[arg(long)]
    dev: bool,

    /// Path to the CDC-ACM serial device for the local RP2040.
    /// Auto-detected from /dev/ttyACM* if omitted.
    #[arg(long)]
    serial: Option<String>,
}

pub struct AppState {
    pub config:  RwLock<DualieConfig>,
    pub device:  DeviceState,
    pub dev_mode: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG")
                .unwrap_or_else(|_| "dualie=info".into()),
        )
        .init();

    let args = Args::parse();

    // Load or initialise persisted config
    let config = DualieConfig::load_or_default()?;
    info!("Config loaded from {}", config::config_path().display());

    let state = Arc::new(AppState {
        config:   RwLock::new(config),
        device:   DeviceState::default(),
        dev_mode: args.dev,
    });

    // Spawn serial peer for the local RP2040 (auto-detect or use --serial path).
    // TODO (Phase 1.3): implement auto-detection in peer::detect_serial_path().
    let serial_path = args.serial.unwrap_or_else(|| "/dev/ttyACM0".to_owned());
    info!("Opening RP2040 serial connection on {serial_path}");
    let _serial_client = peer::spawn(serial_path);
    // _serial_client kept alive for the process lifetime; expose via AppState in Phase 1.3

    // Start key interceptor on a dedicated OS thread (rdev blocks)
    let intercept_state = Arc::clone(&state);
    std::thread::spawn(move || {
        if let Err(e) = intercept::run(intercept_state) {
            tracing::error!("Key interceptor failed: {e}");
        }
    });

    // Build and serve the HTTP app
    let app = web::router(Arc::clone(&state));
    let addr = SocketAddr::from(([127, 0, 0, 1], args.port));
    info!("Dualie UI → http://localhost:{}", args.port);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
