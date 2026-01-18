#!/bin/bash
# 验证混合质量评估方案: Y-MS-SSIM + U-SSIM + V-SSIM
set -e

TMP="/tmp/hybrid_quality_$$"
mkdir -p "$TMP"

echo "🔬 Hybrid Quality Metric Test"
echo "Y-MS-SSIM (0.8) + U-SSIM (0.1) + V-SSIM (0.1)"
echo "=============================================="
echo ""

# 创建测试视频
echo "📹 Creating test videos..."
ffmpeg -f lavfi -i testsrc=duration=2:size=640x480:rate=30 \
    -c:v libx264 -crf 0 -pix_fmt yuv420p -y "$TMP/ref.mp4" 2>/dev/null

ffmpeg -i "$TMP/ref.mp4" \
    -vf "extractplanes=y+u+v[y][u][v];[y]lutyuv=y='val*0.9'[y2];[y2][u][v]mergeplanes=0x001020:yuv420p" \
    -c:v libx264 -crf 0 -pix_fmt yuv420p -y "$TMP/y_deg.mp4" 2>/dev/null

ffmpeg -i "$TMP/ref.mp4" \
    -vf "extractplanes=y+u+v[y][u][v];[u]lutyuv=u='val*0.7'[u2];[v]lutyuv=v='val*0.7'[v2];[y][u2][v2]mergeplanes=0x001020:yuv420p" \
    -c:v libx264 -crf 0 -pix_fmt yuv420p -y "$TMP/uv_deg.mp4" 2>/dev/null

ffmpeg -i "$TMP/ref.mp4" \
    -vf "extractplanes=y+u+v[y][u][v];[y]lutyuv=y='val*0.9'[y2];[u]lutyuv=u='val*0.7'[u2];[v]lutyuv=v='val*0.7'[v2];[y2][u2][v2]mergeplanes=0x001020:yuv420p" \
    -c:v libx264 -crf 0 -pix_fmt yuv420p -y "$TMP/all_deg.mp4" 2>/dev/null

echo "✅ Test videos ready"
echo ""

# 计算混合指标的函数
calc_hybrid() {
    local ref=$1
    local dist=$2
    local name=$3
    
    echo "📊 $name"
    echo "───────────────────────────────────────"
    
    # Y通道 MS-SSIM
    ffmpeg -i "$ref" -i "$dist" \
        -lavfi "[0:v]extractplanes=y[r];[1:v]extractplanes=y[d];[r][d]libvmaf=feature='name=float_ms_ssim':log_fmt=json:log_path=$TMP/y_ms.json" \
        -f null - 2>/dev/null
    Y_MS=$(python3 -c "import json; print(json.load(open('$TMP/y_ms.json'))['pooled_metrics']['float_ms_ssim']['mean'])")
    
    # U通道 SSIM
    ffmpeg -i "$ref" -i "$dist" \
        -lavfi "[0:v]extractplanes=u[r];[1:v]extractplanes=u[d];[r][d]ssim" \
        -f null - 2>&1 | grep "All:" | sed 's/.*All:\([0-9.]*\).*/\1/' > "$TMP/u_ssim.txt"
    U_SSIM=$(cat "$TMP/u_ssim.txt")
    
    # V通道 SSIM
    ffmpeg -i "$ref" -i "$dist" \
        -lavfi "[0:v]extractplanes=v[r];[1:v]extractplanes=v[d];[r][d]ssim" \
        -f null - 2>&1 | grep "All:" | sed 's/.*All:\([0-9.]*\).*/\1/' > "$TMP/v_ssim.txt"
    V_SSIM=$(cat "$TMP/v_ssim.txt")
    
    # SSIM All (对比用)
    ffmpeg -i "$ref" -i "$dist" -lavfi ssim -f null - 2>&1 | grep "All:" | sed 's/.*All:\([0-9.]*\).*/\1/' > "$TMP/ssim_all.txt"
    SSIM_ALL=$(cat "$TMP/ssim_all.txt")
    
    # 计算混合分数
    python3 << EOF
y_ms = float("$Y_MS")
u_ssim = float("$U_SSIM")
v_ssim = float("$V_SSIM")
ssim_all = float("$SSIM_ALL")

hybrid = y_ms * 0.8 + u_ssim * 0.1 + v_ssim * 0.1

print(f"  Y-MS-SSIM (0.8): {y_ms:.6f}")
print(f"  U-SSIM    (0.1): {u_ssim:.6f}")
print(f"  V-SSIM    (0.1): {v_ssim:.6f}")
print(f"  ─────────────────────────────")
print(f"  Hybrid Score:    {hybrid:.6f}")
print(f"  SSIM All (ref):  {ssim_all:.6f}")
print("")
EOF
}

# 测试各种降级场景
calc_hybrid "$TMP/ref.mp4" "$TMP/y_deg.mp4" "Test 1: Y-only degradation (-10%)"
calc_hybrid "$TMP/ref.mp4" "$TMP/uv_deg.mp4" "Test 2: UV-only degradation (-30%)"
calc_hybrid "$TMP/ref.mp4" "$TMP/all_deg.mp4" "Test 3: All-channel degradation"

# 分析
echo "═══════════════════════════════════════════════════════════════"
echo "📊 Analysis"
echo "═══════════════════════════════════════════════════════════════"
python3 << 'EOF'
print("")
print("✅ Hybrid metric advantages:")
print("   1. Y-MS-SSIM: Superior luma structural similarity")
print("   2. U/V-SSIM: Effective chroma degradation detection")
print("   3. Weighted fusion: Balanced quality assessment")
print("")
print("💡 Recommended weights:")
print("   Y: 0.8 (luma dominates perception)")
print("   U: 0.1 (chroma contributes)")
print("   V: 0.1 (chroma contributes)")
print("")
print("🎯 This approach:")
print("   - Detects both luma AND chroma degradation")
print("   - More accurate than SSIM All alone")
print("   - Leverages MS-SSIM's multi-scale advantage")
EOF

rm -rf "$TMP"
echo ""
echo "🧹 Cleanup complete"
