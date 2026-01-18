#!/bin/bash
# 快速测试 VMAF 独立工具
set -e

echo "🧪 Quick VMAF Test"

# 创建测试视频
TMP="/tmp/vmaf_test_$$"
mkdir -p "$TMP"

echo "📹 Creating test videos..."
ffmpeg -f lavfi -i testsrc=duration=2:size=320x240:rate=30 \
    -c:v libx264 -crf 18 -y "$TMP/ref.mp4" 2>/dev/null

ffmpeg -i "$TMP/ref.mp4" -c:v libx264 -crf 25 \
    -y "$TMP/dist.mp4" 2>/dev/null

echo "✅ Videos created"

# 转换为 Y4M
echo "🔄 Converting to Y4M..."
ffmpeg -i "$TMP/ref.mp4" -pix_fmt yuv420p \
    -f yuv4mpegpipe -y "$TMP/ref.y4m" 2>/dev/null

ffmpeg -i "$TMP/dist.mp4" -pix_fmt yuv420p \
    -f yuv4mpegpipe -y "$TMP/dist.y4m" 2>/dev/null

echo "✅ Y4M ready"

# 运行 VMAF
echo "📊 Running vmaf..."
vmaf --reference "$TMP/ref.y4m" \
     --distorted "$TMP/dist.y4m" \
     --model version=vmaf_float_v0.6.1 \
     --feature float_ms_ssim \
     --output "$TMP/result.json" \
     --json

echo "✅ VMAF complete"

# 解析结果
if [ -f "$TMP/result.json" ]; then
    echo ""
    echo "📊 Results:"
    python3 << 'EOF'
import json
with open('/tmp/vmaf_test_$$/result.json'.replace('$$', str(__import__('os').getppid()))) as f:
    data = json.load(f)
    ms_ssim = data['pooled_metrics']['float_ms_ssim']['mean']
    vmaf = data['pooled_metrics']['vmaf']['mean']
    print(f"  MS-SSIM: {ms_ssim:.4f}")
    print(f"  VMAF:    {vmaf:.2f}")
EOF
fi

# 清理
rm -rf "$TMP"
echo ""
echo "✅ Test complete!"
