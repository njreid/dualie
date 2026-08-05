# typed: strict
# frozen_string_literal: true

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

# Installs the prebuilt capshift daemon and its virtual-HID service definition.
class Capshift < Formula
  desc "Caps-lock chord shortcut daemon for macOS — app launch/focus and key remaps"
  homepage "https://github.com/njreid/dualie"
  version "0.1.0"

  depends_on :macos

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/njreid/dualie/releases/download/capshift-v#{version}/capshift-#{version}-aarch64-apple-darwin.tar.gz"
      sha256 "0000000000000000000000000000000000000000000000000000000000000000"
    else
      odie "capshift only supports Apple Silicon (arm64) Macs"
    end
  end

  def install
    bin.install "capshift"
    pkgshare.install "dev.njreid.capshift.kvhd.plist"
  end

  def caveats
    <<~EOS
      First install the standalone virtual-HID driver if it is not installed:
        brew install --cask njreid/capshift/karabiner-driverkit-virtualhiddevice

      Install and start the root-only Karabiner VirtualHIDDevice daemon:
        sudo cp "#{opt_pkgshare}/dev.njreid.capshift.kvhd.plist" /Library/LaunchDaemons/
        sudo chown root:wheel /Library/LaunchDaemons/dev.njreid.capshift.kvhd.plist
        sudo chmod 644 /Library/LaunchDaemons/dev.njreid.capshift.kvhd.plist
        sudo launchctl bootstrap system /Library/LaunchDaemons/dev.njreid.capshift.kvhd.plist

      Then start capshift as a root service so it can access that daemon:
        sudo brew services start capshift

      Accessibility permission is required for keyboard interception:
        System Settings → Privacy & Security → Accessibility → add capshift

      The Karabiner-DriverKit-VirtualHIDDevice driver must still be approved
      interactively before its daemon can create a virtual keyboard — see:
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
