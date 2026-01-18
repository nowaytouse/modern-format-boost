#!/bin/bash
# 验证 ffmpeg libvmaf 的色度敏感性
set -e

TMP="/tmp/ffmpeg_libvmaf_chroma_$$"
mkdir -p "$TMP"

echo "🔬 FFmpeg libvmaf Chroma Sensitivity Test"
echo "=========================================="
echo ""

# 1. 创建参考视频
echo "📹 Creating reference video..."
ffmpeg -f lavfi -i testsrc=duration=2:size=320x240:rate=30 \
    -c:v libx264 -crf 0 -pix_fmt yuv420p -y "$TMP/ref.mp4" 2>/dev/null
echo "✅ Reference ready"

# 2. Y-only 降级
echo ""
echo "📹 Creating Y-only degraded video..."
ffmpeg -i "$TMP/ref.mp4" \
    -vf "extractplanes=y+u+v[y][u][v];[y]lutyuv=y='val*0.9'[y2];[y2][u][v]mergeplanes=0x001020:yuv420p" \
    -c:v libx264 -crf 0 -pix_fmt yuv420p -y "$TMP/y_only.mp4" 2>/dev/null
echo "✅ Y-only degraded (luma -10%)"

# 3. UV-only 降级
echo ""
echo "📹 Creating UV-only degraded video..."
ffmpeg -i "$TMP/ref.mp4" \
    -vf "extractplanes=y+u+v[y][u][v];[u]lutyuv=u='val*0.7'[u2];[v]lutyuv=v='val*0.7'[v2];[y][u2][v2]mergeplanes=0x001020:yuv420p" \
    -c:v libx264 -crf 0 -pix_fmt yuv420p -y "$TMP/uv_only.mp4" 2>/dev/null
echo "✅ UV-only degraded (chroma -30%)"

# 4. 测试 ffmpeg libvmaf
echo ""
echo "📊 Testing ffmpeg libvmaf MS-SSIM..."
echo ""

echo "Test 1: Y-only degradation"
ffmpeg -i "$TMP/ref.mp4" -i "$TMP/y_only.mp4" \
    -lavfi "[0:v][1:v]libvmaf=feature='name=float_ms_ssim':log_fmt=json:log_path=$TMP/y_result.json" \
    -f null - 2>/dev/null
Y_SCORE=$(python3 -c "import json; print(f\"{json.load(open('$TMP/y_result.json'))['pooled_metrics']['float_ms_ssim']['mean']:.6f}\")")
echo "  MS-SSIM: $Y_SCORE"

echo ""
echo "Test 2: UV-only degradation"
ffmpeg -i "$TMP/ref.mp4" -i "$TMP/uv_only.mp4" \
    -lavfi "[0:v][1:v]libvmaf=feature='name=float_ms_ssim':log_fmt=json:log_path=$TMP/uv_result.json" \
    -f null - 2>/dev/null
UV_SCORE=$(python3 -c "import json; print(f\"{json.load(open('$TMP/uv_result.json'))['pooled_metrics']['float_ms_ssim']['mean']:.6f}\")")
echo "  MS-SSIM: $UV_SCORE"

# 5. 分析
echo ""
echo "═══════════════════════════════════════"
echo "📊 Analysis"
echo "═══════════════════════════════════════"
echo "Y-only degradation:  $Y_SCORE"
echo "UV-only degradation: $UV_SCORE"
echo ""

python3 << EOF
y = float("$Y_SCORE")
uv = float("$UV_SCORE")

print("🔍 Conclusions:")
print("")

if y < 0.999:
    print("✅ Y-channel sensitivity: CONFIRMED")
    print(f"   Luma degradation detected (score: {y:.6f})")
else:
    print("❌ Y-channel sensitivity: NOT DETECTED")

print("")

if uv < 0.999:
    print("✅ UV-channel sensitivity: CONFIRMED")
    print(f"   Chroma degradation detected (score: {uv:.6f})")
    print("")
    print("💡 ffmpeg libvmaf DOES detect chroma changes!")
else:
    print("❌ UV-channel sensitivity: NOT DETECTED")
    print("")
    print("💡 ffmpeg libvmaf is Y-channel only (same as standalone vmaf)")
EOF

rm -rf "$TMP"

echo ""
echo "🧹 Cleanup complete"
