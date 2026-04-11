use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// ── Sync-pair configuration ───────────────────────────────────────────────────

/// One directory pair that the sync engine watches and reconciles.
///
/// `local` is an absolute path on *this* machine; the hub maps it to a
/// canonical `name` that identifies the pair across both machines.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncPair {
    /// Stable, human-readable identifier shared by both machines
    /// (e.g. `"ssh"`, `"nvim"`, `"inbox"`).
    pub name:        String,
    /// Absolute local path to the directory being watched.
    pub local:       PathBuf,
    /// If true, sub-directories are included recursively.
    #[serde(default = "default_true")]
    pub recursive:   bool,
    /// Glob patterns to exclude (relative to `local`).
    #[serde(default)]
    pub exclude:     Vec<String>,
}

fn default_true() -> bool { true }

// ── Conflict record ───────────────────────────────────────────────────────────

/// Written alongside a conflicting file as `<filename>.conflict-<timestamp>`.
///
/// The newer (winning) version is written to the original path; the older
/// version is preserved under the `.conflict-*` name so no work is lost.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConflictRecord {
    /// The canonical sync-pair name.
    pub pair:        String,
    /// Relative path within the pair that conflicted.
    pub rel_path:    String,
    /// SHA-256 of the version that was displaced (kept as backup).
    pub displaced_sha256: [u8; 32],
    /// `modified_ms` of the version that was displaced.
    pub displaced_modified_ms: u64,
    /// Machine-id of the machine that "won" (its version is now live).
    pub winner_machine_id: String,
    /// Unix-ms timestamp when the conflict was detected.
    pub detected_ms: u64,
}

impl ConflictRecord {
    /// Return the backup filename suffix, e.g. `.conflict-1713000000000`.
    pub fn suffix(&self) -> String {
        format!(".conflict-{}", self.detected_ms)
    }
}

// ── Sync decision ─────────────────────────────────────────────────────────────

/// What the hub should do after comparing two `SyncEntry` lists.
#[derive(Debug, Clone)]
pub enum SyncDecision {
    /// Local version wins — push to remote.
    PushToRemote { rel_path: String },
    /// Remote version wins — pull from remote.
    PullFromRemote { rel_path: String },
    /// Files are identical — no action needed.
    Identical,
    /// Both sides modified since last sync — LWW, keep a conflict backup.
    Conflict {
        rel_path:      String,
        winner_is_local: bool,
    },
}

/// Compare two `SyncEntry` instances and return the appropriate decision.
pub fn reconcile(
    local:  Option<&crate::protocol::SyncEntry>,
    remote: Option<&crate::protocol::SyncEntry>,
) -> SyncDecision {
    match (local, remote) {
        (Some(l), None) => SyncDecision::PushToRemote { rel_path: l.rel_path.clone() },
        (None, Some(r)) => SyncDecision::PullFromRemote { rel_path: r.rel_path.clone() },
        (None, None)    => SyncDecision::Identical,
        (Some(l), Some(r)) => {
            if l.sha256 == r.sha256 {
                return SyncDecision::Identical;
            }
            // Last-writer-wins: the higher `modified_ms` is canonical.
            SyncDecision::Conflict {
                rel_path:        l.rel_path.clone(),
                winner_is_local: l.modified_ms >= r.modified_ms,
            }
        }
    }
}
