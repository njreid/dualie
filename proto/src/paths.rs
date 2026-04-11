use std::path::PathBuf;

// ── Platform-aware path helpers ───────────────────────────────────────────────

/// Return the platform config directory for the Dualie application.
///
/// | Platform | Path                                          |
/// |----------|-----------------------------------------------|
/// | Linux    | `$XDG_CONFIG_HOME/dualie` or `~/.config/dualie` |
/// | macOS    | `~/Library/Application Support/dev.dualie.dualie` |
/// | Windows  | `%APPDATA%\dualie\dualie\config`              |
pub fn config_dir() -> PathBuf {
    project_dirs().config_dir().to_owned()
}

/// Return the platform data directory for the Dualie application.
pub fn data_dir() -> PathBuf {
    project_dirs().data_dir().to_owned()
}

/// Return the path to the main daemon config file.
pub fn config_file() -> PathBuf {
    config_dir().join("config.json")
}

/// Return the path to the sync-pairs definition file.
pub fn sync_pairs_file() -> PathBuf {
    config_dir().join("sync-pairs.json")
}

/// Return the default "inbox" directory where the remote machine can drop
/// files (analogous to AirDrop).
pub fn inbox_dir() -> PathBuf {
    // On all platforms this lands in the user's home directory.
    home_dir().join("Dualie Inbox")
}

fn project_dirs() -> directories::ProjectDirs {
    directories::ProjectDirs::from("dev", "dualie", "dualie")
        .expect("could not determine platform config directory")
}

fn home_dir() -> PathBuf {
    // `directories` doesn't expose home directly; use std fallback.
    #[allow(deprecated)]
    std::env::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
}
