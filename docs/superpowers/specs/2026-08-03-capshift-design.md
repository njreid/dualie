# capshift — minimal caps-lock shortcut daemon

Date: 2026-08-03

## Problem

Dualie's existing `daemon` crate implements caps-lock-chord shortcuts (`VirtualAction::AppLaunch` / `ShellCommand`) as part of a much larger KVM system: IOHIDManager device-seize, Karabiner VirtualHIDDevice injection, RP2040 serial peer, git-synced multi-machine config, file sync, clipboard sync. For a single MacBook that just wants "hold Caps Lock + key → focus/launch an app (or run a shell command)", none of that KVM machinery is needed.

`capshift` is a new, independent, macOS-only crate that provides just the caps-lock-chord-to-action behavior, with no dependency on the RP2040 proto, KVM config, or any of the multi-machine sync logic.

## Non-goals

- No Linux/Windows support (macOS only)
- No key *remapping* (no modifier swaps, no layer-based output config) — Karabiner VirtualHIDDevice is used here purely as the passthrough/injection mechanism required by exclusive device seize, not for remapping
- No multi-machine config sync, no RP2040/serial peer
- No config UI / web server
- No layers, per-output configs, or other KVM-specific config concepts

## Architecture

**Revision:** a plain `CGEventTap` turned out not to be reliable enough (taps can be silently disabled by the OS under load, and swallowing/passing-through individual events via a listen-mode tap is flakier than Karabiner's approach). `capshift` instead reuses the same architecture as the existing dualie daemon's macOS interceptor: **IOHIDManager exclusive device seize** + **Karabiner VirtualHIDDevice (KVHD) report injection**, exactly like `daemon/src/intercept/macos.rs` + `daemon/src/intercept/macos_kvhd.rs`, just with the layer/output/serial logic stripped out.

Because the physical keyboard is seized exclusively (`kIOHIDOptionsTypeSeizeDevice`), the OS receives *no* input from it directly — every key must be re-injected as an HID boot-protocol report through the Karabiner VirtualHIDKeyboard IOKit user client, or the keyboard stops working for typing entirely. So the chord logic is really a filter over a full passthrough:

- Caps Lock key-down → enter "chord active" state; **not** added to the injected report (never forwarded, so it never toggles caps-lock or reaches other apps)
- While chord is active, next key-down that matches a configured binding → fire that binding's action; **not** forwarded either
- Any other key-down/up (chord inactive, or chord active but unmatched) → forwarded as-is: added/removed from the virtual-pressed set, built into an 8-byte HID report, posted to KVHD
- Caps Lock key-up → exit "chord active" state, not forwarded

Caps Lock is fully repurposed as a modifier; tapping it alone does nothing (no timing/tap-vs-hold disambiguation needed).

This needs both macOS Accessibility permission (for the exclusive device seize, same requirement as the existing dualie daemon) **and Karabiner-Elements installed and running** (it provides the VirtualHIDDevice driver capshift posts reports to). This is a real runtime dependency, not optional — `capshift` cannot pass typed keystrokes through without it.

## Components

- `capshift/Cargo.toml` — new workspace member, macOS-only deps: `core-graphics`, `core-foundation`, `clap`, `serde`, `kdl`, `miette`, `notify`, `anyhow`, `thiserror`, `tracing` + `tracing-subscriber`, `directories`
- `capshift/src/main.rs` — CLI entry (clap), loads config, spawns the interceptor on a dedicated OS thread (`CFRunLoopRun()` blocks), parks main thread
- `capshift/src/hid.rs` — IOHIDManager device-seize + input-value callback, ported from `daemon/src/intercept/macos.rs`, dropping the `cfg_rx`/`active_output`/`serial`/layer-recompile plumbing — just seize, decode HID usage/value, call into the chord state machine, forward the resulting report to KVHD
- `capshift/src/kvhd.rs` — Karabiner VirtualHIDKeyboard IOKit user client binding + `build_report`, ported near-verbatim from `daemon/src/intercept/macos_kvhd.rs` (unchanged — this code has no KVM-specific logic to strip)
- `capshift/src/chord.rs` — the pure chord state machine (caps-down → armed, matching key-down → fire + swallow, caps-up → disarm, everything else → passthrough), analogous in spirit to `remap.rs`'s `process_key` but far simpler since there are no layers/outputs — just one modifier key and a flat binding table
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

- Missing Accessibility permission → `IOHIDDeviceOpen(..., kIOHIDOptionsTypeSeizeDevice)` fails; log a clear, actionable error ("System Settings → Privacy & Security → Accessibility → add capshift") — matches existing `macos.rs` behavior (warns per-device rather than hard-exiting, since the manager may still pick up other devices later)
- Karabiner-Elements not installed/running → `KvhdHandle::open()` fails at startup with "Karabiner VirtualHIDKeyboard service not found — is Karabiner-Elements installed and running?" (this error already exists verbatim in `macos_kvhd.rs`); this is fatal at startup (`exit(1)`) since capshift cannot pass typed keystrokes through without it — running with a seized-but-uninjected keyboard would make the keyboard unusable
- Invalid/incomplete `bind` line (missing label, both/neither of app+shell) → warn and skip that one binding; rest of config still loads
- Action dispatch failure (`open -b` fails, shell command fails to spawn) → warn, non-fatal, matches existing `launch.rs` behavior (spawn-and-forget, log on spawn error only)
- Duplicate bindings for the same key → last one in the file wins, warn on load

## Testing

- Unit tests for KDL parsing (valid bindings, missing fields, duplicate keys) — no hardware/OS interaction needed, same pattern as any KDL-parsing unit tests elsewhere in the repo
- Chord state machine logic (caps-down → armed → matching key-down → fire + swallow → caps-up → disarm; unmatched keys → passthrough) is small enough to unit test with synthetic HID usage/value sequences, independent of the real IOHIDManager/KVHD FFI, by keeping it a pure function/struct that the HID callback calls into (mirrors how `remap.rs`'s `process_key` is tested independently of the IOHIDManager callback in the existing daemon)
- No automated test for the actual IOHIDManager seize / KVHD injection / Accessibility-permission path — manual smoke test only (documented in the plan's manual verification step; requires Karabiner-Elements installed on the test machine)

## Packaging / CI (Apple Silicon Homebrew install)

- Extend `.github/workflows/release.yml` (or add a sibling `release-capshift.yml`) with a build matrix limited to `aarch64-apple-darwin` and `x86_64-apple-darwin` — no SPA/web build step, since capshift has no config UI
- Package as `capshift-<version>-<target>.tar.gz`, same shape as the existing dualie release artifacts
- New formula `homebrew/capshift.rb`, published as `Formula/capshift.rb` in the existing `dualie-dev/homebrew-dualie` tap (reuses the existing `TAP_TOKEN` secret — no new tap repo needed)
- Formula's `service do ... end` block registers a launchd agent so `brew services start capshift` gives autostart-at-login with crash restart, same pattern as the existing `dualie.rb` formula
- Formula `caveats` block documents both the Accessibility permission requirement *and* the Karabiner-Elements runtime dependency (installing Karabiner-Elements itself is left to the user — a Homebrew formula cannot depend on a cask, so this is caveats-only, same limitation the existing `dualie.rb` already has)

## Open items for the implementation plan

- The exact IOHIDManager + KVHD FFI surface needed already exists verbatim in `daemon/src/intercept/macos.rs` and `daemon/src/intercept/macos_kvhd.rs` — the plan should port those files with the layer/serial/active-output plumbing stripped, not re-derive the FFI declarations from scratch.
- `daemon/src/intercept/keycodes.rs` (HID modifier-bit helpers) is also a candidate to port/share rather than reimplement — confirm in the plan whether it's simple enough to duplicate or worth extracting to a shared crate.
