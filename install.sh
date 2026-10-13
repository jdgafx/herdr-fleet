#!/bin/sh
# herdr-fleet installer: downloads the prebuilt binary for this platform from
# GitHub Releases, verifies its SHA-256, and installs it to ~/.local/bin.
#
#   curl -fsSL https://raw.githubusercontent.com/jdgafx/herdr-fleet/main/install.sh | sh
#
# Env: INSTALL_DIR (default ~/.local/bin), VERSION (default: latest release).
set -eu

repo="https://github.com/jdgafx/herdr-fleet"
bin="herdr-fleet"
install_dir="${INSTALL_DIR:-$HOME/.local/bin}"

os="$(uname -s)"
arch="$(uname -m)"
case "$os-$arch" in
  Linux-x86_64)              target="x86_64-unknown-linux-gnu" ;;
  Linux-aarch64|Linux-arm64) target="aarch64-unknown-linux-gnu" ;;
  Darwin-arm64)              target="aarch64-apple-darwin" ;;
  Darwin-x86_64)             target="x86_64-apple-darwin" ;;
  *)
    echo "$bin: no prebuilt binary for $os-$arch." >&2
    echo "$bin: install from source with: cargo install --git $repo" >&2
    exit 1
    ;;
esac

base="$repo/releases/latest/download"
if [ -n "${VERSION:-}" ]; then
  base="$repo/releases/download/$VERSION"
fi

tarball="$bin-$target.tar.gz"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

echo "$bin: downloading $tarball"
curl -fsSL "$base/$tarball" -o "$tmp/$tarball"
curl -fsSL "$base/$tarball.sha256" -o "$tmp/$tarball.sha256"

if ! (cd "$tmp" && if command -v sha256sum >/dev/null 2>&1; then
        sha256sum -c "$tarball.sha256" >/dev/null
      else
        shasum -a 256 -c "$tarball.sha256" >/dev/null
      fi); then
  echo "$bin: checksum verification failed" >&2
  exit 1
fi

tar -xzf "$tmp/$tarball" -C "$tmp"
mkdir -p "$install_dir"
install -m 755 "$tmp/$bin" "$install_dir/$bin"

echo "$bin: installed to $install_dir/$bin"
case ":$PATH:" in
  *":$install_dir:"*) ;;
  *) echo "$bin: note: $install_dir is not in your PATH yet" ;;
esac
