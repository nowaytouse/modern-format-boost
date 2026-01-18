#!/bin/bash
set -e
cd "$(dirname "$0")/.."

echo "🔨 Building v7.4.2..."
cargo build --release --manifest-path imgquality_hevc/Cargo.toml 2>&1 | tail -20

BINARY="target/release/imgquality-hevc"
echo ""
echo "✅ Binary: $BINARY"
ls -lh "$BINARY"
date -r $(stat -f "%m" "$BINARY") '+Time: %Y-%m-%d %H:%M:%S'
