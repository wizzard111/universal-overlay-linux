#!/bin/sh
# Universal Overlay — release build for Linux.
#
# Output (everything inside source/target, the repository root is untouched):
#   target/package/universal-overlay-linux-<arch>/
#       install.sh  update.sh  uninstall.sh       <- the only files at the top
#       app/        program, Vulkan layer and GL hook (64 and 32 bit), GNOME extension
#   target/dist/universal-overlay-linux-<arch>.tar.gz   (what the updater downloads)
#   target/dist/SHA256SUMS-linux.txt
#
#   --print-package   print the package folder as the last line
# UO_GITHUB_REPO=owner/name — where the updater looks for releases.
# 32-bit libraries need: rustup target add i686-unknown-linux-gnu and gcc-multilib.
set -eu

SOURCE=$(cd "$(dirname "$0")/.." && pwd)
REPO=$(dirname "$SOURCE")
cd "$SOURCE"
VERSION=$(sed -n 's/^version *= *"\(.*\)"/\1/p' Cargo.toml | head -n 1)
NAME=universal-overlay-linux-$(uname -m)
PKG=$SOURCE/target/package/$NAME
APP=$PKG/app
DIST=$SOURCE/target/dist
T32=i686-unknown-linux-gnu
echo "== Universal Overlay $VERSION (Linux)" >&2
[ -n "${UO_GITHUB_REPO:-}" ] || echo "   UO_GITHUB_REPO not set: the updater uses wizzard111/universal-overlay-linux" >&2

cargo build --release --workspace >&2
HAVE32=0
if rustup target list --installed 2>/dev/null | grep -qx "$T32"; then
    if cargo build --release --target "$T32" -p uo-vklayer -p uo-glhook >&2; then HAVE32=1; fi
fi
[ $HAVE32 = 1 ] || echo "warning: 32-bit libraries not built (rustup target add $T32; apt install gcc-multilib)" >&2

rm -rf "$PKG"
mkdir -p "$APP/gnome-extension" "$DIST"
R=target/release
for f in universal-overlay universal-overlay-settings universal-overlay-updater uo-probe libuo_vklayer.so libuo_glhook.so; do
    cp "$R/$f" "$APP/"
done
if [ $HAVE32 = 1 ]; then
    cp "target/$T32/release/libuo_vklayer.so" "$APP/libuo_vklayer32.so"
    cp "target/$T32/release/libuo_glhook.so" "$APP/libuo_glhook32.so"
fi
cp resources/universal-overlay-run resources/icon.svg "$REPO/docs/README.md" "$APP/"
cp resources/gnome-shell-extension/* "$APP/gnome-extension/"
printf '%s\n' "$VERSION" > "$APP/VERSION"
cp "$REPO/install.sh" "$REPO/update.sh" "$REPO/uninstall.sh" "$PKG/"
chmod 755 "$PKG"/*.sh "$APP/universal-overlay" "$APP/universal-overlay-settings" "$APP/universal-overlay-updater" \
          "$APP/uo-probe" "$APP/universal-overlay-run"
chmod 644 "$APP"/*.so "$APP/icon.svg" "$APP/README.md" "$APP/VERSION" "$APP"/gnome-extension/*
strip "$APP"/*.so 2>/dev/null || true

tar -C "$SOURCE/target/package" -czf "$DIST/$NAME.tar.gz" "$NAME"
(cd "$DIST" && { sha256sum "$NAME.tar.gz" 2>/dev/null || shasum -a 256 "$NAME.tar.gz"; } > SHA256SUMS-linux.txt)
echo "== Done: $DIST/$NAME.tar.gz" >&2
if [ "${1:-}" = --print-package ]; then echo "$PKG"; fi
