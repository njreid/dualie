# Homebrew formula for capshift — caps-lock chord shortcut daemon.
#
# Published in its own tap:
#   brew tap njreid/capshift
#   brew install capshift
#
# One-time setup: before the first `capshift-v*` release tag is pushed,
# seed the tap repo with this file:
#   cp homebrew/capshift.rb <tap-repo>/Formula/capshift.rb
# Thereafter CI patches version/sha256 in place on every release.

class Capshift < Formula
  desc "Caps-lock chord shortcut daemon for macOS — app launch/focus and key remaps"
  homepage "https://github.com/njreid/dualie"
  version "0.1.0"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/njreid/dualie/releases/download/capshift-v#{version}/capshift-#{version}-aarch64-apple-darwin.tar.gz"
      sha256 "0000000000000000000000000000000000000000000000000000000000000000"
    else
      odie "capshift only supports Apple Silicon (arm64) Macs"
    end
  end

  on_linux do
    odie "capshift only supports macOS"
  end

  # Standalone DriverKit virtual-keyboard driver (not the full
  # Karabiner-Elements app) — see Casks/karabiner-driverkit-virtualhiddevice.rb
  # in this tap. `brew install capshift` pulls it in automatically; see that
  # cask's caveats for the manual driver-activation and daemon steps macOS
  # requires (these can't be automated — driver extensions need interactive
  # approval in System Settings).
  depends_on cask: "njreid/capshift/karabiner-driverkit-virtualhiddevice"

  def install
    bin.install "capshift"
  end

  def caveats
    <<~EOS
      To start capshift now and restart at login:
        brew services start capshift

      Accessibility permission is required for keyboard interception:
        System Settings → Privacy & Security → Accessibility → add capshift

      The Karabiner-DriverKit-VirtualHIDDevice driver (installed as a
      dependency of this formula) must be activated and its daemon running
      before capshift can inject keystrokes — see:
        brew info --cask njreid/capshift/karabiner-driverkit-virtualhiddevice

      Config file: ~/.config/capshift/config.kdl
    EOS
  end

  service do
    run [opt_bin/"capshift"]
    keep_alive true
    log_path var/"log/capshift.log"
    error_log_path var/"log/capshift.err"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/capshift --version")
  end
end
