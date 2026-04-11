use anyhow::Result;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::info;

mod peer;
mod state;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("dualie_hub=debug".parse()?),
        )
        .init();

    info!("dualie-hub starting");

    let shared = Arc::new(Mutex::new(state::HubState::default()));

    peer::run_peer_server("0.0.0.0:7475", shared).await?;

    Ok(())
}
