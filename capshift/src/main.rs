use anyhow::Result;
use clap::Parser;
use tracing::info;

mod actions;
mod chord;
mod config;
mod keycodes;

#[cfg(target_os = "macos")]
mod hid;
#[cfg(target_os = "macos")]
mod kvhd;

/// capshift — caps-lock chord shortcut daemon for macOS.
#[derive(Parser, Debug)]
#[command(version, about)]
struct Args {}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(std::env::var("RUST_LOG").unwrap_or_else(|_| "capshift=info".into()))
        .init();

    let _args = Args::parse();

    #[cfg(not(target_os = "macos"))]
    {
        eprintln!("error: capshift only supports macOS");
        std::process::exit(1);
    }

    #[cfg(target_os = "macos")]
    {
        let cfg_rx = config::watch()?;
        info!("config: {}", config::config_path().display());

        std::thread::spawn(move || {
            if let Err(e) = hid::run(cfg_rx) {
                tracing::error!("hid interceptor: {e}");
                std::process::exit(1);
            }
        });

        std::future::pending::<()>().await;
    }

    Ok(())
}
