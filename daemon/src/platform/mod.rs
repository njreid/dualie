use anyhow::Result;
use serde::Serialize;

#[cfg(target_os = "macos")]
mod mac;
#[cfg(target_os = "linux")]
mod linux;

/// A discovered application the user can bind to a virtual action.
#[derive(Debug, Clone, Serialize)]
pub struct AppEntry {
    /// Platform-specific identifier (bundle ID on Mac, .desktop name on Linux)
    pub id:    String,
    /// Human-readable display name
    pub name:  String,
    /// Absolute path to the app icon, if available
    pub icon:  Option<String>,
}

/// Launch an app by its platform ID.
/// Returns Ok(()) if the launch command was issued (not necessarily successful).
pub fn launch_app(app_id: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    return mac::launch_app(app_id);
    #[cfg(target_os = "linux")]
    return linux::launch_app(app_id);
    #[allow(unreachable_code)]
    Err(anyhow::anyhow!("app launch not supported on this platform"))
}

/// Return a list of installed applications for the UI dropdown.
pub fn list_apps() -> Result<Vec<AppEntry>> {
    #[cfg(target_os = "macos")]
    return mac::list_apps();
    #[cfg(target_os = "linux")]
    return linux::list_apps();
    #[allow(unreachable_code)]
    Ok(vec![])
}

/// Basic system information returned by GET /api/v1/platform/info
#[derive(Debug, Serialize)]
pub struct SystemInfo {
    pub os:      &'static str,
    pub version: String,
    pub arch:    &'static str,
}

pub fn system_info() -> SystemInfo {
    SystemInfo {
        os:      std::env::consts::OS,
        arch:    std::env::consts::ARCH,
        version: os_version(),
    }
}

fn os_version() -> String {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("sw_vers")
            .arg("-productVersion")
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
            .unwrap_or_default()
    }
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string("/etc/os-release")
            .unwrap_or_default()
            .lines()
            .find(|l| l.starts_with("PRETTY_NAME="))
            .map(|l| l.trim_start_matches("PRETTY_NAME=").trim_matches('"').to_owned())
            .unwrap_or_default()
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    String::new()
}
