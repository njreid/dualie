# capshift — minimal caps-lock shortcut daemon

Date: 2026-08-03

## Problem

Dualie's existing `daemon` crate implements caps-lock-chord shortcuts (`VirtualAction::AppLaunch` / `ShellCommand`) as part of a much larger KVM system: IOHIDManager device-seize, Karabiner VirtualHIDDevice injection, RP2040 serial peer, git-synced multi-machine config, file sync, clipboard sync. For a single MacBook that just wants "hold Caps Lock + key → focus/launch an app (or run a shell command)", none of that KVM machinery is needed.

`capshift` is a new, independent, macOS-only crate that provides just the caps-lock-chord-to-action behavior, with no dependency on the RP2040 proto, KVM config, or any of the multi-machine sync logic.

## Non-goals

- No Linux/Windows support (macOS only)
- No Karabiner-Elements dependency, no virtual HID device, no key remapping/forwarding
- No multi-machine config sync, no RP2040/serial peer
- No config UI / web server
- No layers, per-output configs, or other KVM-specific config concepts

## Architecture

`capshift` runs as a background daemon that installs a `CGEventTap` in **listen** mode (`kCGEventTapOptionDefault` is not required — a passive/listen tap combined with returning `NULL` from the callback to swallow events is sufficient; no exclusive device seize like the KVM daemon's IOHIDManager approach). It watches for chords of the form: Caps Lock held down + another key pressed.

State machine:
- Caps Lock key-down → enter "chord active" state; **swallow** the event (never toggles caps-lock, never reaches other apps)
- While chord is active, next key-down that matches a configured binding → fire that binding's action, swallow the event
- While chord is active, key-down that does *not* match any binding → pass through unmodified (chord state does not block unrelated typing)
- Caps Lock key-up → exit "chord active" state

Caps Lock is fully repurposed as a modifier; tapping it alone does nothing (no timing/tap-vs-hold disambiguation needed).

This needs macOS Accessibility permission (same requirement as the existing dualie daemon) for the event tap to receive and swallow events system-wide. It does **not** need Karabiner-Elements — capshift never posts synthetic HID reports, since it isn't remapping keys, only intercepting a chord and firing a side effect.

## Components

- `capshift/Cargo.toml` — new workspace member, macOS-only deps: `core-graphics`, `core-foundation` (already used by the daemon crate for the same purpose), `clap`, `serde`, `kdl`, `miette`, `notify`, `anyhow`, `thiserror`, `tracing` + `tracing-subscriber`, `directories`
- `capshift/src/main.rs` — CLI entry (clap), loads config, spawns the tap on a dedicated OS thread (CGEventTap run loop blocks), parks main thread
- `capshift/src/tap.rs` — CGEventTap FFI wrapper + the chord state machine described above (a much-trimmed descendant of `daemon/src/intercept/macos.rs` — no IOHIDManager, no KVHD, no serial/active-output plumbing)
- `capshift/src/config.rs` — loads `~/.config/capshift/config.kdl`, hot-reloads on change via `notify` (same pattern as `daemon/src/config.rs`'s watch, without the multi-machine/git-sync layers)
- `capshift/src/actions.rs` — `fire(&Action)`: `AppLaunch` (macOS `open -b <bundle_id>`, brings app to foreground if already running) and `ShellCommand` (`sh -c <command>`) — ported near-verbatim from `daemon/src/launch.rs`'s macOS branch, dropping the Linux `gtk-launch`/`gio` code entirely

## Config format

KDL file at `~/.config/capshift/config.kdl`:

```kdl
// ~/.config/capshift/config.kdl
bind "s" app="com.tinyspeck.slackmacgap" label="Slack"
bind "m" app="com.apple.mail" label="Mail"
bind "t" shell="open -a Terminal" label="Terminal"
```

- `bind "<key>" app="<bundle-id>" label="<name>"` — focus/launch app by bundle ID
- `bind "<key>" shell="<command>" label="<name>"` — run a shell command
- Exactly one of `app=` / `shell=` must be present per binding; a binding with both or neither is a config error (logged, that binding skipped, rest of config still loads)
- Flat list, no layers/outputs — single keyboard, single machine
- File is created with a couple of commented-out example bindings on first run if it doesn't exist (matches dualie's existing auto-create-config behavior)

## Error handling

- Missing Accessibility permission → `CGEventTapCreate` returns NULL; log a clear, actionable error ("System Settings → Privacy & Security → Accessibility → add capshift") and `exit(1)` rather than silently running with a no-op tap
- Invalid/incomplete `bind` line (missing label, both/neither of app+shell) → warn and skip that one binding; rest of config still loads
- Action dispatch failure (`open -b` fails, shell command fails to spawn) → warn, non-fatal, matches existing `launch.rs` behavior (spawn-and-forget, log on spawn error only)
- Duplicate bindings for the same key → last one in the file wins, warn on load

## Testing

- Unit tests for KDL parsing (valid bindings, missing fields, duplicate keys) — no hardware/OS interaction needed, same pattern as any KDL-parsing unit tests elsewhere in the repo
- Chord state machine logic (caps-down → match → swallow → caps-up) is small enough to unit test with synthetic key-code sequences, independent of the real CGEventTap FFI, by extracting it into a pure function/struct that the tap callback calls into (mirrors how `remap.rs`'s `process_key` is tested independently of the IOHIDManager callback in the existing daemon)
- No automated test for the actual CGEventTap/Accessibility-permission path — manual smoke test only (documented in the plan's manual verification step)

## Packaging / CI (Apple Silicon Homebrew install)

- Extend `.github/workflows/release.yml` (or add a sibling `release-capshift.yml`) with a build matrix limited to `aarch64-apple-darwin` and `x86_64-apple-darwin` — no SPA/web build step, since capshift has no config UI
- Package as `capshift-<version>-<target>.tar.gz`, same shape as the existing dualie release artifacts
- New formula `homebrew/capshift.rb`, published as `Formula/capshift.rb` in the existing `dualie-dev/homebrew-dualie` tap (reuses the existing `TAP_TOKEN` secret — no new tap repo needed)
- Formula's `service do ... end` block registers a launchd agent so `brew services start capshift` gives autostart-at-login with crash restart, same pattern as the existing `dualie.rb` formula
- Formula `caveats` block documents the Accessibility permission requirement, same as `dualie.rb`

## Open items for the implementation plan

- Exact CGEventTap FFI surface needed (a subset of what `intercept/macos.rs` already declares — `CGEventTapCreate`, `CGEventTapEnable`, `CFMachPortCreateRunLoopSource`, callback returning `CGEventRef` or NULL to swallow) is not yet enumerated here; the plan should confirm the precise API against Apple's headers rather than guessing signatures.
