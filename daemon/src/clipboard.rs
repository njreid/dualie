/// clipboard.rs — OS clipboard read/write via arboard.
///
/// arboard supports X11, Wayland (via wl-clipboard / data-control protocol),
/// and macOS.  All calls are synchronous and run in a blocking thread context;
/// never call these from async code without `spawn_blocking`.

use anyhow::{Context, Result};

/// Write `text` to the system clipboard.
pub fn write_text(text: &str) -> Result<()> {
    let mut ctx = arboard::Clipboard::new().context("opening clipboard")?;
    ctx.set_text(text).context("writing to clipboard")?;
    Ok(())
}

/// Read text from the system clipboard.
pub fn read_text() -> Result<String> {
    let mut ctx = arboard::Clipboard::new().context("opening clipboard")?;
    ctx.get_text().context("reading from clipboard")
}
