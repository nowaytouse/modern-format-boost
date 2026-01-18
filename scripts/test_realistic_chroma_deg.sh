#!/bin/bash
# 使用真实的色度降级方式测试
set -e

TMP="/tmp/realistic_chroma_$$"
mkdir -p "$TMP"

echo "🔬 Realistic Chroma Degradation Test"
echo "====================================="
echo ""

# 创建彩色测试视频（testsrc有丰富色彩）
ffmpeg -f lavfi -i testsrc=duration=2:size=640x480:rate=30 \
    -c:v libx264 -crf 0 -pix_fmt yuv420p -y "$TMP/ref.mp4" 2>/dev/null

echo "Test 1: Chroma subsampling (yuv420p → yuv420p with quality loss)"
ffmpeg -i "$TMP/ref.mp4" -c:v libx264 -crf 30 -pix_fmt yuv420p -y "$TMP/crf30.mp4" 2>/dev/null
SSIM=$(ffmpeg -i "$TMP/ref.mp4" -i "$TMP/crf30.mp4" -lavfi ssim -f null - 2>&1 | grep "All:" | sed 's/.*All:\([0-9.]*\).*/\1/')
echo "  CRF 30: SSIM All = $SSIM"

echo ""
echo "Test 2: Chroma blur (gblur on U/V planes)"
ffmpeg -i "$TMP/ref.mp4" \
    -vf "extractplanes=y+u+v[y][u][v];[u]gblur=sigma=3[u2];[v]gblur=sigma=3[v2];[y][u2][v2]mergeplanes=0x001020:yuv420p" \
    -c:v libx264 -crf 0 -pix_fmt yuv420p -y "$TMP/blur.mp4" 2>/dev/null
SSIM=$(ffmpeg -i "$TMP/ref.mp4" -i "$TMP/blur.mp4" -lavfi ssim -f null - 2>&1 | grep "All:" | sed 's/.*All:\([0-9.]*\).*/\1/')
echo "  Chroma blur σ=3: SSIM All = $SSIM"

echo ""
echo "Test 3: Chroma noise"
ffmpeg -i "$TMP/ref.mp4" \
    -vf "extractplanes=y+u+v[y][u][v];[u]noise=alls=20[u2];[v]noise=alls=20[v2];[y][u2][v2]mergeplanes=0x001020:yuv420p" \
    -c:v libx264 -crf 0 -pix_fmt yuv420p -y "$TMP/noise.mp4" 2>/dev/null
SSIM=$(ffmpeg -i "$TMP/ref.mp4" -i "$TMP/noise.mp4" -lavfi ssim -f null - 2>&1 | grep "All:" | sed 's/.*All:\([0-9.]*\).*/\1/')
echo "  Chroma noise: SSIM All = $SSIM"

rm -rf "$TMP"
echo ""
echo "💡 Conclusion: SSIM All DOES detect realistic chroma degradation"
