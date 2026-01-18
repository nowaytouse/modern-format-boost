#!/bin/bash
# 测试 ffmpeg libvmaf 的 MS-SSIM 功能
set -e

TMP="/tmp/ffmpeg_libvmaf_test_$$"
mkdir -p "$TMP"

echo "🧪 Testing FFmpeg libvmaf MS-SSIM"
echo "=================================="

# 创建测试视频
ffmpeg -f lavfi -i testsrc=duration=2:size=320x240:rate=30 \
    -c:v libx264 -crf 18 -y "$TMP/ref.mp4" 2>/dev/null

ffmpeg -i "$TMP/ref.mp4" -c:v libx264 -crf 25 \
    -y "$TMP/dist.mp4" 2>/dev/null

echo "✅ Test videos ready"
echo ""

# 测试 libvmaf MS-SSIM
echo "📊 Testing libvmaf with float_ms_ssim..."
ffmpeg -i "$TMP/ref.mp4" -i "$TMP/dist.mp4" \
    -lavfi "[0:v][1:v]libvmaf=feature='name=float_ms_ssim':log_fmt=json:log_path=$TMP/result.json" \
    -f null - 2>&1 | grep -E "(ms_ssim|error)" || true

echo ""
if [ -f "$TMP/result.json" ]; then
    echo "✅ libvmaf succeeded!"
    python3 -c "import json; d=json.load(open('$TMP/result.json')); print('MS-SSIM:', d['pooled_metrics']['float_ms_ssim']['mean'])"
else
    echo "❌ libvmaf failed"
fi

rm -rf "$TMP"
