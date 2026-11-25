class Stackrig < Formula
  desc "stackrig"
  homepage "https://github.com/tschuba/stackrig"
  url "https://github.com/tschuba/stackrig.git", tag: "v0.1.0" # x-release-please-version
  head "https://github.com/tschuba/stackrig.git", branch: "master"

  depends_on "rust" => :build

  def install
    system "cargo", "install", *std_cargo_args
  end

  test do
    assert_predicate bin/"strig", :exist?
  end
end
