#!/bin/bash
# 验证 ffmpeg libvmaf 的多通道 MS-SSIM 支持
set -e

TMP="/tmp/ffmpeg_libvmaf_multichannel_$$"
mkdir -p "$TMP"

echo "🔬 FFmpeg libvmaf Multi-Channel MS-SSIM Test"
echo "=============================================="
echo ""

# 1. 创建参考视频 (使用更大分辨率避免U/V通道太小)
echo "📹 Creating reference video..."
ffmpeg -f lavfi -i testsrc=duration=2:size=640x480:rate=30 \
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

# 4. 测试多通道 MS-SSIM
echo ""
echo "═══════════════════════════════════════════════════════════════"
echo "📊 Testing Multi-Channel MS-SSIM with ffmpeg libvmaf"
echo "═══════════════════════════════════════════════════════════════"
echo ""

# 测试 1: Y-only 降级 - 分别计算 Y/U/V 通道
echo "Test 1: Y-only degradation (分通道验证)"
echo "───────────────────────────────────────"

# Y 通道
echo -n "  Y channel: "
ffmpeg -i "$TMP/ref.mp4" -i "$TMP/y_only.mp4" \
    -lavfi "[0:v]extractplanes=y[ref];[1:v]extractplanes=y[dist];[ref][dist]libvmaf=feature='name=float_ms_ssim':log_fmt=json:log_path=$TMP/y_y.json" \
    -f null - 2>/dev/null
Y_Y=$(python3 -c "import json; print(f\"{json.load(open('$TMP/y_y.json'))['pooled_metrics']['float_ms_ssim']['mean']:.6f}\")")
echo "$Y_Y"

# U 通道
echo -n "  U channel: "
ffmpeg -i "$TMP/ref.mp4" -i "$TMP/y_only.mp4" \
    -lavfi "[0:v]extractplanes=u[ref];[1:v]extractplanes=u[dist];[ref][dist]libvmaf=feature='name=float_ms_ssim':log_fmt=json:log_path=$TMP/y_u.json" \
    -f null - 2>/dev/null
Y_U=$(python3 -c "import json; print(f\"{json.load(open('$TMP/y_u.json'))['pooled_metrics']['float_ms_ssim']['mean']:.6f}\")")
echo "$Y_U"

# V 通道
echo -n "  V channel: "
ffmpeg -i "$TMP/ref.mp4" -i "$TMP/y_only.mp4" \
    -lavfi "[0:v]extractplanes=v[ref];[1:v]extractplanes=v[dist];[ref][dist]libvmaf=feature='name=float_ms_ssim':log_fmt=json:log_path=$TMP/y_v.json" \
    -f null - 2>/dev/null
Y_V=$(python3 -c "import json; print(f\"{json.load(open('$TMP/y_v.json'))['pooled_metrics']['float_ms_ssim']['mean']:.6f}\")")
echo "$Y_V"

echo ""
echo "Test 2: UV-only degradation (分通道验证)"
echo "───────────────────────────────────────"

# Y 通道
echo -n "  Y channel: "
ffmpeg -i "$TMP/ref.mp4" -i "$TMP/uv_only.mp4" \
    -lavfi "[0:v]extractplanes=y[ref];[1:v]extractplanes=y[dist];[ref][dist]libvmaf=feature='name=float_ms_ssim':log_fmt=json:log_path=$TMP/uv_y.json" \
    -f null - 2>/dev/null
UV_Y=$(python3 -c "import json; print(f\"{json.load(open('$TMP/uv_y.json'))['pooled_metrics']['float_ms_ssim']['mean']:.6f}\")")
echo "$UV_Y"

# U 通道
echo -n "  U channel: "
ffmpeg -i "$TMP/ref.mp4" -i "$TMP/uv_only.mp4" \
    -lavfi "[0:v]extractplanes=u[ref];[1:v]extractplanes=u[dist];[ref][dist]libvmaf=feature='name=float_ms_ssim':log_fmt=json:log_path=$TMP/uv_u.json" \
    -f null - 2>/dev/null
UV_U=$(python3 -c "import json; print(f\"{json.load(open('$TMP/uv_u.json'))['pooled_metrics']['float_ms_ssim']['mean']:.6f}\")")
echo "$UV_U"

# V 通道
echo -n "  V channel: "
ffmpeg -i "$TMP/ref.mp4" -i "$TMP/uv_only.mp4" \
    -lavfi "[0:v]extractplanes=v[ref];[1:v]extractplanes=v[dist];[ref][dist]libvmaf=feature='name=float_ms_ssim':log_fmt=json:log_path=$TMP/uv_v.json" \
    -f null - 2>/dev/null
UV_V=$(python3 -c "import json; print(f\"{json.load(open('$TMP/uv_v.json'))['pooled_metrics']['float_ms_ssim']['mean']:.6f}\")")
echo "$UV_V"

# 5. 分析结果
echo ""
echo "═══════════════════════════════════════════════════════════════"
echo "📊 Analysis"
echo "═══════════════════════════════════════════════════════════════"
echo ""
echo "Y-only degradation (luma -10%):"
echo "  Y channel: $Y_Y"
echo "  U channel: $Y_U"
echo "  V channel: $Y_V"
echo ""
echo "UV-only degradation (chroma -30%):"
echo "  Y channel: $UV_Y"
echo "  U channel: $UV_U"
echo "  V channel: $UV_V"
echo ""

python3 << EOF
# Y-only 测试
y_y = float("$Y_Y")
y_u = float("$Y_U")
y_v = float("$Y_V")

# UV-only 测试
uv_y = float("$UV_Y")
uv_u = float("$UV_U")
uv_v = float("$UV_V")

print("🔍 Conclusions:")
print("")
print("Test 1: Y-only degradation")
print("───────────────────────────")
if y_y < 0.999:
    print(f"✅ Y channel detected degradation: {y_y:.6f}")
else:
    print(f"❌ Y channel missed degradation: {y_y:.6f}")

if y_u >= 0.999 and y_v >= 0.999:
    print(f"✅ U/V channels unchanged: U={y_u:.6f}, V={y_v:.6f}")
else:
    print(f"⚠️  U/V channels changed unexpectedly: U={y_u:.6f}, V={y_v:.6f}")

print("")
print("Test 2: UV-only degradation")
print("───────────────────────────")
if uv_y >= 0.999:
    print(f"✅ Y channel unchanged: {uv_y:.6f}")
else:
    print(f"⚠️  Y channel changed unexpectedly: {uv_y:.6f}")

if uv_u < 0.999 or uv_v < 0.999:
    print(f"✅ U/V channels detected degradation: U={uv_u:.6f}, V={uv_v:.6f}")
else:
    print(f"❌ U/V channels missed degradation: U={uv_u:.6f}, V={uv_v:.6f}")

print("")
print("═══════════════════════════════════════════════════════════════")
print("💡 Conclusion:")
print("═══════════════════════════════════════════════════════════════")

if y_y < 0.999 and (uv_u < 0.999 or uv_v < 0.999):
    print("✅ ffmpeg libvmaf SUPPORTS multi-channel MS-SSIM!")
    print("   Using extractplanes filter enables per-channel verification")
    print("")
    print("📝 Recommended approach:")
    print("   1. Extract Y/U/V planes separately")
    print("   2. Calculate MS-SSIM for each channel")
    print("   3. Weighted average: Y×0.8 + U×0.1 + V×0.1")
else:
    print("⚠️  Multi-channel detection incomplete")
EOF

rm -rf "$TMP"

echo ""
echo "🧹 Cleanup complete"
