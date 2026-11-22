#!/bin/sh
# Builds dist/tunnelrat-pc.tar.gz: prebuilt turad and tura plus everything
# pc/setup.sh needs, for PCs without Rust or the source. On the other PC:
#   tar xzf tunnelrat-pc.tar.gz && tunnelrat/pc/setup.sh
# SPDX-License-Identifier: GPL-3.0-only
set -eu
here=$(cd "$(dirname "$0")" && pwd)
repo=$(dirname "$here")
cargo=$(command -v cargo || echo "$HOME/.cargo/bin/cargo")
(cd "$here" && "$cargo" build --release --quiet)
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
t="$stage/tunnelrat"
mkdir -p "$t/pc/bin" "$t/art"
cp "$here/target/release/turad" "$here/target/release/tura" "$t/pc/bin/"
cp -r "$here/setup.sh" "$here/packaging" "$here/viewer" "$here/app" "$here/gnome-extension" "$t/pc/"
cp "$repo/art/app-icon.png" "$t/art/"
cp "$repo/LICENSE" "$repo/README.md" "$t/"
find "$t" -name __pycache__ -prune -exec rm -rf {} +
mkdir -p "$repo/dist"
tar -C "$stage" -czf "$repo/dist/tunnelrat-pc.tar.gz" tunnelrat
echo "$repo/dist/tunnelrat-pc.tar.gz ($(du -h "$repo/dist/tunnelrat-pc.tar.gz" | cut -f1))"
