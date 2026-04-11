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

/// Dualie daemon – serves the config web UI and dispatches virtual key actions.
#[derive(Parser, Debug)]
#[command(version, about)]
struct Args {
    /// Port to listen on for the config web UI
    #[arg(long, default_value_t = 7474)]
    port: u16,

    /// Enable verbose logging and Vite dev proxy (use with `just dev`)
    #[arg(long)]
    dev: bool,

    /// Address of the Dualie hub (Pi Zero 2W), e.g. 10.0.1.1:7475.
    /// If omitted, the daemon runs standalone without hub features.
    #[arg(long)]
    hub: Option<String>,

    /// Stable identifier for this machine sent to the hub (defaults to hostname).
    #[arg(long)]
    machine_id: Option<String>,
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

    // Optionally connect to the hub
    if let Some(hub_addr) = args.hub {
        let id = args.machine_id
            .or_else(|| hostname::get().ok()?.into_string().ok())
            .unwrap_or_else(|| "unknown".to_owned());
        info!("Connecting to hub at {hub_addr} as machine '{id}'");
        let _client = peer::spawn(hub_addr, id);
        // _client is kept alive for the process lifetime; expose via AppState later
    }

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
