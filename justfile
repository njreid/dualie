# Dualie – justfile
# Usage: just <recipe>
# Install just: cargo install just  OR  brew install just
#
# Recipes are grouped: firmware → daemon → combined.

default:
    @just --list

# ── Firmware ──────────────────────────────────────────────────────────────────

# Configure the firmware build (run once, or after CMakeLists changes)
firmware-configure:
    cmake -S . -B build -DCMAKE_BUILD_TYPE=Release

# Build the firmware .uf2
firmware-build: firmware-configure
    cmake --build build --parallel

# Build firmware unit tests (runs on host, no Pico required)
firmware-test:
    cmake -S . -B build-test -DDUALIE_HOST_TESTS=ON
    cmake --build build-test --parallel
    build-test/dualie_tests

# Clean firmware build artefacts
firmware-clean:
    rm -rf build build-test

# Flash firmware to both boards from Machine A.
#
# Steps:
#   1. Build firmware
#   2. Send RebootToBootloader over CDC-ACM → RP2040-A enters USB MSC mode
#   3. Wait for the RPI-RP2 drive to appear
#   4. Copy the .uf2 — RP2040-A flashes itself, then auto-flashes RP2040-B
#      over the inter-board UART (DeskHop cross-board upgrade mechanism)
#
# Override the serial device with: just flash SERIAL=/dev/ttyACM1
SERIAL := ""
flash: firmware-build
    #!/usr/bin/env bash
    set -euo pipefail

    SERIAL_FLAG=""
    if [ -n "{{SERIAL}}" ]; then
        SERIAL_FLAG="--serial {{SERIAL}}"
    fi

    echo "→ Sending RebootToBootloader to RP2040-A …"
    cargo run -q --bin dualie -- $SERIAL_FLAG --serial-cmd reboot-to-bootloader

    echo "→ Waiting for RPI-RP2 bootloader drive …"
    for i in $(seq 1 40); do
        if [ -b /dev/disk/by-label/RPI-RP2 ] 2>/dev/null; then
            DRIVE=$(readlink -f /dev/disk/by-label/RPI-RP2)
            MOUNT=$(findmnt -n -o TARGET --source "$DRIVE" 2>/dev/null || true)
            if [ -z "$MOUNT" ]; then
                MOUNT=$(udisksctl mount -b "$DRIVE" --no-user-interaction 2>&1 | grep -oP 'at \K\S+')
            fi
            break
        fi
        if [ -d /run/media/$USER/RPI-RP2 ]; then
            MOUNT=/run/media/$USER/RPI-RP2
            break
        fi
        sleep 0.5
    done

    if [ -z "${MOUNT:-}" ]; then
        echo "ERROR: RPI-RP2 drive did not appear after 20s" >&2
        exit 1
    fi

    echo "→ Flashing dualie_board_A.uf2 to $MOUNT …"
    cp build/dualie_board_A.uf2 "$MOUNT/"
    sync

    echo "✓ RP2040-A flashed. It will now auto-flash RP2040-B over UART."
    echo "  Both boards reboot automatically when done."

# ── Daemon ────────────────────────────────────────────────────────────────────

# Build the daemon binary, root input daemon, and dua CLI/TUI
daemon-build:
    cargo build --release -p dualie -p dua

# Run the daemon
daemon-run:
    cargo run -p dualie

# Run daemon tests
daemon-test:
    cargo test --workspace

# Clean daemon build artefacts
daemon-clean:
    cargo clean

# ── Combined ──────────────────────────────────────────────────────────────────

# Build everything (firmware + daemon)
build: firmware-build daemon-build

# Run all tests
test: firmware-test daemon-test

# ── Install / uninstall ───────────────────────────────────────────────────────

# Install daemon binary and dua CLI/TUI to ~/.local/bin, register services
install: daemon-build
    #!/usr/bin/env bash
    set -e
    DUALIE_BIN="${HOME}/.local/bin/dualie"
    install -Dm755 target/release/dualie "${DUALIE_BIN}"
    install -Dm755 target/release/dua "${HOME}/.local/bin/dua"
    if [[ "$(uname)" == "Darwin" ]]; then
        # ── Root input daemon (LaunchDaemon — runs as root) ────────────────────
        INPUT_BIN="/usr/local/bin/dualie-input"
        sudo install -Dm755 target/release/dualie-input "${INPUT_BIN}"
        INPUT_PLIST="/Library/LaunchDaemons/dev.dualie.input.plist"
        sed "s|@INPUT_BIN@|${INPUT_BIN}|g" resources/dev.dualie.input.plist \
            | sudo tee "${INPUT_PLIST}" > /dev/null
        sudo launchctl unload "${INPUT_PLIST}" 2>/dev/null || true
        sudo launchctl load "${INPUT_PLIST}"
        echo "Installed root input daemon (${INPUT_BIN})."
        echo "  → Add ${INPUT_BIN} to System Settings → Privacy & Security → Accessibility"

        # ── User daemon (LaunchAgent — runs as current user) ───────────────────
        PLIST_DEST="${HOME}/Library/LaunchAgents/dev.dualie.plist"
        mkdir -p "${HOME}/Library/LaunchAgents"
        sed "s|@DUALIE_BIN@|${DUALIE_BIN}|g" resources/dev.dualie.plist > "${PLIST_DEST}"
        launchctl unload "${PLIST_DEST}" 2>/dev/null || true
        launchctl load "${PLIST_DEST}"
        echo "Installed and loaded user daemon (${DUALIE_BIN})."
    else
        # Add user to input group if not already a member (for evdev access)
        if ! groups | grep -q '\binput\b'; then
            echo "  → Adding $(whoami) to the 'input' group (evdev access for local remap)"
            sudo usermod -aG input "$(whoami)"
            echo "  → Log out and back in for group membership to take effect"
        fi
        mkdir -p "${HOME}/.config/systemd/user"
        cp resources/dualie.service "${HOME}/.config/systemd/user/"
        systemctl --user daemon-reload
        systemctl --user enable --now dualie.service
        echo "Installed and enabled systemd user service."
    fi

# Uninstall daemon and service
uninstall:
    #!/usr/bin/env bash
    if [[ "$(uname)" == "Darwin" ]]; then
        launchctl unload "${HOME}/Library/LaunchAgents/dev.dualie.plist" 2>/dev/null || true
        rm -f "${HOME}/Library/LaunchAgents/dev.dualie.plist"
        sudo launchctl unload /Library/LaunchDaemons/dev.dualie.input.plist 2>/dev/null || true
        sudo rm -f /Library/LaunchDaemons/dev.dualie.input.plist /usr/local/bin/dualie-input
    else
        systemctl --user disable --now dualie.service 2>/dev/null || true
        rm -f "${HOME}/.config/systemd/user/dualie.service"
    fi
    rm -f "${HOME}/.local/bin/dualie" "${HOME}/.local/bin/dua"
    echo "Uninstalled."
