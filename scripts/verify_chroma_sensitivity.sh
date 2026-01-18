#!/bin/bash
# 严格验证 vmaf float_ms_ssim 对色度变化的敏感性
set -e

TMP="/tmp/chroma_test_$$"
mkdir -p "$TMP"

echo "🔬 Rigorous Chroma Sensitivity Test"
echo "===================================="
echo ""

# 1. 创建参考视频
echo "📹 Step 1: Creating reference video..."
ffmpeg -f lavfi -i testsrc=duration=2:size=320x240:rate=30 \
    -c:v libx264 -crf 0 -pix_fmt yuv420p -y "$TMP/ref.mp4" 2>/dev/null
ffmpeg -i "$TMP/ref.mp4" -f yuv4mpegpipe -y "$TMP/ref.y4m" 2>/dev/null
echo "✅ Reference ready"

# 2. 创建只有 Y 通道差异的视频（色度保持不变）
echo ""
echo "📹 Step 2: Creating Y-only degraded video..."
ffmpeg -i "$TMP/ref.mp4" \
    -vf "extractplanes=y+u+v[y][u][v];[y]lutyuv=y='val*0.9'[y2];[y2][u][v]mergeplanes=0x001020:yuv420p" \
    -c:v libx264 -crf 0 -pix_fmt yuv420p -y "$TMP/y_only.mp4" 2>/dev/null
ffmpeg -i "$TMP/y_only.mp4" -f yuv4mpegpipe -y "$TMP/y_only.y4m" 2>/dev/null
echo "✅ Y-only degraded (luma reduced by 10%)"

# 3. 创建只有 U/V 通道差异的视频（亮度保持不变）
echo ""
echo "📹 Step 3: Creating UV-only degraded video..."
ffmpeg -i "$TMP/ref.mp4" \
    -vf "extractplanes=y+u+v[y][u][v];[u]lutyuv=u='val*0.7'[u2];[v]lutyuv=v='val*0.7'[v2];[y][u2][v2]mergeplanes=0x001020:yuv420p" \
    -c:v libx264 -crf 0 -pix_fmt yuv420p -y "$TMP/uv_only.mp4" 2>/dev/null
ffmpeg -i "$TMP/uv_only.mp4" -f yuv4mpegpipe -y "$TMP/uv_only.y4m" 2>/dev/null
echo "✅ UV-only degraded (chroma reduced by 30%)"

# 4. 创建全通道差异的视频
echo ""
echo "📹 Step 4: Creating all-channel degraded video..."
ffmpeg -i "$TMP/ref.mp4" -c:v libx264 -crf 28 -pix_fmt yuv420p \
    -y "$TMP/all.mp4" 2>/dev/null
ffmpeg -i "$TMP/all.mp4" -f yuv4mpegpipe -y "$TMP/all.y4m" 2>/dev/null
echo "✅ All-channel degraded (CRF 28)"

# 5. 运行 vmaf 测试
echo ""
echo "📊 Step 5: Running vmaf tests..."
echo ""

echo "Test 1: Y-only degradation"
vmaf -r "$TMP/ref.y4m" -d "$TMP/y_only.y4m" \
     --feature float_ms_ssim -o "$TMP/y_result.json" --json 2>/dev/null
Y_SCORE=$(python3 -c "import json; print(f\"{json.load(open('$TMP/y_result.json'))['pooled_metrics']['float_ms_ssim']['mean']:.6f}\")")
echo "  MS-SSIM: $Y_SCORE"

echo ""
echo "Test 2: UV-only degradation"
vmaf -r "$TMP/ref.y4m" -d "$TMP/uv_only.y4m" \
     --feature float_ms_ssim -o "$TMP/uv_result.json" --json 2>/dev/null
UV_SCORE=$(python3 -c "import json; print(f\"{json.load(open('$TMP/uv_result.json'))['pooled_metrics']['float_ms_ssim']['mean']:.6f}\")")
echo "  MS-SSIM: $UV_SCORE"

echo ""
echo "Test 3: All-channel degradation"
vmaf -r "$TMP/ref.y4m" -d "$TMP/all.y4m" \
     --feature float_ms_ssim -o "$TMP/all_result.json" --json 2>/dev/null
ALL_SCORE=$(python3 -c "import json; print(f\"{json.load(open('$TMP/all_result.json'))['pooled_metrics']['float_ms_ssim']['mean']:.6f}\")")
echo "  MS-SSIM: $ALL_SCORE"

# 6. 分析结果
echo ""
echo "═══════════════════════════════════════"
echo "📊 Analysis"
echo "═══════════════════════════════════════"
echo "Y-only degradation:  $Y_SCORE"
echo "UV-only degradation: $UV_SCORE"
echo "All-channel degrad:  $ALL_SCORE"
echo ""

python3 << EOF
y = float("$Y_SCORE")
uv = float("$UV_SCORE")
all_ch = float("$ALL_SCORE")

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
else:
    print("❌ UV-channel sensitivity: NOT DETECTED")

print("")

if all_ch < min(y, uv):
    print("✅ Combined degradation: CONFIRMED")
    print(f"   All-channel score lower than individual channels")
else:
    print("⚠️  Combined degradation: UNEXPECTED")

print("")
print("💡 Final Verdict:")
if uv < 0.999:
    print("   vmaf float_ms_ssim DOES include chroma information")
    print("   → Suitable for multi-channel quality verification")
else:
    print("   vmaf float_ms_ssim may be Y-only")
    print("   → Need additional chroma verification")
EOF

# 清理
rm -rf "$TMP"

echo ""
echo "🧹 Cleanup complete"
