#!/bin/bash
# 测试SSIM All对色度降级的敏感性
set -e

TMP="/tmp/ssim_chroma_$$"
mkdir -p "$TMP"

echo "🔬 SSIM Chroma Detection Test"
echo "=============================="
echo ""

ffmpeg -f lavfi -i testsrc=duration=2:size=640x480:rate=30 \
    -c:v libx264 -crf 0 -pix_fmt yuv420p -y "$TMP/ref.mp4" 2>/dev/null

# 不同程度的色度降级
for pct in 10 20 30 40 50; do
    factor=$(python3 -c "print(1.0 - $pct/100.0)")
    ffmpeg -i "$TMP/ref.mp4" \
        -vf "extractplanes=y+u+v[y][u][v];[u]lutyuv=u='val*$factor'[u2];[v]lutyuv=v='val*$factor'[v2];[y][u2][v2]mergeplanes=0x001020:yuv420p" \
        -c:v libx264 -crf 0 -pix_fmt yuv420p -y "$TMP/uv_${pct}.mp4" 2>/dev/null
    
    SSIM=$(ffmpeg -i "$TMP/ref.mp4" -i "$TMP/uv_${pct}.mp4" -lavfi ssim -f null - 2>&1 | grep "All:" | sed 's/.*All:\([0-9.]*\).*/\1/')
    echo "UV degradation ${pct}%: SSIM All = $SSIM"
done

rm -rf "$TMP"
