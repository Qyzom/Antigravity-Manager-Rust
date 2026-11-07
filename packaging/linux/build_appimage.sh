#!/bin/bash
set -e

echo "==> Building AMR release binary..."
cargo build --release --bin amr

APPDIR="target/AppDir"
rm -rf "$APPDIR"
mkdir -p "$APPDIR/usr/bin"
for sz in 16 32 48 64 128 256 512; do
  mkdir -p "$APPDIR/usr/share/icons/hicolor/${sz}x${sz}/apps"
  cp "assets/icons/logo_${sz}.png" "$APPDIR/usr/share/icons/hicolor/${sz}x${sz}/apps/amr.png" 2>/dev/null || true
done
mkdir -p "$APPDIR/usr/share/applications"
mkdir -p "$APPDIR/usr/share/metainfo"
mkdir -p "$APPDIR/usr/share/appdata"

cp target/release/amr "$APPDIR/usr/bin/"
cp packaging/linux/amr.desktop "$APPDIR/usr/share/applications/"
cp packaging/linux/amr.desktop "$APPDIR/"
cp packaging/linux/amr.metainfo.xml "$APPDIR/usr/share/metainfo/amr.metainfo.xml"
cp packaging/linux/amr.metainfo.xml "$APPDIR/usr/share/metainfo/io.github.amr.metainfo.xml"
cp packaging/linux/amr.metainfo.xml "$APPDIR/usr/share/appdata/amr.appdata.xml"
cp assets/icons/logo_256.png "$APPDIR/amr.png"
cp assets/icons/logo_256.png "$APPDIR/.DirIcon"

cat << 'EOF' > "$APPDIR/AppRun"
#!/bin/sh
HERE="$(dirname "$(readlink -f "${0}")")"
export XDG_DATA_DIRS="$HERE/usr/share:${XDG_DATA_DIRS:-/usr/local/share:/usr/share}"
export APPDIR="$HERE"
exec "$HERE/usr/bin/amr" "$@"
EOF
chmod +x "$APPDIR/AppRun"

VERSION="${1:-0.1.3}"
ARCH="${2:-x86_64}"
echo "==> AppDir structure prepared at $APPDIR"
RUNTIME_FLAG=""
if [ -f "target/runtime-${ARCH}" ]; then
    RUNTIME_FLAG="--runtime-file target/runtime-${ARCH}"
fi

if command -v appimagetool &> /dev/null; then
    ARCH=${ARCH} appimagetool $RUNTIME_FLAG "$APPDIR" "AMR-${VERSION}-${ARCH}.AppImage"
    echo "==> AppImage created successfully: AMR-${VERSION}-${ARCH}.AppImage"
elif [ -f "target/squashfs-root/AppRun" ]; then
    ARCH=${ARCH} target/squashfs-root/AppRun --no-appstream $RUNTIME_FLAG "$APPDIR" "AMR-${VERSION}-${ARCH}.AppImage"
    echo "==> AppImage created successfully: AMR-${VERSION}-${ARCH}.AppImage"
else
    echo "Notice: appimagetool not found locally. Ready for CI packaging."
fi
