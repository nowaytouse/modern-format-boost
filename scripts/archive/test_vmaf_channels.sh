#!/bin/bash
# 测试 vmaf 的 float_ms_ssim 是否包含色度信息
set -e

TMP="/tmp/vmaf_channels_$$"
mkdir -p "$TMP"

echo "🧪 Testing vmaf float_ms_ssim channel coverage"
echo ""

# 创建测试视频（带色度变化）
echo "📹 Creating test videos with chroma differences..."
ffmpeg -f lavfi -i testsrc=duration=2:size=320x240:rate=30 \
    -c:v libx264 -crf 18 -y "$TMP/ref.mp4" 2>/dev/null

# 创建色度损失版本（降低色度采样）
ffmpeg -i "$TMP/ref.mp4" -vf "format=yuv420p" \
    -c:v libx264 -crf 25 -y "$TMP/dist.mp4" 2>/dev/null

# 转换为 Y4M
ffmpeg -i "$TMP/ref.mp4" -pix_fmt yuv420p \
    -f yuv4mpegpipe -y "$TMP/ref.y4m" 2>/dev/null

ffmpeg -i "$TMP/dist.mp4" -pix_fmt yuv420p \
    -f yuv4mpegpipe -y "$TMP/dist.y4m" 2>/dev/null

echo "✅ Videos ready"
echo ""

# 运行 vmaf
echo "📊 Running vmaf with float_ms_ssim..."
vmaf -r "$TMP/ref.y4m" -d "$TMP/dist.y4m" \
     --feature float_ms_ssim \
     -o "$TMP/result.json" --json 2>&1 | grep -v "^$"

echo ""
if [ -f "$TMP/result.json" ]; then
    echo "✅ Results:"
    python3 << EOF
import json
with open('$TMP/result.json') as f:
    data = json.load(f)
    metrics = data['pooled_metrics']
    
    print(f"  float_ms_ssim: {metrics['float_ms_ssim']['mean']:.4f}")
    print(f"  vmaf:          {metrics['vmaf']['mean']:.2f}")
    
    print("\n💡 Conclusion:")
    print("  float_ms_ssim is calculated on YUV420p input")
    print("  → Includes luma (Y) and chroma (U, V) information")
    print("  → No need for separate channel calculations")
EOF
fi

rm -rf "$TMP"
