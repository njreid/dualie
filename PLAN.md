# Dualie Implementation Plan

## Current state

The daemon serves a Svelte web UI over HTTP, persists config as JSON, and intercepts virtual
keys (F13–F24) fired by the RP2040. Remapping is applied in firmware only, to the
hardware-connected keyboard.

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

Dependencies in `daemon/Cargo.toml`:
```toml
kdl      = { version = "6" }
knuffel  = { version = "3" }    # typed KDL deserialiser
miette   = { version = "7", features = ["fancy"] }  # parse error reporting
notify   = { version = "6" }    # file watcher for hot-reload
```

Replace `daemon/src/config.rs`:
- `DualieConfig` derives `knuffel::Decode` instead of `serde::Deserialize`
- Config path: `~/.config/dualie/config.kdl` (via `proto::paths::config_file()`, update
  extension)
- `DualieConfig::load_or_default()` — parse KDL, report errors with miette spans
- `DualieConfig::save()` — serialise back to KDL (use `kdl` crate to build document)
- `DualieConfig::watch()` — returns a `notify::Watcher` that sends on a channel when the
  file changes; daemon reloads and applies without restart

Migration: provide a `dualie convert` subcommand that reads the old `config.json` and
writes `config.kdl`.

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

### 2.3 Config KDL schema

```kdl
version 1

output "a" {
    key-remaps {
        remap src="caps"  dst="escape"
        remap src="lctrl" dst="lalt"
    }
    modifier-remaps {
        remap src="lctrl" dst="lalt"
    }
    caps-layer passthrough=true {
        entry src="h" dst="left"
        entry src="j" dst="down"
        entry src="k" dst="up"
        entry src="l" dst="right"
        entry src="1" type="jump-a"
        entry src="2" type="jump-b"
        entry src="space" type="swap"
        entry src="t" type="action" slot=0
    }
    virtual-actions {
        slot 0 type="app-launch" app-id="org.wezfurlong.wezterm" label="Terminal"
        slot 1 type="shell" command="rofi -show drun" label="Launcher"
    }
}

output "b" {
    // ...
}

sync-pairs {
    pair "nvim"  local="~/.config/nvim"
    pair "ssh"   local="~/.ssh" recursive=false
    pair "fish"  local="~/.config/fish"
}
```

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
