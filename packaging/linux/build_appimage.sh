#!/bin/bash
set -e

echo "==> Building Aroxy release binary..."
cargo build --release --bin aroxy

APPDIR="target/AppDir"
rm -rf "$APPDIR"
mkdir -p "$APPDIR/usr/bin"
for sz in 16 32 48 64 128 256 512; do
  mkdir -p "$APPDIR/usr/share/icons/hicolor/${sz}x${sz}/apps"
  cp "assets/icons/logo_${sz}.png" "$APPDIR/usr/share/icons/hicolor/${sz}x${sz}/apps/aroxy.png" 2>/dev/null || true
done
mkdir -p "$APPDIR/usr/share/applications"

cp target/release/aroxy "$APPDIR/usr/bin/"
cp packaging/linux/aroxy.desktop "$APPDIR/usr/share/applications/"
cp packaging/linux/aroxy.desktop "$APPDIR/"
cp assets/icons/logo_256.png "$APPDIR/aroxy.png"
cp assets/icons/logo_256.png "$APPDIR/.DirIcon"

cat << 'EOF' > "$APPDIR/AppRun"
#!/bin/sh
HERE="$(dirname "$(readlink -f "${0}")")"
exec "$HERE/usr/bin/aroxy" "$@"
EOF
chmod +x "$APPDIR/AppRun"

VERSION="${1:-0.0.4}"
echo "==> AppDir structure prepared at $APPDIR"
if command -v appimagetool &> /dev/null; then
    appimagetool "$APPDIR" "Aroxy-${VERSION}-x86_64.AppImage"
    echo "==> AppImage created successfully: Aroxy-${VERSION}-x86_64.AppImage"
else
    echo "Notice: appimagetool not found locally. Ready for CI packaging."
fi
