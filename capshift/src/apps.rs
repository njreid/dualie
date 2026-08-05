//! Running macOS application discovery for `capshift apps`.

use anyhow::{bail, Result};
#[cfg(target_os = "macos")]
use anyhow::Context;

#[cfg(target_os = "macos")]
pub fn print_running(include_background: bool) -> Result<()> {
    // JXA's Objective-C bridge reads NSWorkspace directly. Unlike asking
    // System Events, this does not require Automation permission.
    const SCRIPT: &str = r#"
ObjC.import('AppKit');
const apps = $.NSWorkspace.sharedWorkspace.runningApplications;
const rows = [];
for (let i = 0; i < apps.count; i++) {
    const app = apps.objectAtIndex(i);
    if (!INCLUDE_BACKGROUND && Number(app.activationPolicy) !== 0) continue;
    const name = ObjC.unwrap(app.localizedName);
    const id = ObjC.unwrap(app.bundleIdentifier);
    if (name && id) rows.push(name + '\t' + id);
}
rows.join('\n');
"#;

    let script = SCRIPT.replace(
        "INCLUDE_BACKGROUND",
        if include_background { "true" } else { "false" },
    );
    let output = std::process::Command::new("osascript")
        .args(["-l", "JavaScript", "-e", &script])
        .output()
        .context("running osascript to query NSWorkspace")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("listing running applications failed: {}", stderr.trim());
    }

    let stdout = String::from_utf8(output.stdout).context("osascript returned non-UTF-8 output")?;
    let mut rows: Vec<&str> = stdout.lines().filter(|line| !line.is_empty()).collect();
    rows.sort_unstable_by_key(|row| row.to_ascii_lowercase());
    rows.dedup();
    for row in rows {
        println!("{row}");
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub fn print_running(_include_background: bool) -> Result<()> {
    bail!("listing running applications is only supported on macOS")
}
