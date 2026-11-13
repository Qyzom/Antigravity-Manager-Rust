#!/bin/bash
set -e

if [ ! -f target/release/amr ]; then
    echo "==> Building AMR release binary..."
    cargo build --release --bin amr
fi

VERSION="${1:-$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)}"
ARCH="${2:-x86_64}"
BUILD_DATE="$(date -u +%Y-%m-%d)"

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
cp packaging/linux/amr.desktop "$APPDIR/usr/share/applications/amr.desktop"
echo "X-AppImage-Version=${VERSION}" >> "$APPDIR/usr/share/applications/amr.desktop"
cp "$APPDIR/usr/share/applications/amr.desktop" "$APPDIR/amr.desktop"

sed -e "s/@VERSION@/${VERSION}/g" -e "s/@DATE@/${BUILD_DATE}/g" packaging/linux/amr.metainfo.xml > "$APPDIR/usr/share/metainfo/amr.metainfo.xml"
cp "$APPDIR/usr/share/metainfo/amr.metainfo.xml" "$APPDIR/usr/share/metainfo/io.github.amr.metainfo.xml"
cp "$APPDIR/usr/share/metainfo/amr.metainfo.xml" "$APPDIR/usr/share/appdata/amr.appdata.xml"
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
elif [ -f "squashfs-root/AppRun" ]; then
    ARCH=${ARCH} ./squashfs-root/AppRun --no-appstream $RUNTIME_FLAG "$APPDIR" "AMR-${VERSION}-${ARCH}.AppImage"
    echo "==> AppImage created successfully: AMR-${VERSION}-${ARCH}.AppImage"
else
    echo "Notice: appimagetool not found locally. Ready for CI packaging."
fi
