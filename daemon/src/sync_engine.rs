/// sync_engine.rs — Pure file sync logic: local-section guards, LWW, conflicts.
///
/// # Three-way merge model
///
/// Each config file is conceptually split into two layers:
///
/// - **Global** — the shared settings that should be identical across all machines.
/// - **Local sections** — machine-specific overrides wrapped in guard comments,
///   never transmitted.
///
/// When a remote file arrives, *both* the local and remote files are parsed.
/// LWW comparison is done on the global views only.  The winning global is then
/// reconstructed with the *receiver's* local sections re-inserted at matching
/// placeholder positions:
///
/// ```text
///   remote_raw → parse → remote.global ─────────────────┐
///                                                        │  reconstruct
///   local_raw  → parse → local.global  → LWW compare    ├─────────────→ result
///                      ↘ local.local_sections ───────────┘
/// ```
///
/// This means two machines can run the same app with different machine-specific
/// settings (clipboard commands, GPU backends, local paths) and still converge
/// on the same shared config.
///
/// # Local-section guards
///
/// Lines between `{comment} dualie:local-start` and `{comment} dualie:local-end`
/// are machine-specific.  The comment character matches the target language:
/// `#` for TOML/YAML/Python, `//` for KDL/JSON5/Rust, `;` for INI.
///
/// JSON (no comment syntax) should use `comment_char = ""` which disables guards
/// entirely — the file is synced as a plain LWW blob.
///
/// ## Example — Zellij TOML (comment char `#`)
///
/// ```text
/// theme = "catppuccin-mocha"
/// default_shell = "/usr/bin/fish"
/// # dualie:local-start
/// copy_command = "xclip"     # Linux only
/// copy_clipboard = "primary"
/// # dualie:local-end
/// mouse_mode = true
/// ```
///
/// Machine B (macOS) has `copy_command = "pbcopy"` in its local section.
/// When B's file (newer) wins the LWW race, machine A's result is:
/// `theme` updated to B's value, but `copy_command` stays `"xclip"`.
///
/// ## Example — KDL (comment char `//`)
///
/// ```text
/// // dualie:local-start
/// load_plugins { url "file:///home/njr/.local/zjstatus.wasm" }
/// // dualie:local-end
/// theme "nord"
/// ```
///
/// # Sync flow
///
/// 1. Receiver calls `apply_remote(local_raw, local_mtime, remote_raw, remote_mtime, cc)`.
/// 2. Both files are parsed; globals compared.  If identical → `Winner::Identical`.
/// 3. Timestamps compared (with `TIMESTAMP_TOLERANCE_MS` window).
/// 4. Remote newer → `Winner::Remote`: reconstruct with receiver's local sections.
/// 5. Local newer  → `Winner::Local`: no write.
/// 6. Within tolerance AND content differs → `Winner::Conflict`: append remote's
///    global as a comment block; save `.dualie-conflict` backup of the local file.
///
/// # Conflict output
///
/// ```text
/// theme = "dark"                    ← receiver keeps its current global
///
/// # dualie:conflict-start (other machine — kept for reference)
/// # theme = "light"
/// # dualie:conflict-end
/// ```

use std::time::SystemTime;

// ── Guard marker constants ────────────────────────────────────────────────────

pub const GUARD_LOCAL_START: &str = "dualie:local-start";
pub const GUARD_LOCAL_END:   &str = "dualie:local-end";
pub const GUARD_LOCAL_LINE:  &str = "dualie:local";

const GUARD_PLACEHOLDER_PREFIX: &str = "dualie:local-placeholder-";
const GUARD_CONFLICT_START:     &str = "dualie:conflict-start";
const GUARD_CONFLICT_END:       &str = "dualie:conflict-end";

/// Tolerance window: if timestamps are within this many milliseconds, treat
/// the files as having the same modification time and declare a conflict.
pub const TIMESTAMP_TOLERANCE_MS: u128 = 1_000;

// ── ParsedFile ────────────────────────────────────────────────────────────────

/// A file parsed into a transmittable global view and machine-local sections.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedFile {
    /// Content with every local section replaced by a placeholder comment line.
    /// This is the content compared/transmitted across machines.
    pub global: String,

    /// Extracted local sections, keyed by their sequential placeholder ID (0, 1, …).
    /// Each value is the raw lines that were between the guard markers,
    /// *without* the `local-start` / `local-end` markers themselves.
    pub local_sections: Vec<String>,
}

/// Parse `content` into a `ParsedFile`, stripping local sections and replacing
/// them with numbered placeholder comments using `comment_char`.
///
/// If `comment_char` is empty, local-section guards are not recognised
/// (the file is returned as-is with an empty `local_sections` list).
pub fn parse_file(content: &str, comment_char: &str) -> ParsedFile {
    if comment_char.is_empty() {
        return ParsedFile {
            global:         content.to_owned(),
            local_sections: Vec::new(),
        };
    }

    let local_start_marker = format!("{comment_char} {GUARD_LOCAL_START}");
    let local_end_marker   = format!("{comment_char} {GUARD_LOCAL_END}");
    let local_line_marker  = format!("{comment_char} {GUARD_LOCAL_LINE}");

    let mut global_lines = Vec::new();
    let mut local_sections: Vec<String> = Vec::new();
    let mut current_local: Option<Vec<String>> = None;

    for line in content.lines() {
        let trimmed = line.trim();

        if trimmed == local_start_marker.trim() {
            // Begin accumulating a local block.
            current_local = Some(Vec::new());
        } else if trimmed == local_end_marker.trim() {
            // End of local block — emit placeholder.
            if let Some(lines) = current_local.take() {
                let id = local_sections.len();
                local_sections.push(lines.join("\n"));
                global_lines.push(format!(
                    "{comment_char} {GUARD_PLACEHOLDER_PREFIX}{id}"
                ));
            }
        } else if let Some(ref mut local) = current_local {
            // Inside a local block — accumulate.
            local.push(line.to_owned());
        } else if trimmed == local_line_marker.trim() {
            // Single-line guard — skip and emit placeholder.
            // (The next line after the marker is the local line.)
            // We treat the marker line itself as the local content here;
            // the actual line to skip is handled as a "start + immediate end".
            let id = local_sections.len();
            local_sections.push(String::new());
            global_lines.push(format!(
                "{comment_char} {GUARD_PLACEHOLDER_PREFIX}{id}"
            ));
        } else {
            global_lines.push(line.to_owned());
        }
    }

    // Unterminated local block — treat as passthrough (don't lose content).
    if let Some(lines) = current_local {
        for line in lines {
            global_lines.push(line);
        }
    }

    ParsedFile {
        global: global_lines.join("\n"),
        local_sections,
    }
}

/// Reconstruct a full file from a global view and a set of local sections.
///
/// Placeholder lines of the form `{comment_char} dualie:local-placeholder-N`
/// are replaced with the matching local section wrapped in guard markers.
/// Placeholders without a matching section are removed.
pub fn reconstruct(global: &str, local_sections: &[String], comment_char: &str) -> String {
    if comment_char.is_empty() {
        return global.to_owned();
    }

    let mut lines = Vec::new();
    for line in global.lines() {
        let trimmed = line.trim();
        if let Some(id_str) = trimmed
            .strip_prefix(comment_char)
            .and_then(|s| s.trim().strip_prefix(GUARD_PLACEHOLDER_PREFIX))
        {
            if let Ok(id) = id_str.parse::<usize>() {
                if let Some(section) = local_sections.get(id) {
                    lines.push(format!("{comment_char} {GUARD_LOCAL_START}"));
                    if !section.is_empty() {
                        lines.push(section.clone());
                    }
                    lines.push(format!("{comment_char} {GUARD_LOCAL_END}"));
                    continue;
                }
            }
            // Placeholder with no matching section — drop silently.
        } else {
            lines.push(line.to_owned());
        }
    }
    lines.join("\n")
}

// ── LWW sync ──────────────────────────────────────────────────────────────────

/// Outcome of applying a remote file to a local file.
#[derive(Debug)]
pub struct SyncOutcome {
    /// The content to write to disk on the receiving machine.
    pub content: String,
    /// If `Some`, a `.dualie-conflict` file should be written with this content
    /// (the full pre-sync local file, before any changes).
    pub conflict_backup: Option<String>,
    /// Which machine won.
    pub winner: Winner,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Winner {
    /// Local file was newer — no write needed.
    Local,
    /// Remote file was newer — local should be overwritten.
    Remote,
    /// Timestamps were within tolerance AND content differed — conflict.
    Conflict,
    /// Both global contents were identical — nothing to do.
    Identical,
}

/// Apply a remote file update using last-write-wins semantics with three-way merge.
///
/// Both the local and remote raw file contents are parsed.  LWW comparison is
/// performed on the stripped global views.  If remote wins, the result is the
/// remote's global reconstructed with the *receiver's* local sections — so
/// machine-specific settings (clipboard commands, GPU backends, …) survive
/// even when the remote's shared settings win.
///
/// Parameters:
/// - `local_raw`   — full raw content of the local file on disk.
/// - `local_mtime` — modification time of the local file.
/// - `remote_raw`  — full raw content received from the remote machine.
/// - `remote_mtime`— modification time of the remote file.
/// - `comment_char`— line-comment prefix for guard markers (`"#"`, `"//"`, …).
///                   Pass `""` to disable guard parsing (e.g. for JSON).
pub fn apply_remote(
    local_raw:   &str,
    local_mtime: SystemTime,
    remote_raw:  &str,
    remote_mtime: SystemTime,
    comment_char: &str,
) -> SyncOutcome {
    let local_parsed  = parse_file(local_raw,  comment_char);
    let remote_parsed = parse_file(remote_raw, comment_char);

    // If global content is identical, nothing to do regardless of timestamps.
    if local_parsed.global == remote_parsed.global {
        return SyncOutcome {
            content:         local_raw.to_owned(),
            conflict_backup: None,
            winner:          Winner::Identical,
        };
    }

    let local_ms  = mtime_ms(local_mtime);
    let remote_ms = mtime_ms(remote_mtime);
    let delta     = local_ms.abs_diff(remote_ms);

    if delta <= TIMESTAMP_TOLERANCE_MS {
        // Ambiguous — declare a conflict.  Keep our global, append remote's as a comment.
        let conflict_comment = wrap_conflict_block(&remote_parsed.global, comment_char);
        let merged = format!("{}\n\n{conflict_comment}", local_parsed.global);
        let content = reconstruct(&merged, &local_parsed.local_sections, comment_char);
        return SyncOutcome {
            content,
            conflict_backup: Some(local_raw.to_owned()),
            winner:          Winner::Conflict,
        };
    }

    if local_ms >= remote_ms {
        // Local is newer — nothing to do.
        return SyncOutcome {
            content:         local_raw.to_owned(),
            conflict_backup: None,
            winner:          Winner::Local,
        };
    }

    // Remote is newer — merge remote's global with our local sections.
    let content = reconstruct(&remote_parsed.global, &local_parsed.local_sections, comment_char);
    SyncOutcome {
        content,
        conflict_backup: None,
        winner:          Winner::Remote,
    }
}

// ── Conflict comment block ────────────────────────────────────────────────────

/// Wrap `content` in conflict guard comments using `comment_char`.
///
/// Each line of `content` is prefixed with `{comment_char} ` so the block
/// is inert in the target config language.
pub fn wrap_conflict_block(content: &str, comment_char: &str) -> String {
    if comment_char.is_empty() {
        return String::new();
    }
    let mut out = Vec::new();
    out.push(format!("{comment_char} {GUARD_CONFLICT_START} (other machine — kept for reference)"));
    for line in content.lines() {
        if line.is_empty() {
            out.push(comment_char.to_owned());
        } else {
            out.push(format!("{comment_char} {line}"));
        }
    }
    out.push(format!("{comment_char} {GUARD_CONFLICT_END}"));
    out.join("\n")
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn mtime_ms(t: SystemTime) -> u128 {
    t.duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    fn t(secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(secs)
    }

    // ── parse_file ────────────────────────────────────────────────────────────

    #[test]
    fn parse_no_guards() {
        let content = "theme = \"dark\"\nfont = 12\n";
        let pf = parse_file(content, "#");
        assert_eq!(pf.global, content.trim_end_matches('\n'));
        assert!(pf.local_sections.is_empty());
    }

    #[test]
    fn parse_single_block() {
        let content = "a = 1\n# dualie:local-start\nlocal_line\n# dualie:local-end\nb = 2";
        let pf = parse_file(content, "#");
        assert_eq!(pf.local_sections.len(), 1);
        assert_eq!(pf.local_sections[0], "local_line");
        assert!(pf.global.contains("# dualie:local-placeholder-0"));
        assert!(!pf.global.contains("local_line"));
    }

    #[test]
    fn parse_multiple_blocks() {
        let content = "\
# dualie:local-start
block0
# dualie:local-end
middle
# dualie:local-start
block1_line1
block1_line2
# dualie:local-end
end";
        let pf = parse_file(content, "#");
        assert_eq!(pf.local_sections.len(), 2);
        assert_eq!(pf.local_sections[0], "block0");
        assert_eq!(pf.local_sections[1], "block1_line1\nblock1_line2");
    }

    #[test]
    fn parse_empty_comment_char_passthrough() {
        let content = "// dualie:local-start\nshould remain\n// dualie:local-end\n";
        let pf = parse_file(content, "");
        assert_eq!(pf.global, content);
        assert!(pf.local_sections.is_empty());
    }

    #[test]
    fn parse_double_slash_comment() {
        let content = "a = 1\n// dualie:local-start\nlocal!\n// dualie:local-end\nb = 2";
        let pf = parse_file(content, "//");
        assert_eq!(pf.local_sections[0], "local!");
        assert!(pf.global.contains("// dualie:local-placeholder-0"));
    }

    // ── reconstruct ──────────────────────────────────────────────────────────

    #[test]
    fn reconstruct_roundtrip() {
        let original = "a = 1\n# dualie:local-start\nmy_local\n# dualie:local-end\nb = 2";
        let pf = parse_file(original, "#");
        let rebuilt = reconstruct(&pf.global, &pf.local_sections, "#");
        assert_eq!(rebuilt, original);
    }

    #[test]
    fn reconstruct_drops_unknown_placeholder() {
        // If the remote sends a placeholder but we have no matching section, it's removed.
        let global = "a = 1\n# dualie:local-placeholder-0\nb = 2";
        let result = reconstruct(global, &[], "#");
        assert!(!result.contains("placeholder"));
        assert!(!result.contains("local-start"));
    }

    #[test]
    fn reconstruct_preserves_foreign_local_sections() {
        // Remote global has no placeholders; we still keep our local sections
        // but they'll be dropped since there are no matching placeholders.
        let remote_global = "theme = \"dark\"";
        let local_sections = vec!["gpu = \"amd\"".to_owned()];
        let result = reconstruct(remote_global, &local_sections, "#");
        // No placeholder in remote_global → local section is lost (expected).
        assert_eq!(result, "theme = \"dark\"");
    }

    // ── apply_remote ─────────────────────────────────────────────────────────

    #[test]
    fn remote_newer_wins() {
        let local  = "theme = old";
        let remote = "theme = new";
        let out = apply_remote(local, t(100), remote, t(200), "#");
        assert_eq!(out.winner, Winner::Remote);
        assert_eq!(out.content, "theme = new");
        assert!(out.conflict_backup.is_none());
    }

    #[test]
    fn local_newer_wins() {
        let local  = "theme = old";
        let remote = "theme = new";
        let out = apply_remote(local, t(200), remote, t(100), "#");
        assert_eq!(out.winner, Winner::Local);
        assert_eq!(out.content, "theme = old");
    }

    #[test]
    fn identical_global_is_noop() {
        let content = "theme = same";
        let out = apply_remote(content, t(100), content, t(200), "#");
        assert_eq!(out.winner, Winner::Identical);
    }

    #[test]
    fn conflict_saves_backup_and_appends_comment() {
        let local  = "theme = light";
        let remote = "theme = dark";
        // Same timestamp → conflict
        let out = apply_remote(local, t(100), remote, t(100), "#");
        assert_eq!(out.winner, Winner::Conflict);
        assert_eq!(out.conflict_backup.as_deref(), Some("theme = light"));
        assert!(out.content.contains("# dualie:conflict-start"));
        assert!(out.content.contains("theme = dark"));
        assert!(out.content.contains("# dualie:conflict-end"));
    }

    #[test]
    fn remote_wins_but_keeps_local_sections() {
        // Both machines have a local section.  Remote (macOS) uses pbcopy;
        // local (Linux) uses xclip.  Remote wins the LWW race.
        // Expected: shared setting updated to remote's value; local clipboard preserved.
        let local = "\
setting = old
# dualie:local-start
clipboard = xclip
# dualie:local-end
font = mono";

        let remote = "\
setting = new
# dualie:local-start
clipboard = pbcopy
# dualie:local-end
font = mono";

        let out = apply_remote(local, t(100), remote, t(200), "#");
        assert_eq!(out.winner, Winner::Remote);
        assert!(out.content.contains("setting = new"),  "shared setting should update");
        assert!(out.content.contains("clipboard = xclip"), "local section must be preserved");
        assert!(out.content.contains("# dualie:local-start"));
        assert!(!out.content.contains("pbcopy"), "remote's local section must not appear");
    }

    #[test]
    fn local_wins_keeps_its_own_local_sections() {
        let local = "\
theme = dark
# dualie:local-start
gpu = vulkan
# dualie:local-end";

        let remote = "\
theme = light
# dualie:local-start
gpu = metal
# dualie:local-end";

        let out = apply_remote(local, t(200), remote, t(100), "#");
        assert_eq!(out.winner, Winner::Local);
        assert_eq!(out.content, local);
    }

    // ── wrap_conflict_block ───────────────────────────────────────────────────

    #[test]
    fn conflict_block_prefixes_all_lines() {
        let content = "line1\nline2\nline3";
        let block = wrap_conflict_block(content, "#");
        assert!(block.starts_with("# dualie:conflict-start"));
        assert!(block.contains("# line1"));
        assert!(block.contains("# line2"));
        assert!(block.ends_with("# dualie:conflict-end"));
    }

    #[test]
    fn conflict_block_empty_comment_is_empty() {
        let block = wrap_conflict_block("content", "");
        assert!(block.is_empty());
    }

    // ── Real-world config scenarios ───────────────────────────────────────────

    /// Zellij config.kdl — KDL format, comment char `//`.
    ///
    /// Machine A (Linux desktop): local section contains a Wayland clipboard plugin.
    /// Machine B (macOS laptop): updates `theme` to "nord" (newer mtime).
    /// Result: A gets `theme = "nord"` but keeps its Wayland plugin local section.
    #[test]
    fn zellij_kdl_remote_updates_theme_preserves_local_plugin() {
        let local_a = r#"theme "catppuccin-mocha"
default_shell "/usr/bin/fish"
mouse_mode true
pane_frames false

// dualie:local-start
load_plugins {
    url "file:///home/njr/.local/share/zellij/plugins/zjstatus.wasm"
}
// dualie:local-end

scroll_buffer_size 10000"#;

        // Machine B has updated theme and its own local section (macOS paths).
        let remote_b = r#"theme "nord"
default_shell "/usr/bin/fish"
mouse_mode true
pane_frames false

// dualie:local-start
load_plugins {
    url "file:///Users/njr/Library/Application Support/zellij/zjstatus.wasm"
}
// dualie:local-end

scroll_buffer_size 10000"#;

        let out = apply_remote(local_a, t(1000), remote_b, t(2000), "//");

        assert_eq!(out.winner, Winner::Remote);
        assert!(out.content.contains(r#"theme "nord""#),
            "shared theme should update to B's value");
        assert!(out.content.contains("zjstatus.wasm"),
            "local plugin section should be present");
        assert!(out.content.contains("/home/njr/"),
            "local plugin path must be A's path, not B's");
        assert!(!out.content.contains("Application Support"),
            "B's macOS path must not appear");
        assert!(out.content.contains("// dualie:local-start"));
        assert!(out.conflict_backup.is_none());
    }

    /// Zellij config.toml — TOML format, comment char `#`.
    ///
    /// Both machines edit the theme at approximately the same time (within the
    /// 1-second conflict window).  Both have machine-local clipboard sections.
    /// Result: conflict — local content kept, remote's global appended as comments,
    /// `.dualie-conflict` backup written.
    #[test]
    fn zellij_toml_conflict_simultaneous_theme_edit() {
        let local_linux = "\
theme = \"catppuccin-mocha\"
default_shell = \"/usr/bin/fish\"
mouse_mode = true
# dualie:local-start
copy_command = \"xclip\"
copy_clipboard = \"primary\"
# dualie:local-end
pane_frames = false";

        let remote_macos = "\
theme = \"tokyo-night\"
default_shell = \"/usr/bin/fish\"
mouse_mode = true
# dualie:local-start
copy_command = \"pbcopy\"
copy_clipboard = \"system\"
# dualie:local-end
pane_frames = false";

        // Within the 1-second tolerance window → conflict
        let out = apply_remote(local_linux, t(1000), remote_macos, t(1000), "#");

        assert_eq!(out.winner, Winner::Conflict);
        assert!(out.conflict_backup.is_some(),
            "conflict backup must be saved");
        assert_eq!(out.conflict_backup.as_deref(), Some(local_linux));

        // Local content is kept as the primary
        assert!(out.content.contains("catppuccin-mocha"),
            "local theme must appear in primary content");
        // Remote content appended as a comment block
        assert!(out.content.contains("# dualie:conflict-start"));
        assert!(out.content.contains("# theme = \"tokyo-night\""),
            "remote theme must appear in conflict block");
        assert!(out.content.contains("# dualie:conflict-end"));

        // Neither machine's local clipboard section should pollute the other
        assert!(out.content.contains("copy_command = \"xclip\""),
            "local clipboard section preserved");
        assert!(!out.content.contains("pbcopy"),
            "remote clipboard must not bleed through");
    }

    /// Helix config.toml — TOML, same comment char `#`.
    ///
    /// Remote adds a new `[editor.whitespace]` section.  Local has a local section
    /// with a machine-specific LSP path.  Remote is newer.
    /// Result: new section added, local LSP path preserved.
    #[test]
    fn helix_toml_remote_adds_section_preserves_local_lsp() {
        let local = "\
theme = \"catppuccin_mocha\"

[editor]
line-number = \"relative\"
mouse = false

# dualie:local-start
[language-server.rust-analyzer]
command = \"/home/njr/.rustup/toolchains/stable-x86_64/bin/rust-analyzer\"
# dualie:local-end";

        let remote = "\
theme = \"catppuccin_mocha\"

[editor]
line-number = \"relative\"
mouse = false

# dualie:local-start
[language-server.rust-analyzer]
command = \"/Users/njr/.rustup/toolchains/stable-aarch64/bin/rust-analyzer\"
# dualie:local-end

[editor.whitespace.render]
space = \"all\"
tab = \"all\"";

        let out = apply_remote(local, t(100), remote, t(200), "#");

        assert_eq!(out.winner, Winner::Remote);
        assert!(out.content.contains("[editor.whitespace.render]"),
            "new section from remote must appear");
        assert!(out.content.contains("x86_64"),
            "local LSP path (x86_64) must be preserved");
        assert!(!out.content.contains("aarch64"),
            "remote LSP path (aarch64) must not appear");
    }

    /// VSCode settings.json — JSON has no comment syntax.
    ///
    /// With `comment_char = ""`, guards are disabled entirely.  The file is synced
    /// as a plain LWW blob — the newer version wins unconditionally with no
    /// local-section preservation.
    #[test]
    fn json_no_comment_char_pure_lww_remote_wins() {
        let local = r#"{
    "editor.fontSize": 14,
    "editor.theme": "One Dark Pro",
    "terminal.shell": "/usr/bin/bash"
}"#;

        let remote = r#"{
    "editor.fontSize": 16,
    "editor.theme": "Dracula",
    "terminal.shell": "/usr/bin/fish"
}"#;

        let out = apply_remote(local, t(100), remote, t(200), "");
        assert_eq!(out.winner, Winner::Remote);
        // Full remote content replaces local — no partial merge
        assert_eq!(out.content, remote);
        assert!(out.conflict_backup.is_none());
    }

    #[test]
    fn json_no_comment_char_pure_lww_local_wins() {
        let local  = r#"{"editor.fontSize": 16}"#;
        let remote = r#"{"editor.fontSize": 14}"#;

        let out = apply_remote(local, t(200), remote, t(100), "");
        assert_eq!(out.winner, Winner::Local);
        assert_eq!(out.content, local);
    }

    /// tmux.conf — shell-style comments (`#`), multiple local sections.
    ///
    /// Machine A (Linux) has two local sections: one for clipboard integration
    /// (xclip) and one for the Linux-specific status bar.  Remote B (macOS)
    /// updates the prefix key and adds a new global binding.  Both have two
    /// local sections in matching positions.
    #[test]
    fn tmux_conf_multiple_local_sections_two_positions() {
        let local_linux = "\
set -g prefix C-a
unbind C-b

# dualie:local-start
bind -T copy-mode-vi y send-keys -X copy-pipe-and-cancel \"xclip -selection clipboard\"
# dualie:local-end

set -g mouse on
set -g base-index 1

# dualie:local-start
set -g status-right '#(~/.config/tmux/linux-status.sh)'
# dualie:local-end

set -g history-limit 50000";

        let remote_macos = "\
set -g prefix C-Space
unbind C-b

# dualie:local-start
bind -T copy-mode-vi y send-keys -X copy-pipe-and-cancel \"pbcopy\"
# dualie:local-end

set -g mouse on
set -g base-index 1

# dualie:local-start
set -g status-right '#(~/.config/tmux/macos-status.sh)'
# dualie:local-end

set -g history-limit 100000";

        let out = apply_remote(local_linux, t(500), remote_macos, t(1500), "#");

        assert_eq!(out.winner, Winner::Remote);
        // Shared settings updated from remote
        assert!(out.content.contains("set -g prefix C-Space"), "prefix must update");
        assert!(out.content.contains("history-limit 100000"),  "history limit must update");
        // Both of Linux's local sections preserved
        assert!(out.content.contains("xclip"),             "clipboard section preserved");
        assert!(out.content.contains("linux-status.sh"),   "status section preserved");
        // macOS local content must not appear
        assert!(!out.content.contains("pbcopy"),           "pbcopy must not appear");
        assert!(!out.content.contains("macos-status.sh"),  "macos status must not appear");
    }
}
