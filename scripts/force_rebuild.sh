#!/bin/bash
# 强制重新编译所有项目
set -e

cd "$(dirname "$0")/.."

echo "🧹 Cleaning all build artifacts..."
cargo clean
echo ""

echo "🔨 Force rebuilding imgquality-hevc..."
cargo build --release --manifest-path imgquality_hevc/Cargo.toml
echo ""

BINARY="target/release/imgquality-hevc"
echo "✅ Build complete!"
echo "📦 Binary: $BINARY"
ls -lh "$BINARY"
echo "   Timestamp: $(date -r $(stat -f "%m" "$BINARY") '+%Y-%m-%d %H:%M:%S')"
echo ""

echo "🧪 Testing version..."
./"$BINARY" --version
