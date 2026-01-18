#!/bin/bash
set -e
cd "$(dirname "$0")/.."

echo "🔨 Building with ffmpeg libvmaf priority..."
cd shared_utils && cargo build --release 2>&1 | tail -5
cd ..

echo ""
echo "✅ Build complete"
echo ""
echo "🧪 Testing MS-SSIM calculation..."

# 创建测试视频
TMP="/tmp/priority_test_$$"
mkdir -p "$TMP"

ffmpeg -f lavfi -i testsrc=duration=2:size=320x240:rate=30 \
    -c:v libx264 -crf 18 -y "$TMP/ref.mp4" 2>/dev/null

ffmpeg -i "$TMP/ref.mp4" -c:v libx264 -crf 25 \
    -y "$TMP/dist.mp4" 2>/dev/null

echo "✅ Test videos ready"
echo ""

# 直接测试 calculate_ms_ssim 函数（通过 vidquality_hevc）
cd vidquality_hevc
echo "📊 Running quality verification..."
cargo run --release -- analyze "$TMP/ref.mp4" 2>&1 | grep -A 5 "MS-SSIM" || echo "No MS-SSIM output"

rm -rf "$TMP"

echo ""
echo "💡 Check output above for 'ffmpeg libvmaf' priority"
