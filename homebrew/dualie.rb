# Homebrew formula for the Dualie daemon.
#
# To create a tap:
#   gh repo create dualie-dev/homebrew-dualie --public
#   cp homebrew/dualie.rb <tap-repo>/Formula/dualie.rb
#
# Users install with:
#   brew tap dualie-dev/dualie
#   brew install dualie

class Dualie < Formula
  desc "KVM switch daemon for Dualie – serves config UI and dispatches virtual key actions"
  homepage "https://github.com/dualie-dev/dualie"
  version "0.1.0"

  # SHA256 checksums are filled in by CI when a release is tagged.
  # Run: shasum -a 256 dualie-<version>-<arch>.tar.gz
  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/dualie-dev/dualie/releases/download/v#{version}/dualie-#{version}-aarch64-apple-darwin.tar.gz"
      sha256 "0000000000000000000000000000000000000000000000000000000000000000"
    else
      url "https://github.com/dualie-dev/dualie/releases/download/v#{version}/dualie-#{version}-x86_64-apple-darwin.tar.gz"
      sha256 "0000000000000000000000000000000000000000000000000000000000000000"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/dualie-dev/dualie/releases/download/v#{version}/dualie-#{version}-aarch64-unknown-linux-musl.tar.gz"
      sha256 "0000000000000000000000000000000000000000000000000000000000000000"
    else
      url "https://github.com/dualie-dev/dualie/releases/download/v#{version}/dualie-#{version}-x86_64-unknown-linux-musl.tar.gz"
      sha256 "0000000000000000000000000000000000000000000000000000000000000000"
    end
  end

  def install
    bin.install "dualie"
  end

  # Register the launchd user agent on macOS.
  # On Linux the user runs `systemctl --user enable --now dualie` manually,
  # or uses `just install` from the repo.
  def caveats
    on_macos do
      <<~EOS
        To start dualie now and restart at login:
          brew services start dualie

        Accessibility permission is required for virtual key interception:
          System Settings → Privacy & Security → Accessibility → add dualie

        Config UI: http://localhost:7474
      EOS
    end
    on_linux do
      <<~EOS
        To enable as a systemd user service:
          systemctl --user enable --now dualie

        Config UI: http://localhost:7474
      EOS
    end
  end

  service do
    run [opt_bin/"dualie"]
    keep_alive true
    log_path var/"log/dualie.log"
    error_log_path var/"log/dualie.err"
    # Run as the current user so it can access the Accessibility event tap
    # and the user's config directory.
  end

  test do
    # Smoke-test: binary starts, prints version, exits cleanly
    assert_match version.to_s, shell_output("#{bin}/dualie --version")
  end
end
