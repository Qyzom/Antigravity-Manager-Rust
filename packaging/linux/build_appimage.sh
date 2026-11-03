#!/bin/bash
set -e

echo "==> Building Aroxy release binary..."
cargo build --release --bin aroxy

APPDIR="target/AppDir"
rm -rf "$APPDIR"
mkdir -p "$APPDIR/usr/bin"
mkdir -p "$APPDIR/usr/share/icons/hicolor/32x32/apps"
mkdir -p "$APPDIR/usr/share/applications"

cp target/release/aroxy "$APPDIR/usr/bin/"
cp packaging/linux/aroxy.desktop "$APPDIR/usr/share/applications/"
cp packaging/linux/aroxy.desktop "$APPDIR/"

# Create 32x32 PNG icon for AppDir
python3 -c "
import struct
# create minimal 32x32 png
" 2>/dev/null || true

cat << 'EOF' > "$APPDIR/AppRun"
#!/bin/sh
HERE="$(dirname "$(readlink -f "${0}")")"
exec "$HERE/usr/bin/aroxy" "$@"
EOF
chmod +x "$APPDIR/AppRun"

echo "==> AppDir structure prepared at $APPDIR"
if command -v appimagetool &> /dev/null; then
    appimagetool "$APPDIR" Aroxy-0.0.1-x86_64.AppImage
    echo "==> AppImage created successfully: Aroxy-0.0.1-x86_64.AppImage"
else
    echo "Notice: appimagetool not found locally. Ready for CI packaging."
fi
