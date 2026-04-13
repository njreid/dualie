/// ipc.rs — Synchronous query to the daemon status socket.
///
/// The daemon writes one JSON line per connection and closes immediately.
/// We use a blocking std UnixStream so the TUI doesn't need to be fully async.

use std::os::unix::net::UnixStream;
use std::time::Duration;

use anyhow::{Context, Result};

#[derive(Debug, Clone)]
pub struct DaemonStatus {
    pub version:     String,
    pub config:      String,
    pub serial:      String,   // "connected" | "disconnected"
    pub git_pending: u32,
    pub repo_dir:    String,
    pub pid:         u64,
}

/// Connect to the socket, read the JSON line, return parsed status.
pub fn query_status(socket_path: &str) -> Result<DaemonStatus> {
    let stream = UnixStream::connect(socket_path)
        .with_context(|| format!("connecting to {socket_path}"))?;

    stream.set_read_timeout(Some(Duration::from_secs(2)))?;

    let mut buf = String::new();
    let mut reader = std::io::BufReader::new(stream);
    std::io::BufRead::read_line(&mut reader, &mut buf)?;

    parse_status_json(buf.trim())
}

fn parse_status_json(s: &str) -> Result<DaemonStatus> {
    // Hand-parse the simple flat JSON the daemon emits rather than pulling in serde.
    // Expected format: {"version":"0.1.0","config":"/path","serial":"connected","pid":1234}
    fn extract<'a>(src: &'a str, key: &str) -> Option<&'a str> {
        let needle = format!("\"{key}\":");
        let start = src.find(needle.as_str())? + needle.len();
        let rest = src[start..].trim_start();
        if rest.starts_with('"') {
            // String value
            let inner = &rest[1..];
            let end = inner.find('"')?;
            Some(&inner[..end])
        } else {
            // Numeric value
            let end = rest.find([',', '}']).unwrap_or(rest.len());
            Some(rest[..end].trim())
        }
    }

    Ok(DaemonStatus {
        version:     extract(s, "version").unwrap_or("?").to_owned(),
        config:      extract(s, "config").unwrap_or("?").to_owned(),
        serial:      extract(s, "serial").unwrap_or("disconnected").to_owned(),
        git_pending: extract(s, "git_pending").unwrap_or("0").parse().unwrap_or(0),
        repo_dir:    extract(s, "repo_dir").unwrap_or("").to_owned(),
        pid:         extract(s, "pid").unwrap_or("0").parse().unwrap_or(0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_status() {
        let s = r#"{"version":"0.1.0","config":"/home/user/.config/dualie/dualie.kdl","serial":"connected","git_pending":0,"repo_dir":"","pid":1234}"#;
        let st = parse_status_json(s).unwrap();
        assert_eq!(st.version, "0.1.0");
        assert_eq!(st.serial, "connected");
        assert_eq!(st.pid, 1234);
        assert_eq!(st.git_pending, 0);
    }

    #[test]
    fn parse_git_pending_and_repo_dir() {
        let s = r#"{"version":"0.2.0","config":"/tmp/x.kdl","serial":"connected","git_pending":3,"repo_dir":"/home/user/.local/share/dualie/repo","pid":42}"#;
        let st = parse_status_json(s).unwrap();
        assert_eq!(st.git_pending, 3);
        assert_eq!(st.repo_dir, "/home/user/.local/share/dualie/repo");
    }

    #[test]
    fn parse_disconnected() {
        let s = r#"{"version":"0.2.0","config":"/tmp/x.kdl","serial":"disconnected","git_pending":0,"repo_dir":"","pid":5678}"#;
        let st = parse_status_json(s).unwrap();
        assert_eq!(st.serial, "disconnected");
    }

    #[test]
    fn parse_missing_fields_uses_defaults() {
        // Empty JSON object — all fields missing, should not panic
        let st = parse_status_json("{}").unwrap();
        assert_eq!(st.version, "?");
        assert_eq!(st.serial, "disconnected");
        assert_eq!(st.git_pending, 0);
        assert_eq!(st.repo_dir, "");
        assert_eq!(st.pid, 0);
    }

    #[test]
    fn parse_path_with_spaces_in_config() {
        let s = r#"{"version":"1.0","config":"/home/user/my docs/dualie.kdl","serial":"connected","git_pending":0,"repo_dir":"","pid":99}"#;
        let st = parse_status_json(s).unwrap();
        assert_eq!(st.config, "/home/user/my docs/dualie.kdl");
    }
}
