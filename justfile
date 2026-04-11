# Dualie – justfile
# Usage: just <recipe>
# Install just: brew install just
#
# Top-level recipes delegate to firmware/, daemon/, and web/ sub-projects.

default:
    @just --list

# ── Firmware ──────────────────────────────────────────────────────────────────

# Configure the firmware build (run once, or after CMakeLists changes)
firmware-configure:
    cmake -S firmware -B firmware/build -DCMAKE_BUILD_TYPE=Release

# Build the firmware .uf2
firmware-build: firmware-configure
    cmake --build firmware/build --parallel

# Build firmware unit tests (runs on host, no Pico required)
firmware-test:
    cmake -S firmware -B firmware/build-test -DDUALIE_HOST_TESTS=ON
    cmake --build firmware/build-test --parallel
    firmware/build-test/dualie_tests

# Flash firmware to a connected Pico (copies .uf2 to the mounted drive)
firmware-flash DRIVE="/Volumes/RPI-RP2":
    cp firmware/build/dualie.uf2 {{DRIVE}}/

# Clean firmware build artefacts
firmware-clean:
    rm -rf firmware/build firmware/build-test

# ── Daemon ────────────────────────────────────────────────────────────────────

# Build the daemon binary
daemon-build:
    cargo build --manifest-path daemon/Cargo.toml --release

# Run the daemon in development mode (recompiles on source changes)
daemon-dev:
    cd daemon && cargo watch -x 'run -- --dev'

# Run daemon tests
daemon-test:
    cargo test --manifest-path daemon/Cargo.toml

# Clean daemon build artefacts
daemon-clean:
    cargo clean --manifest-path daemon/Cargo.toml

# ── Web SPA ───────────────────────────────────────────────────────────────────

# Install web dependencies
web-install:
    cd web && npm install

# Start the Vite dev server (proxies /api to daemon on 7474)
web-dev: web-install
    cd web && npm run dev

# Build the SPA bundle (output goes to daemon/src/web/static for embedding)
web-build: web-install
    cd web && npm run build

# Clean web build artefacts
web-clean:
    rm -rf web/node_modules daemon/src/web/static/assets
    # Restore the placeholder index.html so `cargo check` still works
    echo '<!doctype html><html><head><meta charset="utf-8"><title>Dualie</title></head><body><div id="app"></div></body></html>' \
        > daemon/src/web/static/index.html

# ── Combined ──────────────────────────────────────────────────────────────────

# Build everything (firmware + daemon + web)
build: web-build daemon-build firmware-build

# Run all tests
test: firmware-test daemon-test

# Start daemon (with file-watch recompile) + Vite dev server concurrently.
# Requires: cargo install cargo-watch
dev:
    #!/usr/bin/env bash
    set -e
    if ! command -v cargo-watch &>/dev/null; then
        echo "cargo-watch not found — install it first:"
        echo "  cargo install cargo-watch"
        exit 1
    fi
    # Ensure web deps are present before forking
    cd web && npm install --silent && cd ..

    echo "Starting daemon (cargo watch) on :7474 …"
    cd daemon && cargo watch -x 'run -- --dev' &
    DAEMON_PID=$!
    cd ..

    echo "Starting Vite dev server on :5173 …"
    cd web && npm run dev &
    WEB_PID=$!
    cd ..

    trap "echo 'Shutting down…'; kill $DAEMON_PID $WEB_PID 2>/dev/null; wait" EXIT INT TERM
    wait $DAEMON_PID $WEB_PID

# Clean everything
clean: firmware-clean daemon-clean web-clean

# ── Install / uninstall (non-brew) ────────────────────────────────────────────

# Install daemon binary to ~/.local/bin and register the user service
install: daemon-build
    #!/usr/bin/env bash
    set -e
    DUALIE_BIN="${HOME}/.local/bin/dualie"
    install -Dm755 daemon/target/release/dualie "${DUALIE_BIN}"
    if [[ "$(uname)" == "Darwin" ]]; then
        PLIST_DEST="${HOME}/Library/LaunchAgents/dev.dualie.plist"
        mkdir -p "${HOME}/Library/LaunchAgents"
        sed "s|@DUALIE_BIN@|${DUALIE_BIN}|g" resources/dev.dualie.plist > "${PLIST_DEST}"
        launchctl unload "${PLIST_DEST}" 2>/dev/null || true
        launchctl load "${PLIST_DEST}"
        echo "Installed and loaded launchd service."
        echo "  → Add ${DUALIE_BIN} to System Settings → Privacy & Security → Accessibility"
    else
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
    else
        systemctl --user disable --now dualie.service 2>/dev/null || true
        rm -f "${HOME}/.config/systemd/user/dualie.service"
    fi
    rm -f "${HOME}/.local/bin/dualie"
    echo "Uninstalled."
