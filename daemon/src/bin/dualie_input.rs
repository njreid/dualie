/// dualie-input — slim root daemon for keyboard capture and remapping.
///
/// Runs as root (required for IOHIDManager exclusive device seize on macOS).
/// All other daemon functionality lives in the unprivileged `dualie` process.
///
/// # Protocol
///
/// Listens on `/var/run/dualie-input.sock` (chmod 0666 so the user daemon can
/// connect without root).  Accepts one client at a time.  On connection:
///   ← ConfigSnapshot(cbor)   set initial config
///   ← SetActiveOutput(u8)    update active output
///   → SwitchOutput(u8)       caps-layer swap/jump fired
///   → FireAction(u8)         virtual action slot fired
///   → ClipPull               clipboard pull requested
///
/// If the client disconnects, the input daemon keeps running and waits for
/// reconnection.  Key events continue to pass through using the last config.

use std::os::unix::net::UnixListener;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use clap::Parser;
use tokio::sync::watch;
use tracing::{error, info, warn};

use dualie::config::DualieConfig;
use dualie::intercept::{self, ActiveOutput};
use dualie::peer::SerialClient;
use dualie_proto::input_proto::{
    FromInput, INPUT_SOCKET,
    decode_to_input, encode_from_input, read_frame, write_frame, ToInput,
};

#[derive(Parser, Debug)]
#[command(name = "dualie-input", version, about = "Dualie root input daemon")]
struct Args {}

#[tokio::main]
async fn main() -> Result<()> {
    let _ = Args::parse();

    tracing_subscriber::fmt()
        .with_env_filter(std::env::var("RUST_LOG").unwrap_or_else(|_| "dualie_input=info,dualie=info".into()))
        .init();

    // ── Config watch channel ──────────────────────────────────────────────────
    let (cfg_tx, cfg_rx) = watch::channel(DualieConfig::default());
    let cfg_tx = Arc::new(Mutex::new(cfg_tx));

    // ── Active output shared state ────────────────────────────────────────────
    let active_output: ActiveOutput = Arc::new(AtomicU8::new(0));

    // ── Channel for forwarding intercept events to the user daemon ────────────
    let (event_tx, event_rx) = std::sync::mpsc::channel::<FromInput>();

    // ── Spawn the intercept thread ────────────────────────────────────────────
    {
        let cfg_rx2        = cfg_rx.clone();
        let active_output2 = active_output.clone();
        // SerialClient::with_event_sender translates DualieMessage side-effects
        // (ActiveOutput, ClipboardPull) into FromInput events over the mpsc.
        let serial_bridge  = SerialClient::with_event_sender(event_tx.clone());

        std::thread::spawn(move || {
            if let Err(e) = intercept::run(cfg_rx2, serial_bridge, active_output2) {
                error!("intercept: {e}");
            }
        });
    }

    // ── Unix socket listener ──────────────────────────────────────────────────
    let _ = std::fs::remove_file(INPUT_SOCKET);
    let listener = UnixListener::bind(INPUT_SOCKET)?;
    std::fs::set_permissions(INPUT_SOCKET, std::os::unix::fs::PermissionsExt::from_mode(0o666))?;
    info!("dualie-input: listening on {INPUT_SOCKET}");

    // ── Event-forwarding thread ───────────────────────────────────────────────
    // Reads FromInput events from the mpsc and writes them to the active client.
    let client_writer: Arc<Mutex<Option<std::os::unix::net::UnixStream>>> =
        Arc::new(Mutex::new(None));
    {
        let client_writer2 = client_writer.clone();
        std::thread::spawn(move || {
            for event in event_rx {
                let frame = match encode_from_input(&event) {
                    Ok(f) => f,
                    Err(e) => { warn!("encode event: {e}"); continue; }
                };
                let mut guard = client_writer2.lock().unwrap();
                if let Some(ref mut stream) = *guard {
                    if write_frame(stream, &frame).is_err() {
                        *guard = None; // client disconnected
                    }
                }
            }
        });
    }

    // ── Accept loop (blocking — run in a dedicated thread) ────────────────────
    let cfg_tx2      = cfg_tx.clone();
    let active2      = active_output.clone();
    let cw           = client_writer.clone();
    tokio::task::spawn_blocking(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => {
                    info!("dualie-input: user daemon connected");

                    match stream.try_clone() {
                        Ok(write_clone) => { *cw.lock().unwrap() = Some(write_clone); }
                        Err(e) => { warn!("stream clone: {e}"); continue; }
                    }

                    let mut reader = stream;
                    loop {
                        let body = match read_frame(&mut reader) {
                            Ok(b) => b,
                            Err(e) => {
                                info!("dualie-input: user daemon disconnected ({e})");
                                *cw.lock().unwrap() = None;
                                break;
                            }
                        };
                        match decode_to_input(&body) {
                            Ok(ToInput::ConfigSnapshot(cbor)) => {
                                match ciborium::from_reader::<DualieConfig, _>(cbor.as_slice()) {
                                    Ok(cfg) => { let _ = cfg_tx2.lock().unwrap().send(cfg); }
                                    Err(e)  => warn!("config decode: {e}"),
                                }
                            }
                            Ok(ToInput::SetActiveOutput(idx)) => {
                                active2.store(idx, Ordering::Relaxed);
                            }
                            Err(e) => warn!("decode: {e}"),
                        }
                    }
                }
                Err(e) => error!("accept: {e}"),
            }
        }
    }).await?;

    Ok(())
}
