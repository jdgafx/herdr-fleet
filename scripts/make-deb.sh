#!/bin/sh
# Build a .deb from a built herdr-fleet binary.
#   scripts/make-deb.sh <version> <path-to-binary> [output-dir]
# Produces herdr-fleet_<version>_amd64.deb installing to /usr/bin/herdr-fleet.
set -eu

version="${1:?usage: make-deb.sh <version> <binary> [outdir]}"
binary="${2:?usage: make-deb.sh <version> <binary> [outdir]}"
outdir="${3:-.}"

stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT

mkdir -p "$stage/DEBIAN" "$stage/usr/bin"
install -m 755 "$binary" "$stage/usr/bin/herdr-fleet"

cat > "$stage/DEBIAN/control" <<EOF
Package: herdr-fleet
Version: $version
Section: utils
Priority: optional
Architecture: amd64
Maintainer: Chris <jdgafx@users.noreply.github.com>
Homepage: https://github.com/jdgafx/herdr-fleet
Description: Live fleet dashboard for herdr (Rust TUI)
Shows every herdr agent pane with status, model, context and cost,
with a background refresh worker so keys never wait on herdr.
EOF

out="$outdir/herdr-fleet_${version}_amd64.deb"
dpkg-deb --build --root-owner-group "$stage" "$out" >/dev/null
(cd "$(dirname "$out")" && sha256sum "$(basename "$out")" > "$(basename "$out").sha256")
echo "$out"
