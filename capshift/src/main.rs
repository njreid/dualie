use anyhow::Result;
use clap::Parser;

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

    tracing::info!("capshift starting (skeleton — no interception wired up yet)");
    Ok(())
}
