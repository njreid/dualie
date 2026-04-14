# Dualie

A two-machine USB KVM switch built on two RP2040 microcontrollers, with a
host-side daemon that adds key remapping, a caps-lock shortcut layer, clipboard
sync, and config-file sync across machines.

Each board presents as a USB HID keyboard/mouse + CDC-ACM serial device to its
host. The daemon talks to the RP2040 over the serial channel and applies
the same remap config to every keyboard on the machine — built-in, external,
and the one connected through the hardware switch.

---

## Huge thanks to DeskHop

> **Dualie would not exist without [DeskHop by Hrvoje Čavrak](https://github.com/hrvach/deskhop).**
>
> The entire hardware design, PCB, case, RP2040 dual-board USB architecture, UART protocol,
> mouse absolute-coordinate trick, galvanic isolation approach, TinyUSB integration, PIO-USB
> host stack, firmware safety model — all of it comes directly from DeskHop.
>
> Hrvoje built something genuinely clever, kept it completely open, documented it beautifully,
> and then gave it away for free. The dualie additions are a thin layer on top of an
> exceptional foundation. Please ⭐ the [original repo](https://github.com/hrvach/deskhop),
> consider [donating to Doctors Without Borders](https://donate.doctorswithoutborders.org/secure/donate)
> as Hrvoje suggests, and go read his code — it's a pleasure.

---

## Features

| Feature | Description |
|---------|-------------|
| **KVM switching** | Switch keyboard and mouse between two machines via caps-layer shortcut or hardware button |
| **Key remapping** | Remap any key or modifier on both outputs; applied to every keyboard on the machine |
| **Caps-lock layer** | Hold caps-lock to activate a shortcut layer (chords, app launches, output switching) |
| **Virtual actions** | RP2040 sends `VirtualAction` over CDC-ACM serial; daemon dispatches app-launch or shell commands |
| **Clipboard sync** | Caps+V pulls the other machine's clipboard over the serial link |
| **Config-file sync** | Watched app config files synced between machines over serial with three-way merge |
| **Git versioning** | Config repo backed by git; pull/push from the TUI |
| **Local remap everywhere** | Linux: evdev+uinput; macOS: IOHIDManager + Karabiner-VirtualHIDDevice |

---

## Hardware

Two RP2040 boards connected by UART (optionally via an ISO7721 galvanic
isolator). Each board has a USB-C device port (to the host PC) and a USB-A
host port (keyboard/mouse via PIO-USB).

See **[HARDWARE.md](HARDWARE.md)** for pin assignments, BOM, and flashing
instructions.

---

## Daemon

### Install

```shell
just install
```

Builds the daemon in release mode, installs to `~/.local/bin/dualie`, and
registers a systemd user service (Linux) or launchd agent (macOS).

### Config

Config lives at `~/.config/dualie/dualie.kdl` and is hot-reloaded on save.

```kdl
// dualie.kdl — example config

output A {
    remap {
        key capslock esc
        modifier lalt rctrl      // swap Alt and Ctrl on this output
    }

    layers {
        caps {
            chord  h left        // caps+H → Left arrow
            chord  l right       // caps+L → Right arrow
            chord  k up
            chord  j down
            action s "Slack"     // caps+S → launch Slack
            swap   n             // caps+N → toggle output
        }
    }
}

output B {}

sync {
    app "fish"
    app "neovim"
    app "git"
}
```

### TUI

```shell
dualie-tui
```

Tabs: **Status** · **Remaps** · **Caps Layer** · **Config** · **Sync**

Press `p` to pull from the git remote, `u` to push.

---

## Firmware

### Build

```shell
just firmware-build
```

Requires the Pico SDK (submodule) and a CMake toolchain with `arm-none-eabi-gcc`.

### Flash

```shell
just flash
```

Sends `RebootToBootloader` to board A over CDC-ACM, waits for the `RPI-RP2`
drive, copies the `.uf2`, then board A auto-flashes board B over the inter-board
UART. No physical access to the far machine required.

---

## Project layout

```
daemon/          Rust daemon (key remap, serial peer, config sync, git versioning)
proto/           Shared message types (DualieMessage — CBOR over COBS)
tui/             Terminal UI (dualie-tui)
hub/             Unused hub code (archived)
src/             RP2040 firmware (C, TinyUSB)
resources/       systemd service + launchd plist
homebrew/        Homebrew formula
```

---

## License

[GPLv3](LICENSE) — same as DeskHop.
