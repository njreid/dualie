# Homebrew cask for the standalone Karabiner-DriverKit-VirtualHIDDevice
# driver — capshift's dependency for injecting keystrokes, without needing
# the full Karabiner-Elements app.
#
# Upstream: https://github.com/pqrs-org/Karabiner-DriverKit-VirtualHIDDevice
#
# One-time setup: seed the tap repo with this file:
#   cp homebrew/Casks/karabiner-driverkit-virtualhiddevice.rb \
#     <tap-repo>/Casks/karabiner-driverkit-virtualhiddevice.rb
#
# This cask is NOT auto-updated by capshift's release CI — bump `version`
# and `sha256` by hand when pqrs-org cuts a new driver release.

cask "karabiner-driverkit-virtualhiddevice" do
  version "8.2.0"
  sha256 "7faf4c33046c2274726da9e29da795fb2d2ad81796557db0fcc1686c611eeafc"

  url "https://github.com/pqrs-org/Karabiner-DriverKit-VirtualHIDDevice/releases/download/v#{version}/Karabiner-DriverKit-VirtualHIDDevice-#{version}.pkg"
  name "Karabiner-DriverKit-VirtualHIDDevice"
  desc "Standalone virtual keyboard/mouse DriverKit extension used by capshift"
  homepage "https://github.com/pqrs-org/Karabiner-DriverKit-VirtualHIDDevice"

  depends_on macos: :big_sur

  pkg "Karabiner-DriverKit-VirtualHIDDevice-#{version}.pkg"

  uninstall pkgutil: "org.pqrs.Karabiner-DriverKit-VirtualHIDDevice",
            delete:  [
              "/Applications/.Karabiner-VirtualHIDDevice-Manager.app",
              "/Library/Application Support/org.pqrs/Karabiner-DriverKit-VirtualHIDDevice",
            ]

  caveats <<~EOS
    This installs only the DriverKit virtual-keyboard driver, not the
    Karabiner-Elements app. Two manual steps are still required — macOS
    requires interactive approval for driver extensions, so these can't
    be automated by the installer:

    1. Activate the driver extension (this triggers a macOS approval
       prompt under System Settings → General → Login Items & Extensions
       → Driver Extensions — you must approve it there):
         /Applications/.Karabiner-VirtualHIDDevice-Manager.app/Contents/MacOS/Karabiner-VirtualHIDDevice-Manager activate

    2. Install capshift, then follow its caveats to install the packaged
       root LaunchDaemon that starts and supervises this daemon at boot:
         brew info capshift
  EOS
end
