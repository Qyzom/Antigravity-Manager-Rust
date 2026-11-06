#!/bin/bash
set -e

VERSION="${1:-0.1.1}"
ARCH="${2:-x86_64}"
OUT_FILE="AMR-${VERSION}-1.${ARCH}.rpm"

echo "==> Generating RPM package for AMR..."
if command -v cargo-generate-rpm &> /dev/null; then
    cargo-generate-rpm --auto-req disabled -o "$OUT_FILE"
elif [ -f "target/cargo_tools/bin/cargo-generate-rpm" ]; then
    ./target/cargo_tools/bin/cargo-generate-rpm --auto-req disabled -o "$OUT_FILE"
else
    echo "Installing cargo-generate-rpm..."
    cargo install cargo-generate-rpm --root target/cargo_tools --locked
    ./target/cargo_tools/bin/cargo-generate-rpm --auto-req disabled -o "$OUT_FILE"
fi

echo "==> RPM package created successfully: $OUT_FILE"
