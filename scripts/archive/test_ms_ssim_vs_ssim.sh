#!/bin/bash
# 🔥 v6.9.6: MS-SSIM vs SSIM 对比测试
# 验证三通道 MS-SSIM 和 SSIM All 的差异

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
TEST_DIR="$PROJECT_DIR/test_media"

echo "═══════════════════════════════════════════════════════════════"
echo "🔬 MS-SSIM vs SSIM Comparison Test (v6.9.6)"
echo "═══════════════════════════════════════════════════════════════"

# 检查测试文件
if [ ! -f "$TEST_DIR/test_short_3s.mp4" ] || [ ! -f "$TEST_DIR/test_short_3s_hevc.mp4" ]; then
    echo "❌ Test files not found. Run conversion first."
    exit 1
fi

INPUT="$TEST_DIR/test_short_3s.mp4"
OUTPUT="$TEST_DIR/test_short_3s_hevc.mp4"

echo ""
echo "📁 Input:  $INPUT"
echo "📁 Output: $OUTPUT"
echo ""

# 1. SSIM (单尺度，Y/U/V/All)
echo "═══════════════════════════════════════════════════════════════"
echo "📊 Test 1: SSIM (Single-Scale, Y/U/V/All)"
echo "═══════════════════════════════════════════════════════════════"
ffmpeg -i "$INPUT" -i "$OUTPUT" -lavfi "[0:v][1:v]ssim" -f null - 2>&1 | grep "SSIM"
echo ""

# 2. MS-SSIM Y 通道 (单通道)
echo "═══════════════════════════════════════════════════════════════"
echo "📊 Test 2: MS-SSIM (Y Channel Only)"
echo "═══════════════════════════════════════════════════════════════"
Y_RESULT=$(ffmpeg -i "$INPUT" -i "$OUTPUT" \
  -lavfi "[0:v][1:v]libvmaf=log_path=/dev/stdout:log_fmt=json:feature='name=float_ms_ssim'" \
  -f null - 2>&1 | grep -A 4 "\"float_ms_ssim\":" | grep "mean" | head -1)
echo "MS-SSIM (Y only): $Y_RESULT"
echo ""

# 3. MS-SSIM 三通道 (Y/U/V)
echo "═══════════════════════════════════════════════════════════════"
echo "📊 Test 3: MS-SSIM (3-Channel: Y/U/V)"
echo "═══════════════════════════════════════════════════════════════"

echo "Y Channel:"
ffmpeg -i "$INPUT" -i "$OUTPUT" \
  -filter_complex "[0:v]format=yuv444p,extractplanes=y[y0];[1:v]format=yuv444p,extractplanes=y[y1];[y0][y1]libvmaf=feature='name=float_ms_ssim':log_fmt=json:log_path=/dev/stdout" \
  -f null - 2>&1 | grep -A 4 "\"float_ms_ssim\":" | grep "mean" | head -1

echo "U Channel:"
ffmpeg -i "$INPUT" -i "$OUTPUT" \
  -filter_complex "[0:v]format=yuv444p,extractplanes=u[u0];[1:v]format=yuv444p,extractplanes=u[u1];[u0][u1]libvmaf=feature='name=float_ms_ssim':log_fmt=json:log_path=/dev/stdout" \
  -f null - 2>&1 | grep -A 4 "\"float_ms_ssim\":" | grep "mean" | head -1

echo "V Channel:"
ffmpeg -i "$INPUT" -i "$OUTPUT" \
  -filter_complex "[0:v]format=yuv444p,extractplanes=v[v0];[1:v]format=yuv444p,extractplanes=v[v1];[v0][v1]libvmaf=feature='name=float_ms_ssim':log_fmt=json:log_path=/dev/stdout" \
  -f null - 2>&1 | grep -A 4 "\"float_ms_ssim\":" | grep "mean" | head -1

echo ""
echo "═══════════════════════════════════════════════════════════════"
echo "📊 Summary"
echo "═══════════════════════════════════════════════════════════════"
echo "SSIM:      Single-scale, fast, includes Y/U/V/All"
echo "MS-SSIM:   Multi-scale, more accurate, better human perception"
echo ""
echo "Key differences:"
echo "  - MS-SSIM (Y only) ignores chroma loss → value too high"
echo "  - MS-SSIM (3-ch) includes chroma → more accurate"
echo "  - SSIM All is weighted average of Y/U/V"
echo "═══════════════════════════════════════════════════════════════"
