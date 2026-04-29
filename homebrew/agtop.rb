class Agtop < Formula
  desc "Terminal UI for monitoring AI coding agents (Claude Code, Codex, Aider, ...)"
  homepage "https://github.com/mbrassey/agtop"
  url "https://github.com/mbrassey/agtop/archive/refs/tags/v2.0.0.tar.gz"
  sha256 "REPLACE_WITH_RELEASE_SHA256"
  license "MIT"
  head "https://github.com/mbrassey/agtop.git", branch: "main"

  depends_on "rust" => :build

  def install
    system "cargo", "install", *std_cargo_args
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/agtop --version")
    assert_match "claude", shell_output("#{bin}/agtop --list-builtins")
  end
end
