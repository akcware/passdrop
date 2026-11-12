# Template for the formula in github.com/akcware/homebrew-tap (Formula/passdrop.rb).
# After a release, set `url` to the new tag and `sha256` from the release's checksums.txt.
class Passdrop < Formula
  desc "Ask the human for secrets in a local window; values never enter the agent's chat"
  homepage "https://github.com/akcware/passdrop"
  url "https://github.com/akcware/passdrop/releases/download/v0.1.0/passdrop-v0.1.0-macos-universal.tar.gz"
  sha256 "REPLACE_WITH_SHA256_FROM_CHECKSUMS_TXT"
  license "MIT"

  depends_on :macos

  def install
    bin.install "passdrop"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/passdrop --version")
  end
end
