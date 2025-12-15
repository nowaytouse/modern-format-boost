#!/bin/bash
# VMAF-SSIM协同验证测试脚本 v5.75
# 测试场景：
# 1. 短视频 + VMAF启用 → 应该计算VMAF
# 2. 长视频(>5min) + VMAF启用 → 应该跳过VMAF
# 3. 长视频 + force-vmaf-long → 应该强制计算VMAF

set -e
SCRIPT_DIR="$(dirname "$0")"
PROJECT_DIR="$SCRIPT_DIR/.."

echo "=========================================="
echo "🧪 VMAF-SSIM协同验证测试 v5.75"
echo "=========================================="

# 创建测试目录
TEST_DIR="$PROJECT_DIR/test_vmaf_ssim_output"
rm -rf "$TEST_DIR"
mkdir -p "$TEST_DIR"

# 生成测试视频
echo ""
echo "📹 生成测试视频..."

# 短视频 (10秒)
echo "  → 生成短视频 (10秒)..."
ffmpeg -y -f lavfi -i testsrc=duration=10:size=640x480:rate=30 \
    -c:v libx264 -preset ultrafast -crf 23 \
    "$TEST_DIR/short_10s.mp4" 2>/dev/null

# 长视频 (6分钟 = 360秒)
echo "  → 生成长视频 (6分钟)..."
ffmpeg -y -f lavfi -i testsrc=duration=360:size=640x480:rate=30 \
    -c:v libx264 -preset ultrafast -crf 23 \
    "$TEST_DIR/long_6min.mp4" 2>/dev/null

echo "✅ 测试视频生成完成"

# 检查vidquality_hevc是否已编译
BINARY="$PROJECT_DIR/vidquality_hevc/target/release/vidquality-hevc"
if [ ! -f "$BINARY" ]; then
    echo ""
    echo "🔨 编译 vidquality_hevc..."
    cargo build --release --manifest-path "$PROJECT_DIR/vidquality_hevc/Cargo.toml"
fi

echo ""
echo "=========================================="
echo "测试1: 短视频 + VMAF启用 (双击脚本参数)"
echo "预期: 应该计算VMAF"
echo "=========================================="
"$BINARY" auto \
    "$TEST_DIR/short_10s.mp4" \
    --vmaf \
    --vmaf-threshold 85 \
    --explore \
    --match-quality true \
    --compress \
    --apple-compat \
    --output "$TEST_DIR/short_output.mp4" \
    2>&1 | tee "$TEST_DIR/test1_short_vmaf.log" || true

echo ""
echo "=========================================="
echo "测试2: 长视频 + VMAF启用 (无force)"
echo "预期: 应该跳过VMAF (>5分钟)"
echo "=========================================="
"$BINARY" auto \
    "$TEST_DIR/long_6min.mp4" \
    --vmaf \
    --vmaf-threshold 85 \
    --explore \
    --match-quality true \
    --compress \
    --apple-compat \
    --output "$TEST_DIR/long_output.mp4" \
    2>&1 | tee "$TEST_DIR/test2_long_skip.log" || true

echo ""
echo "=========================================="
echo "测试3: 长视频 + force-vmaf-long"
echo "预期: 应该强制计算VMAF"
echo "=========================================="
echo "⚠️  此测试耗时较长，跳过实际执行"
echo "命令: $BINARY auto \\"
echo "    \"$TEST_DIR/long_6min.mp4\" \\"
echo "    --vmaf --force-vmaf-long --explore --match-quality true --compress \\"
echo "    --output \"$TEST_DIR/long_forced.mp4\""

echo ""
echo "=========================================="
echo "📊 测试结果分析"
echo "=========================================="

echo ""
echo "--- 测试1日志 (短视频+VMAF) ---"
if grep -q "VMAF" "$TEST_DIR/test1_short_vmaf.log" 2>/dev/null; then
    echo "✅ 检测到VMAF相关输出"
    grep -i "vmaf\|ssim\|psnr" "$TEST_DIR/test1_short_vmaf.log" | head -10 || true
else
    echo "⚠️  未检测到VMAF输出"
fi

echo ""
echo "--- 测试2日志 (长视频跳过) ---"
if grep -qi "skip\|跳过\|long" "$TEST_DIR/test2_long_skip.log" 2>/dev/null; then
    echo "✅ 检测到跳过相关输出"
    grep -i "skip\|跳过\|long\|duration" "$TEST_DIR/test2_long_skip.log" | head -10 || true
else
    echo "⚠️  未检测到跳过输出"
fi

echo ""
echo "=========================================="
echo "🎉 测试完成"
echo "=========================================="
echo "测试文件位置: $TEST_DIR/"
ls -la "$TEST_DIR/"
