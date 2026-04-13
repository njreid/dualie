/// git_sync.rs — Git-backed config versioning and cloud sync.
///
/// Maintains a clone of the user's config repo at the platform data dir.
/// The repo uses the layout `appname/filename` (e.g. `dualie/dualie.kdl`)
/// so other app configs can coexist in the same repo.
///
/// Platform repo locations:
///   Linux:  $XDG_DATA_HOME/dualie/repo/   (~/.local/share/dualie/repo/)
///   macOS:  ~/Library/Application Support/dualie/repo/
///
/// # Flow
///
/// Auto-commit (triggered on any local config write):
///   live dualie.kdl → copy → <repo>/dualie/dualie.kdl → git add + commit
///
/// Pull (user-initiated from TUI):
///   git pull --rebase origin main → copy → <repo>/dualie/dualie.kdl → live dualie.kdl
///   notify fires → daemon hot-reloads config
///
/// Push (user-initiated from TUI):
///   git push origin main
///
/// The auto-commit is idempotent: if the repo file already matches HEAD,
/// `git diff --cached --quiet` exits 0 and no commit is made.  This prevents
/// the pull path from generating a spurious commit when notify fires after a pull.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use anyhow::{Context, Result};
use once_cell::sync::OnceCell;
use tokio::process::Command;
use tokio::sync::mpsc;
use tracing::{info, warn};

// ── Global state ──────────────────────────────────────────────────────────────

/// Commits on `origin/main` not yet pulled locally.
/// Updated after every successful `git fetch`.
pub static GIT_PENDING: AtomicU32 = AtomicU32::new(0);

/// Absolute path of the git repo directory, set once at daemon startup.
/// Read by the status socket so the TUI can run git commands directly.
pub static REPO_DIR: OnceCell<std::path::PathBuf> = OnceCell::new();

static COMMIT_TX: OnceCell<mpsc::Sender<()>> = OnceCell::new();

// ── Platform path ─────────────────────────────────────────────────────────────

/// Default location for the git repo clone on this platform.
///
/// Linux:  `~/.local/share/dualie/repo/`
/// macOS:  `~/Library/Application Support/dualie/repo/`
pub fn default_repo_dir() -> PathBuf {
    directories::ProjectDirs::from("dev", "dualie", "dualie")
        .expect("could not determine data directory")
        .data_dir()
        .join("repo")
}

// ── GitRepo ───────────────────────────────────────────────────────────────────

/// A handle to the dualie config git repository.
pub struct GitRepo {
    /// Root of the git repo clone (the directory that contains `.git/`).
    repo_dir:     PathBuf,
    /// The live config file the daemon reads — platform path for `dualie.kdl`.
    config_path:  PathBuf,
    /// Machine name embedded in auto-commit messages.
    machine_name: String,
}

impl GitRepo {
    pub fn new(repo_dir: PathBuf, config_path: PathBuf, machine_name: String) -> Self {
        Self { repo_dir, config_path, machine_name }
    }

    /// Root of the repo (for display / logging).
    pub fn repo_dir(&self) -> &std::path::Path { &self.repo_dir }

    /// Path of `dualie/dualie.kdl` inside the repo.
    fn repo_kdl(&self) -> PathBuf {
        self.repo_dir.join("dualie").join("dualie.kdl")
    }

    /// Run `git -C <repo_dir> <args…>` and return the raw output.
    async fn git(&self, args: &[&str]) -> Result<std::process::Output> {
        Command::new("git")
            .arg("-C").arg(&self.repo_dir)
            .args(args)
            .output()
            .await
            .with_context(|| format!("git {}", args.join(" ")))
    }

    /// Initialise the repo if it has no `.git` directory yet.
    /// Creates the `dualie/` subdirectory and writes an initial empty commit
    /// so that HEAD always exists.
    pub async fn open_or_init(&self) -> Result<()> {
        tokio::fs::create_dir_all(self.repo_dir.join("dualie")).await
            .context("creating repo/dualie/ dir")?;

        let check = self.git(&["rev-parse", "--git-dir"]).await;
        let initialised = check.map(|o| o.status.success()).unwrap_or(false);

        if !initialised {
            let out = self.git(&["init", "-b", "main"]).await?;
            if !out.status.success() {
                anyhow::bail!("git init failed: {}", String::from_utf8_lossy(&out.stderr));
            }
            self.ensure_gitignore().await?;
            // Seed a config for author identity so the initial commit works.
            self.git(&["config", "user.email", "dualie@localhost"]).await?;
            self.git(&["config", "user.name",  "Dualie"]).await?;
            let out = self.git(&[
                "commit", "--allow-empty", "-m", "init: dualie config repo",
            ]).await?;
            if !out.status.success() {
                anyhow::bail!("git initial commit failed: {}", String::from_utf8_lossy(&out.stderr));
            }
            info!("git: initialised repo at {}", self.repo_dir.display());
        } else {
            self.ensure_gitignore().await?;
        }

        Ok(())
    }

    /// Ensure `<repo>/.gitignore` excludes `dualie/local.kdl` and conflict files.
    /// A no-op if the file already contains the right entry.
    pub async fn ensure_gitignore(&self) -> Result<()> {
        let path = self.repo_dir.join(".gitignore");
        let existing = tokio::fs::read_to_string(&path).await.unwrap_or_default();
        if existing.contains("dualie/local.kdl") {
            return Ok(());
        }
        let content = format!(
            "{existing}\n# dualie: machine-local config (never committed)\ndualie/local.kdl\n*.dualie-conflict\n"
        );
        tokio::fs::write(&path, content.trim_start()).await
            .context("writing repo .gitignore")?;
        Ok(())
    }

    /// Set (or update) the `origin` remote URL.
    pub async fn set_remote(&self, url: &str) -> Result<()> {
        let add = self.git(&["remote", "add", "origin", url]).await?;
        if add.status.success() {
            return Ok(());
        }
        // Already exists — update it.
        let out = self.git(&["remote", "set-url", "origin", url]).await?;
        if !out.status.success() {
            anyhow::bail!(
                "git remote set-url failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        Ok(())
    }

    /// Copy the live config into the repo, `git add`, and commit if anything changed.
    ///
    /// Returns `true` if a commit was made, `false` if the working tree was clean.
    pub async fn auto_commit(&self) -> Result<bool> {
        if !self.config_path.exists() {
            return Ok(false);
        }

        tokio::fs::create_dir_all(self.repo_dir.join("dualie")).await?;
        tokio::fs::copy(&self.config_path, self.repo_kdl()).await
            .context("copying config to repo")?;

        self.git(&["add", "dualie/dualie.kdl"]).await?;

        // Exit 0 means nothing staged → nothing to commit.
        let diff = self.git(&["diff", "--cached", "--quiet"]).await?;
        if diff.status.success() {
            return Ok(false);
        }

        let msg = format!("auto: {}", self.machine_name);
        let out = self.git(&["commit", "-m", &msg]).await?;
        if !out.status.success() {
            anyhow::bail!("git commit failed: {}", String::from_utf8_lossy(&out.stderr));
        }
        info!("git: committed config for {}", self.machine_name);
        Ok(true)
    }

    /// `git fetch origin` — updates the remote-tracking branch without changing HEAD.
    pub async fn fetch(&self) -> Result<()> {
        let out = self.git(&["fetch", "origin"]).await?;
        if !out.status.success() {
            anyhow::bail!("git fetch failed: {}", String::from_utf8_lossy(&out.stderr));
        }
        Ok(())
    }

    /// Count commits on `origin/main` not yet on `HEAD`.
    /// Returns 0 if the remote branch does not exist yet (fresh repo).
    pub async fn pending_count(&self) -> Result<u32> {
        let out = self.git(&["rev-list", "--count", "HEAD..origin/main"]).await?;
        if !out.status.success() {
            return Ok(0);
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().parse().unwrap_or(0))
    }

    /// `git pull --rebase origin main`.
    ///
    /// On success, copies `<repo>/dualie/dualie.kdl` to the live config path.
    /// The `notify` watcher in `config::watch()` will then fire and hot-reload.
    /// Returns the number of commits applied.
    ///
    /// Currently called by the TUI directly via shell-out rather than through
    /// this method.  Kept for future daemon-IPC routing.
    #[allow(dead_code)]
    pub async fn pull(&self) -> Result<u32> {
        let pending = self.pending_count().await.unwrap_or(0);

        let out = self.git(&["pull", "--rebase", "origin", "main"]).await?;
        if !out.status.success() {
            anyhow::bail!("git pull failed: {}", String::from_utf8_lossy(&out.stderr));
        }

        // Propagate updated repo file → live config path.
        let repo_kdl = self.repo_kdl();
        if repo_kdl.exists() {
            if let Some(parent) = self.config_path.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            tokio::fs::copy(&repo_kdl, &self.config_path).await
                .context("copying pulled config to live path")?;
            info!("git: pull applied {} commit(s)", pending);
        }

        Ok(pending)
    }

    /// `git push origin main`.
    #[allow(dead_code)]
    pub async fn push(&self) -> Result<()> {
        let out = self.git(&["push", "origin", "main"]).await?;
        if !out.status.success() {
            anyhow::bail!("git push failed: {}", String::from_utf8_lossy(&out.stderr));
        }
        info!("git: pushed to origin");
        Ok(())
    }
}

// ── Background task ───────────────────────────────────────────────────────────

/// Spawn the git sync background task.
///
/// Runs an initial `git fetch` at startup to populate `GIT_PENDING`,
/// then sits in a loop processing auto-commit triggers.
/// Also records the repo path in `REPO_DIR` for the status socket.
pub fn spawn(repo: Arc<GitRepo>) {
    REPO_DIR.set(repo.repo_dir.clone()).ok();

    let (tx, mut rx) = mpsc::channel::<()>(32);
    COMMIT_TX.set(tx).ok();

    tokio::spawn(async move {
        // Initial fetch — best-effort; log and continue if the remote isn't set up.
        match repo.fetch().await {
            Ok(()) => {
                match repo.pending_count().await {
                    Ok(n) => {
                        GIT_PENDING.store(n, Ordering::Relaxed);
                        if n > 0 {
                            info!("git: {} commit(s) available to pull", n);
                        }
                    }
                    Err(e) => warn!("git: pending_count: {e}"),
                }
            }
            Err(e) => warn!("git: initial fetch skipped: {e}"),
        }

        // Auto-commit trigger loop.
        while rx.recv().await.is_some() {
            if let Err(e) = repo.auto_commit().await {
                warn!("git: auto_commit: {e}");
            }
        }
    });
}

/// Trigger an auto-commit from any context (async task or blocking thread).
/// Non-blocking: drops the trigger silently if the channel is full.
pub fn trigger_commit() {
    if let Some(tx) = COMMIT_TX.get() {
        let _ = tx.try_send(());
    }
}
