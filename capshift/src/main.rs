use anyhow::Result;
use clap::{Parser, Subcommand};
#[cfg(target_os = "macos")]
use tracing::info;

mod actions;
mod apps;
mod chord;
mod config;
mod keycodes;
mod kvhd;

#[cfg(target_os = "macos")]
mod hid;

/// capshift — caps-lock chord shortcut daemon for macOS.
#[derive(Parser, Debug)]
#[command(version, about)]
struct Args {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// List running applications as name<TAB>bundle-id.
    #[command(alias = "applications")]
    Apps,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(std::env::var("RUST_LOG").unwrap_or_else(|_| "capshift=info".into()))
        .init();

    let args = Args::parse();

    if let Some(Command::Apps) = args.command {
        apps::print_running()?;
        return Ok(());
    }

    #[cfg(not(target_os = "macos"))]
    {
        eprintln!("error: capshift only supports macOS");
        std::process::exit(1);
    }

    #[cfg(target_os = "macos")]
    {
        let cfg_rx = config::watch()?;
        info!("config: {}", config::config_path()?.display());

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
