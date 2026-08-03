# capshift — minimal caps-lock shortcut daemon

Date: 2026-08-03

## Problem

Dualie's existing `daemon` crate implements caps-lock-chord shortcuts (`VirtualAction::AppLaunch` / `ShellCommand`) as part of a much larger KVM system: IOHIDManager device-seize, Karabiner VirtualHIDDevice injection, RP2040 serial peer, git-synced multi-machine config, file sync, clipboard sync. For a single MacBook that just wants "hold Caps Lock + key → focus/launch an app (or run a shell command)", none of that KVM machinery is needed.

`capshift` is a new, independent, macOS-only crate that provides just the caps-lock-chord-to-action behavior, with no dependency on the RP2040 proto, KVM config, or any of the multi-machine sync logic.

## Non-goals

- No Linux/Windows support (macOS only)
- Caps Lock is the *only* chord modifier — no other modifier combinations (e.g. no caps+ctrl+key, no non-caps chords) are supported as triggers right now
- No multi-machine config sync, no RP2040/serial peer
- No config UI / web server
- No layers, per-output configs, or other KVM-specific config concepts

## Architecture

**Revision:** a plain `CGEventTap` turned out not to be reliable enough (taps can be silently disabled by the OS under load, and swallowing/passing-through individual events via a listen-mode tap is flakier than Karabiner's approach). `capshift` instead reuses the same architecture as the existing dualie daemon's macOS interceptor: **IOHIDManager exclusive device seize** + **Karabiner VirtualHIDDevice (KVHD) report injection**, exactly like `daemon/src/intercept/macos.rs` + `daemon/src/intercept/macos_kvhd.rs`, just with the layer/output/serial logic stripped out.

Because the physical keyboard is seized exclusively (`kIOHIDOptionsTypeSeizeDevice`), the OS receives *no* input from it directly — every key must be re-injected as an HID boot-protocol report through the Karabiner VirtualHIDKeyboard IOKit user client, or the keyboard stops working for typing entirely. So the chord logic is really a filter over a full passthrough:

- Caps Lock key-down → enter "chord active" state; **not** added to the injected report (never forwarded, so it never toggles caps-lock or reaches other apps)
- While chord is active, next key-down that matches a configured **action** binding (`app=`/`shell=`) → fire that binding's action; **not** forwarded
- While chord is active, next key-down that matches a configured **remap** binding (`key=`) → forward the *target* key's HID keycode instead of the original (e.g. caps+h down → Left-arrow down injected via KVHD); the corresponding key-up is translated the same way so the virtual-pressed set stays consistent
- Any other key-down/up (chord inactive, or chord active but unmatched) → forwarded as-is: added/removed from the virtual-pressed set, built into an 8-byte HID report, posted to KVHD
- Caps Lock key-up → exit "chord active" state, not forwarded

Caps Lock is fully repurposed as a modifier; tapping it alone does nothing (no timing/tap-vs-hold disambiguation needed).

This needs both macOS Accessibility permission (for the exclusive device seize, same requirement as the existing dualie daemon) **and Karabiner-Elements installed and running** (it provides the VirtualHIDDevice driver capshift posts reports to). This is a real runtime dependency, not optional — `capshift` cannot pass typed keystrokes through without it.

## Components

- `capshift/Cargo.toml` — new workspace member, macOS-only deps: `core-graphics`, `core-foundation`, `clap`, `serde`, `kdl`, `miette`, `notify`, `anyhow`, `thiserror`, `tracing` + `tracing-subscriber`, `directories`
- `capshift/src/main.rs` — CLI entry (clap), loads config, spawns the interceptor on a dedicated OS thread (`CFRunLoopRun()` blocks), parks main thread
- `capshift/src/hid.rs` — IOHIDManager device-seize + input-value callback, ported from `daemon/src/intercept/macos.rs`, dropping the `cfg_rx`/`active_output`/`serial`/layer-recompile plumbing — just seize, decode HID usage/value, call into the chord state machine, forward the resulting report to KVHD
- `capshift/src/kvhd.rs` — Karabiner VirtualHIDKeyboard IOKit user client binding + `build_report`, ported near-verbatim from `daemon/src/intercept/macos_kvhd.rs` (unchanged — this code has no KVM-specific logic to strip)
- `capshift/src/chord.rs` — the pure chord state machine (caps-down → armed, matching action binding → fire + swallow, matching remap binding → translate keycode, caps-up → disarm, everything else → passthrough), analogous in spirit to `remap.rs`'s `process_key` but far simpler since there's only one modifier key (Caps Lock) and a flat binding table, no layers/outputs
- `capshift/src/keycodes.rs` — `keycode_by_name(&str) -> Option<u8>` name→HID-keycode table, ported verbatim from `daemon/src/config.rs`'s existing `keycode_by_name` (it already covers a-z, 0-9, arrows, f1-f12, media keys, etc. — dualie's KDL config already has a `chord <src> <dst>` directive using this exact table, e.g. the commented-out example in `daemon/src/config.rs` line 36-39: `chord h left` / `chord l right` / `chord k up` / `chord j down` — capshift's `key=` binding is the same idea, just scoped to caps-only chords instead of dualie's general remap layer)
- `capshift/src/config.rs` — loads `~/.config/capshift/config.kdl`, hot-reloads on change via `notify` (same pattern as `daemon/src/config.rs`'s watch, without the multi-machine/git-sync layers)
- `capshift/src/actions.rs` — `fire(&Action)`: `AppLaunch` (macOS `open -b <bundle_id>`, brings app to foreground if already running) and `ShellCommand` (`sh -c <command>`) — ported near-verbatim from `daemon/src/launch.rs`'s macOS branch, dropping the Linux `gtk-launch`/`gio` code entirely

## Config format

KDL file at `~/.config/capshift/config.kdl`:

```kdl
// ~/.config/capshift/config.kdl
bind "s" app="com.tinyspeck.slackmacgap" label="Slack"
bind "m" app="com.apple.mail" label="Mail"
bind "t" shell="open -a Terminal" label="Terminal"

// caps+h/j/k/l as arrow keys
bind "h" key="left"
bind "j" key="down"
bind "k" key="up"
bind "l" key="right"
```

- `bind "<key>" app="<bundle-id>" label="<name>"` — focus/launch app by bundle ID
- `bind "<key>" shell="<command>" label="<name>"` — run a shell command
- `bind "<key>" key="<target-key-name>"` — remap: caps+`<key>` sends `<target-key-name>` instead (e.g. an arrow key); `<target-key-name>` is looked up via the same `keycode_by_name` table as dualie's existing `chord` directive, so any name it accepts (letters, digits, `left`/`right`/`up`/`down`, `f1`-`f12`, media keys, etc.) works here too
- Exactly one of `app=` / `shell=` / `key=` must be present per binding; a binding with more than one or none of them is a config error (logged, that binding skipped, rest of config still loads); `label=` is required for `app=`/`shell=` bindings but not meaningful for `key=` bindings
- Flat list, no layers/outputs, no modifier combinations on the trigger side — single keyboard, single machine, caps-lock-only chords
- File is created with a couple of commented-out example bindings (including the h/j/k/l arrow remap) on first run if it doesn't exist (matches dualie's existing auto-create-config behavior)

## Error handling

- Missing Accessibility permission → `IOHIDDeviceOpen(..., kIOHIDOptionsTypeSeizeDevice)` fails; log a clear, actionable error ("System Settings → Privacy & Security → Accessibility → add capshift") — matches existing `macos.rs` behavior (warns per-device rather than hard-exiting, since the manager may still pick up other devices later)
- Karabiner-Elements not installed/running → `KvhdHandle::open()` fails at startup with "Karabiner VirtualHIDKeyboard service not found — is Karabiner-Elements installed and running?" (this error already exists verbatim in `macos_kvhd.rs`); this is fatal at startup (`exit(1)`) since capshift cannot pass typed keystrokes through without it — running with a seized-but-uninjected keyboard would make the keyboard unusable
- Invalid/incomplete `bind` line (missing label on an app/shell binding, or zero/multiple of app+shell+key present) → warn and skip that one binding; rest of config still loads
- Unknown `key="<target-key-name>"` (not found in `keycode_by_name`) → warn and skip that binding; rest of config still loads
- Action dispatch failure (`open -b` fails, shell command fails to spawn) → warn, non-fatal, matches existing `launch.rs` behavior (spawn-and-forget, log on spawn error only)
- Duplicate bindings for the same key → last one in the file wins, warn on load

## Testing

- Unit tests for KDL parsing (valid action bindings, valid remap bindings, missing fields, multiple/zero of app+shell+key, unknown key name, duplicate keys) — no hardware/OS interaction needed, same pattern as any KDL-parsing unit tests elsewhere in the repo
- `keycode_by_name` ported from `daemon/src/config.rs` already has existing unit test coverage there (line ~1431-1438) to crib from
- Chord state machine logic (caps-down → armed → matching action binding → fire + swallow; matching remap binding → translated keycode forwarded; caps-up → disarm; unmatched keys → passthrough) is small enough to unit test with synthetic HID usage/value sequences, independent of the real IOHIDManager/KVHD FFI, by keeping it a pure function/struct that the HID callback calls into (mirrors how `remap.rs`'s `process_key` is tested independently of the IOHIDManager callback in the existing daemon)
- No automated test for the actual IOHIDManager seize / KVHD injection / Accessibility-permission path — manual smoke test only (documented in the plan's manual verification step; requires Karabiner-Elements installed on the test machine)

## Packaging / CI (Apple Silicon Homebrew install)

- Extend `.github/workflows/release.yml` (or add a sibling `release-capshift.yml`) with a build matrix limited to `aarch64-apple-darwin` and `x86_64-apple-darwin` — no SPA/web build step, since capshift has no config UI
- Package as `capshift-<version>-<target>.tar.gz`, same shape as the existing dualie release artifacts
- New formula `homebrew/capshift.rb`, published as `Formula/capshift.rb` in the existing `dualie-dev/homebrew-dualie` tap (reuses the existing `TAP_TOKEN` secret — no new tap repo needed)
- Formula's `service do ... end` block registers a launchd agent so `brew services start capshift` gives autostart-at-login with crash restart, same pattern as the existing `dualie.rb` formula
- Formula `caveats` block documents both the Accessibility permission requirement *and* the Karabiner-Elements runtime dependency (installing Karabiner-Elements itself is left to the user — a Homebrew formula cannot depend on a cask, so this is caveats-only, same limitation the existing `dualie.rb` already has)

## Open items for the implementation plan

- The exact IOHIDManager + KVHD FFI surface needed already exists verbatim in `daemon/src/intercept/macos.rs` and `daemon/src/intercept/macos_kvhd.rs` — the plan should port those files with the layer/serial/active-output plumbing stripped, not re-derive the FFI declarations from scratch.
- Two distinct existing keycode helpers are candidates to port/share rather than reimplement: `daemon/src/intercept/keycodes.rs`'s `hid_modifier_bit` (needed by `hid.rs` to detect Caps Lock and build reports) and `daemon/src/config.rs`'s `keycode_by_name` (needed by `config.rs`/`keycodes.rs` to parse `key="left"` etc.). Confirm in the plan whether these are simple enough to duplicate into `capshift` or worth extracting into a small shared crate.
