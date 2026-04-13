# Dualie Implementation Plan

## Status

**Phase 1 ✓ complete** — Serial peer, F13–F24 retired, firmware CDC-ACM, `just flash`.

**Phase 2 ✓ complete** — KDL config with hot-reload, web UI removed, Unix socket status server.

**Phase 3 ✓ complete** — Linux evdev+uinput local keyboard remapping.

**Phase 5 ✓ complete** — `dualie-tui` binary with Status / Remaps / Caps Layer / Config / Sync tabs.

**Phase 4 ✓ complete** — macOS keyboard remapping via IOHIDManager + Karabiner VirtualHIDDevice.

**Phase 7 ✓ complete** — Git-backed config versioning. Repo layout `appname/config.kdl`.
Platform paths via `directories` crate. `LocalConfig` from `local.kdl` (gitignored) carries
machine-name and optional repo-path override. Auto-commit on every config save or serial-sync
write. `git fetch` on startup populates `GIT_PENDING` for status display. TUI pull/push via
`GitRepo::pull()` / `GitRepo::push()`.

**Phase 6 ✓ complete** — Clipboard + config-file sync over CDC-ACM serial.

**Current state:** The daemon loads `~/.config/dualie/dualie.kdl` (hot-reloaded via notify),
maintains a CDC-ACM serial connection to the RP2040 with auto-reconnect, exposes a status
socket at `$XDG_RUNTIME_DIR/dualie/daemon.sock`, and on Linux grabs all attached keyboards
via evdev, applies caps-layer / key / modifier remaps, and re-injects events via uinput.
New keyboards are picked up automatically via udev hotplug.

Config-file sync (`daemon/src/file_sync.rs`): `notify` watcher sends `SyncChunk` over serial
when a watched file changes; receiver applies LWW + local-section guards, writes on remote win,
saves `.dualie-conflict` backup on conflict.  Clipboard sync: `ClipboardPull` requests the
other machine's clipboard; `ClipboardPush` delivers it.  Caps-layer `clip-pull` action
sends the request.  Known apps registry embedded as `known_apps.kdl` (35+ apps); user
overrides in `~/.config/dualie/user_apps.kdl`.

## Target state

1. **All keyboards remapped** — built-in + directly connected keyboards get the same remap
   config applied by the daemon intercept layer (Linux: evdev+uinput; macOS: Karabiner driver)
2. **Clean virtual action channel** — RP2040 sends `VirtualAction { slot }` over CDC-ACM
   serial; F13–F24 hack retired entirely
3. **Caps-lock layer works everywhere** — firmware handles hardware keyboard; daemon handles
   all others, with reliable Caps handling on macOS via Karabiner-VirtualHIDDevice driver
4. **KDL config** — human-readable `~/.config/dualie/config.kdl`, hot-reloaded by daemon
5. **TUI** — `dualie-tui` binary replaces the Svelte web UI; Ratatui, talks to daemon via
   Unix socket for live status, edits config file directly
6. **One-command firmware flash** — `just flash` reboots RP2040-A into bootloader via serial,
   copies uf2; RP2040-A auto-flashes RP2040-B over UART

---

## Phase 1 — Serial peer, retire F13–F24, firmware flashing

### 1.1 Rename `HubMessage` → `DualieMessage` in `proto/`

The hub design is shelved. Rename for clarity throughout:
- `proto/src/protocol.rs` — rename enum, drop hub-specific variants (Welcome, Hello, etc.),
  keep: `VirtualAction`, `ActiveOutput`, `ClipboardPush/Pull`, `SyncList/Chunk/Ack`,
  `ConfigRequest/Push`, `Ping/Pong`, `Error`; add `RebootToBootloader`
- `proto/src/lib.rs` — update re-exports
- `daemon/src/peer.rs` — update references
- Archive `hub/` (keep compiling but no active development)

### 1.2 Add `SerialPeer` to `proto/`

New file `proto/src/serial.rs`:
- `SerialPeer` wraps `tokio::io::AsyncRead + AsyncWrite` (the CDC-ACM device fd)
- `send(&DualieMessage)` — CBOR-serialise → COBS-encode → write (0x00 delimiter)
- `recv() -> DualieMessage` — read until 0x00 → COBS-decode → CBOR-deserialise
- Add `cobs` crate to `proto/Cargo.toml`
- `SerialPeer::open(path: &Path)` — open the CDC-ACM device, configure baud rate

Auto-detection of the serial device path:
- Linux: first `/dev/ttyACM*` with USB VID/PID matching RP2040
- macOS: first `/dev/tty.usbmodem*`
- Override via `--serial /dev/ttyACM1` CLI flag

### 1.3 Replace TCP peer with serial peer in `daemon/`

- `daemon/src/peer.rs` — rewrite around `SerialPeer`; remove `TcpPeer`, TCP reconnect loop,
  `HubClient`, hostname crate dep
- Serial reconnect loop: watch for device to appear/disappear (poll `/dev/ttyACM*`)
- Move virtual action dispatch here: receive `DualieMessage::VirtualAction { slot }`,
  look up in config, execute

### 1.4 Remove F13–F24 infrastructure from daemon

- `daemon/src/config.rs` — remove `VKEY_TABLE`, `DUALIE_VKEY_COUNT`, `vkey_slot()`
- `daemon/src/serialize.rs` — remove vkey encoding from `merge_key_remaps`
- `daemon/src/intercept/mod.rs` — remove virtual keycode detection entirely; this module
  now only handles local keyboard remapping (Phase 2)

### 1.5 Firmware: CDC-ACM composite + `VirtualAction` over serial

In `src/`:
- Add TinyUSB CDC-ACM interface to the composite USB descriptor (alongside HID)
- Add minimal COBS encoder to firmware (`src/cobs.h` / `src/cobs.c`, ~50 lines)
- In caps-layer handler (`keyboard.c`): for `CAPS_ENTRY_VIRTUAL`, send
  `VirtualAction { slot }` as a COBS-framed CBOR message over CDC-ACM instead of emitting
  a fake HID keycode
- Add `DualieMessage::RebootToBootloader` handler: call `reset_usb_boot(0, 0)` from
  the RP2040 bootrom — puts the board into USB MSC (RPI-RP2) mode immediately
- The firmware is otherwise transparent: all other `DualieMessage` frames received on
  CDC-ACM are forwarded byte-for-byte to the other board over the inter-board UART,
  and vice versa — no parsing needed for clipboard/sync messages

### 1.6 `just flash` recipe

```
flash:
    #!/usr/bin/env bash
    # 1. Build firmware
    cmake --build build
    # 2. Trigger bootloader on near board via serial
    dualie --serial-cmd reboot-to-bootloader
    # 3. Wait for RPI-RP2 to mount (udev or poll /dev/disk/by-label/RPI-RP2)
    echo "Waiting for RP2040 bootloader..."
    for i in $(seq 1 20); do
        [ -b /dev/disk/by-label/RPI-RP2 ] && break
        sleep 0.5
    done
    # 4. Copy uf2 — RP2040-A flashes itself, then auto-flashes RP2040-B over UART
    cp build/dualie_board_A.uf2 /run/media/$USER/RPI-RP2/
    echo "Flashing complete. Both boards will reboot."
```

The far board (RP2040-B) is flashed automatically by the DeskHop firmware's existing
cross-board upgrade mechanism — it receives the image over UART from RP2040-A after
RP2040-A finishes its own flash. No second USB connection needed.

---

## Phase 2 — Replace JSON config with KDL, remove web UI

### 2.1 Add KDL config parsing

Dependencies added to `daemon/Cargo.toml` ✓:
```toml
kdl    = { version = "6" }                              # KDL DOM parser
miette = { version = "7", features = ["fancy"] }        # parse error reporting with spans
notify = { version = "6", features = ["macos_kqueue"] } # file watcher for hot-reload
```

`daemon/src/config.rs` ✓:
- Hand-written KDL parser (`DualieConfig::from_kdl`) and serialiser (`to_kdl_string`)
- Config path: `~/.config/dualie/dualie.kdl`
- `load_or_default()` — tries KDL, falls back to legacy `config.json`, then default
- `watch()` — spawns notify watcher, returns `watch::Receiver<DualieConfig>`

Legacy JSON migration: `load_or_default` auto-migrates on first load; explicit
`dualie convert` subcommand can be added when needed.

### 2.2 Remove the web UI and HTTP server

Remove from `daemon/Cargo.toml`:
```toml
# DELETE these deps:
axum, tower, tower-http
```

Remove files:
- `daemon/src/web.rs` — entire HTTP router
- `web/` directory — entire Svelte app (archive in git)

Remove from `daemon/src/main.rs`:
- `mod web`
- `axum::serve(...)` call
- `--port` CLI flag

Add Unix socket status server in its place (small, ~50 lines):
- `daemon/src/status.rs` — listen on `$XDG_RUNTIME_DIR/dualie/daemon.sock`
- Accepts connections, streams `DualieMessage::Status { active_output, rp2040_connected }`
- TUI connects here for live status display

### 2.3 Config KDL schema ✓ implemented

```kdl
// dualie.kdl — Dualie daemon configuration
// Keys: single char (a-z, 0-9), named (esc left volup f1…), or 0x hex keycode.
// Modifiers: lctrl lshift lalt lmeta rctrl rshift ralt rmeta
//            short: ctrl shift alt meta cmd win super

output A {
    actions {
        // Implicit slot assignment by order (0, 1, …). Referenced by label in layers.
        launch "Slack"    app-id="com.tinyspeck.slackmacgap"
        shell  "Terminal" command="open -a Terminal"
    }

    remap {
        key capslock esc                    // single char or named key
        key 0x39 0x29                       // raw HID keycodes
        key a a src-mod=lctrl               // require modifier to match
        modifier lalt rctrl                 // swap modifier globally on every report
    }

    layers {
        caps {
            chord  a e                      // caps+A → E
            chord  b ctrl_t                 // caps+B → Ctrl+T
            chord  c ctrl_shift_a           // caps+C → Ctrl+Shift+A
            action s "Slack"                // caps+S → fire Slack action (slot 0)
            jump-a h                        // caps+H → switch to output A
            jump-b k                        // caps+K → switch to output B
            swap   n                        // caps+N → toggle output
        }
    }
}

output B {}
```

Notes:
- `output A` / `output B` — bare KDL v2 identifiers (quotes optional)
- Chord modifier prefix uses underscores: `ctrl_t`, `ctrl_shift_a`, `alt_f4`
- `action` resolves label to slot at parse time; reordering `actions` is safe

---

## Phase 3 — Linux local keyboard remapping (evdev + uinput)

### 3.1 Add crate dependencies

```toml
evdev = { version = "0.12" }   # includes uinput feature
```

### 3.2 Create `daemon/src/intercept/remap.rs`

Platform-independent, pure transformation logic — no I/O:

```rust
pub fn apply_remaps(
    event: KeyEvent,
    cfg: &OutputDaemonConfig,
    state: &mut LayerState,
) -> Option<KeyEvent>
```

`LayerState` tracks `caps_held: bool` and `caps_lock_on: bool`.
Mirrors firmware `process_caps_layer()` in Rust. Fully unit-tested.

### 3.3 Create `daemon/src/intercept/linux.rs`

1. Enumerate `/dev/input/event*` keyboards via udev (`ID_INPUT_KEYBOARD=1`),
   excluding any existing uinput virtual devices
2. `device.grab()` — exclusive evdev grab; Niri/Wayland stops seeing the physical device
3. Create a uinput virtual keyboard (clone capabilities from grabbed device)
4. Per-device task: read `InputEvent` → `apply_remaps` → write to uinput
5. `udev_monitor` task: dynamically grab new keyboards as they connect

---

## Phase 4 — macOS local keyboard remapping (Karabiner driver)

### 4.1 Karabiner-VirtualHIDDevice IOKit bindings

`daemon/src/intercept/macos_kvhd.rs` — implement the three IOKit calls manually in Rust
(~100 lines, no bindgen required):
- `kvhd_initialize() -> Result<KvhdHandle>`
- `kvhd_post_report(handle, report: &HIDKeyboardReport) -> Result<()>`
- `kvhd_reset(handle)`

### 4.2 Create `daemon/src/intercept/macos.rs`

1. Initialise Karabiner virtual HID device
2. Create `CGEventTap` at `kCGHIDEventTap` with `kCGEventTapOptionDefault`
3. In tap callback: extract keycode + flags → `apply_remaps` → suppress original →
   post remapped report to Karabiner virtual device
4. Handle `kCGEventFlagsChanged` with Caps flag to drive `LayerState::caps_held`

---

## Phase 5 — TUI (`dualie-tui` crate)

New workspace member `tui/`:

```toml
[dependencies]
ratatui    = { version = "0.28" }
crossterm  = { version = "0.27" }
dualie-proto = { path = "../proto" }
kdl        = { version = "6" }
knuffel    = { version = "3" }
tokio      = { workspace = true }
```

### Screens

| Screen | Content |
|--------|---------|
| **Status** | Active output (A/B), RP2040 connected, remap active, last virtual action fired |
| **Remaps** | Table of key remaps + modifier remaps for selected output; inline edit |
| **Caps layer** | Table of caps-layer entries; add/remove/edit |
| **Actions** | Virtual action slots 0–31; assign app-launch or shell command |
| **Sync pairs** | List of watched directory pairs; add/remove; show last-sync status |
| **Config** | Raw KDL editor with syntax highlight; save triggers hot-reload |

### IPC

- Status stream: Unix socket `$XDG_RUNTIME_DIR/dualie/daemon.sock`
- Config edits: write `~/.config/dualie/config.kdl` directly; daemon hot-reloads via
  `notify` watcher

---

## Phase 6 — Clipboard + file sync over CDC-ACM

- Add `DualieMessage::ClipboardPush(String)` / `ClipboardPull` variants
- `daemon/src/clipboard.rs` — OS clipboard via `arboard` crate
- Caps+V triggers `ClipboardPull` → serial → other machine's daemon writes to OS clipboard
- Add `DualieMessage::SyncList` / `SyncChunk` / `SyncAck` variants
- `daemon/src/sync.rs` — `notify` watcher + `fast_rsync` delta transfers
- Sync pairs defined in `config.kdl` under `sync-pairs { ... }`
- Firmware is transparent: relays all non-`VirtualAction` frames between boards over UART

---

## Order of execution

```
Phase 1  →  Phase 2  →  Phase 3  →  Phase 4
  ↑                        ↑
serial peer           remap.rs shared
retire F13            by 3 and 4
firmware CDC-ACM
just flash
```

Phases 5 and 6 are independent and can be interleaved once Phase 2 is running.

Phase 1 has no firmware changes required to begin — daemon can open the serial port while
RP2040 still sends F13 keycodes; the two paths coexist until firmware is updated.
