#!/bin/bash
set -e

VERSION="${1:-0.1.1}"
ARCH="${2:-amd64}"
DEB_DIR="target/deb_pkg"
rm -rf "$DEB_DIR"
mkdir -p "$DEB_DIR/DEBIAN"
mkdir -p "$DEB_DIR/usr/bin"
mkdir -p "$DEB_DIR/usr/share/applications"

for sz in 16 32 48 64 128 256 512; do
  mkdir -p "$DEB_DIR/usr/share/icons/hicolor/${sz}x${sz}/apps"
  cp "assets/icons/logo_${sz}.png" "$DEB_DIR/usr/share/icons/hicolor/${sz}x${sz}/apps/amr.png" 2>/dev/null || true
done

cp target/release/amr "$DEB_DIR/usr/bin/amr"
chmod 755 "$DEB_DIR/usr/bin/amr"
cp packaging/linux/amr.desktop "$DEB_DIR/usr/share/applications/amr.desktop"

cat << EOF > "$DEB_DIR/DEBIAN/control"
Package: amr
Version: ${VERSION}
Section: utils
Priority: optional
Architecture: ${ARCH}
Maintainer: Qyzom <t0lotsmail@gmail.com>
Description: AMR (Antigravity Manager Rust)
 Ultra-lightweight AI API proxy, protocol emulator and multi-account quota dispatcher in pure Rust.
Homepage: https://github.com/Qyzom/Antigravity-Manager-Rust
EOF

OUT_NAME="AMR_${VERSION}_${ARCH}.deb"
BUILD_TMP="target/deb_tmp"
rm -rf "$BUILD_TMP"
mkdir -p "$BUILD_TMP"
echo "2.0" > "$BUILD_TMP/debian-binary"
tar --numeric-owner --owner=0 --group=0 -czf "$BUILD_TMP/control.tar.gz" -C "$DEB_DIR/DEBIAN" .
tar --numeric-owner --owner=0 --group=0 -czf "$BUILD_TMP/data.tar.gz" --exclude='./DEBIAN' -C "$DEB_DIR" .
(cd "$BUILD_TMP" && ar rcs "../$OUT_NAME" debian-binary control.tar.gz data.tar.gz)
mv "target/$OUT_NAME" "./$OUT_NAME"
echo "==> Debian package generated successfully: $OUT_NAME"
