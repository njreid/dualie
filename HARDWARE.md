# Dualie Hardware Design

## v1 Hardware: Two RP2040 Zero boards

Dualie v1 uses two [Waveshare RP2040 Zero](https://www.waveshare.com/rp2040-zero.htm) boards —
one per host machine. No additional hub hardware is required.

```text
  Machine A                                              Machine B
┌──────────┐  USB-C (HID + CDC-ACM)  ┌──────────┐  UART  ┌──────────┐  USB-C (HID + CDC-ACM)  ┌──────────┐
│          │◄───────────────────────►│ RP2040   │◄──────►│ RP2040   │◄───────────────────────►│          │
│Computer A│                         │ Zero A   │GP0/GP1 │ Zero B   │                         │Computer B│
└──────────┘                         └──────────┘        └──────────┘                         └──────────┘
                                          ▲                    ▲
                                    GP28/GP29            GP28/GP29
                                    USB host             USB host
                                    keyboard/mouse       keyboard/mouse
```

---

## RP2040 Zero pin assignments

| Pin | Signal | Purpose |
|-----|--------|---------|
| USB-C | USB device | Composite: HID (keyboard + mouse) + CDC-ACM (serial channel to daemon) |
| GP0 | UART0 TX | Inter-board UART — KVM state sync |
| GP1 | UART0 RX | Inter-board UART — KVM state sync |
| GP28 | PIO-USB D+ | USB host for attached keyboard/mouse |
| GP29 | PIO-USB D− | USB host for attached keyboard/mouse |

Both boards are 3.3 V logic. The UART runs directly between them (GP0/GP1 crossed:
A-TX → B-RX, A-RX → B-TX). An optional galvanic isolator between the boards follows the
original DeskHop design:

| Isolator | Max speed | Notes |
|----------|-----------|-------|
| TI ISO7721DR | 100 Mbps | Preferred — supports high UART baud rates |
| ADuM1201BRZ | 1 Mbps | Original DeskHop part — limits UART to 1 Mbaud |

---

## USB composite device (per machine)

Each RP2040 Zero presents two USB interfaces to its host machine over a single USB-C cable:

| Interface | USB class | Appears as | Purpose |
|-----------|-----------|-----------|---------|
| HID keyboard | 03/01/01 | (transparent) | Keyboard reports forwarded to active output |
| HID mouse | 03/01/02 | (transparent) | Mouse reports forwarded to active output |
| CDC-ACM serial | 02/02/00 | `/dev/ttyACM1` (Linux) / `/dev/tty.usbmodem*` (macOS) | Daemon control channel |

The CDC-ACM serial interface is **exclusively held open by the daemon**. Because it is a
character device (not a network interface), no other process can route through it or
intercept its traffic.

---

## CDC-ACM serial channel — replacing the F13–F24 hack

Previous versions used repurposed HID keycodes (F13–F24, International1–8, Lang1–4) as a
side-channel to signal virtual actions from the RP2040 to the daemon. This had several
problems: fake keycodes could leak to applications, the keycode space was polluted, and the
mechanism was fragile.

The CDC-ACM serial channel replaces this entirely. When the RP2040 detects a caps-layer
virtual action, it sends a COBS-framed `DualieMessage::VirtualAction { slot }` over the
serial port. The daemon receives it directly — no HID keycode emitted, no interception
required.

```text
Old:  Caps+key → RP2040 emits F13 HID keycode → daemon intercepts via CGEventTap/evdev
                 → suppresses → looks up slot → dispatches action
                 ↑ fragile, leaky, uses up keycode space

New:  Caps+key → RP2040 sends VirtualAction { slot: 5 } over CDC-ACM serial
                 → daemon receives on /dev/ttyACM1 → dispatches action
                 ↑ direct, clean, no HID involvement
```

The serial channel is also the foundation for future clipboard relay and file sync between
the two machines (see Future section below).

### Message framing

Messages are COBS-encoded with a `0x00` byte delimiter, identical to the TAP bridge design.
The message type is `DualieMessage` (defined in `proto/`), serialised to CBOR.

---

## Daemon: local keyboard remapping

The daemon applies the same remap config to **all keyboards on the machine**, not just the
one connected via the RP2040 hardware:

| Keyboard path | Remapping applied by |
|---------------|---------------------|
| Hardware keyboard via RP2040 | RP2040 firmware (key_remaps, modifier_remaps, caps_layer) |
| Built-in laptop keyboard | Daemon intercept layer |
| Any USB keyboard plugged directly into the machine | Daemon intercept layer |

This gives a consistent typing experience regardless of which keyboard you're using.

### macOS: Karabiner-VirtualHIDDevice

CGEventTap cannot reliably treat Caps Lock as a layer modifier (the OS handles it specially
before events reach the tap). The daemon therefore uses
[Karabiner-VirtualHIDDevice](https://github.com/pqrs-org/Karabiner-VirtualHIDDevice) — the
driver-level component of Karabiner-Elements, available as a standalone install.

Flow:

1. CGEventTap intercepts the raw key event (Accessibility permission required)
2. Daemon applies remap config in userspace
3. Remapped event is injected via the Karabiner virtual HID device at driver level
4. Original event is suppressed

The Karabiner-Elements app does **not** need to be installed or running. Only the
`Karabiner-VirtualHIDDevice.dext` system extension is required. Once approved in System
Settings → Privacy & Security, it persists across reboots independently of any app.

### Linux (CachyOS + Niri / Wayland)

On Wayland there is no compositor-level keyboard hook — you must intercept below the
compositor at the evdev device level:

1. Enumerate keyboard devices in `/dev/input/` via udev
2. `EVIOCGRAB` ioctl — exclusive grab; the Wayland compositor stops seeing the physical device
3. Read raw `input_event` structs, apply remap config
4. Write transformed events to a `/dev/uinput` virtual device (what Niri sees)

Requires the user to be in the `input` group, or the udev rule installed by `just install`.

---

## Bill of materials (v1)

| Component | Qty | Approx. | Notes |
|-----------|-----|---------|-------|
| Waveshare RP2040 Zero | 2 | £4 each | |
| USB-C cable (short) | 2 | £2 each | RP2040 Zero → host machine |
| USB-C cable (keyboard/mouse) | 1–2 | £2 each | Keyboard/mouse → RP2040 USB host via USB-A adapter |
| Jumper wire (crossed) | 2 wires | <£1 | GP0/GP1 UART between boards |
| TI ISO7721DR (optional) | 1 | £2 | Galvanic isolator between UART lines |

**Total: ~£15–20**

---

## Firmware build and flashing

| Binary | Board | Description |
|--------|-------|-------------|
| `dualie_board_A.uf2` | RP2040 Zero → Machine A | Output-A firmware: HID + CDC-ACM composite |
| `dualie_board_B.uf2` | RP2040 Zero → Machine B | Output-B firmware: HID + CDC-ACM composite |

CDC-ACM is always compiled in.

### Flashing both boards from one machine

```shell
just flash
```

This is the only command needed, run from Machine A (the dev machine):

1. Sends `DualieMessage::RebootToBootloader` to RP2040-A over CDC-ACM serial
2. RP2040-A calls `reset_usb_boot(0, 0)` and appears as `RPI-RP2` USB drive
3. `just` copies `dualie_board_A.uf2` to the drive
4. RP2040-A verifies and flashes itself, then reboots
5. RP2040-A automatically transfers the firmware image to RP2040-B over the inter-board UART
6. RP2040-B flashes itself and reboots

No physical access to the far machine or RP2040-B is required. This reuses the
cross-board upgrade mechanism from the original DeskHop firmware.

---

## Security notes

- All daemon↔RP2040 communication travels over the private CDC-ACM serial device. It is a
  character device exclusively held open by the daemon — no IP stack, no network routing,
  no possibility of another process piggybacking on it.
- Remapping is applied only to physical key events. No keystrokes are synthesised without
  a corresponding physical key press.
- The daemon does not store or log keystrokes. No input history is retained.
