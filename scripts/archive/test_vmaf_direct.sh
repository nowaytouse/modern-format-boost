#!/bin/bash
set -e

TMP="/tmp/vmaf_direct_$$"
mkdir -p "$TMP"

# 创建测试视频
ffmpeg -f lavfi -i testsrc=duration=2:size=320x240:rate=30 \
    -c:v libx264 -crf 18 -y "$TMP/ref.mp4" 2>/dev/null

ffmpeg -i "$TMP/ref.mp4" -c:v libx264 -crf 25 \
    -y "$TMP/dist.mp4" 2>/dev/null

# 转换为 Y4M
ffmpeg -i "$TMP/ref.mp4" -pix_fmt yuv420p \
    -f yuv4mpegpipe -y "$TMP/ref.y4m" 2>/dev/null

ffmpeg -i "$TMP/dist.mp4" -pix_fmt yuv420p \
    -f yuv4mpegpipe -y "$TMP/dist.y4m" 2>/dev/null

echo "Testing different vmaf commands:"
echo ""

echo "1. Basic vmaf (no features):"
vmaf -r "$TMP/ref.y4m" -d "$TMP/dist.y4m" \
     -o "$TMP/test1.json" --json 2>&1 | head -5

echo ""
echo "2. With float_ms_ssim feature:"
vmaf -r "$TMP/ref.y4m" -d "$TMP/dist.y4m" \
     --feature float_ms_ssim \
     -o "$TMP/test2.json" --json 2>&1 | head -5

echo ""
if [ -f "$TMP/test2.json" ]; then
    echo "✅ Success! Checking output..."
    python3 -c "import json; d=json.load(open('$TMP/test2.json')); print('Keys:', list(d.get('pooled_metrics', {}).keys()))"
fi

rm -rf "$TMP"
