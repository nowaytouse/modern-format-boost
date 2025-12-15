#!/bin/bash
# 长视频CPU步进测试 - 验证大小约束
# 测试场景：6分钟视频，观察CPU步进过程中的大小变化

set -e
SCRIPT_DIR="$(dirname "$0")"
PROJECT_DIR="$SCRIPT_DIR/.."

echo "=========================================="
echo "🧪 长视频CPU步进测试 v5.87"
echo "=========================================="

# 创建测试目录
TEST_DIR="$PROJECT_DIR/test_long_video_cpu"
rm -rf "$TEST_DIR"
mkdir -p "$TEST_DIR"

# 生成长视频 (6分钟 = 360秒)
echo ""
echo "📹 生成长视频 (6分钟)..."
ffmpeg -y -f lavfi -i testsrc=duration=360:size=640x480:rate=30 \
    -c:v libx264 -preset ultrafast -crf 23 \
    "$TEST_DIR/long_6min.mp4" 2>/dev/null

INPUT_SIZE=$(stat -f%z "$TEST_DIR/long_6min.mp4")
echo "✅ 输入文件: $(numfmt --to=iec-i --suffix=B $INPUT_SIZE 2>/dev/null || echo $INPUT_SIZE bytes)"

# 检查vidquality_hevc是否已编译
BINARY="$PROJECT_DIR/vidquality_hevc/target/release/vidquality-hevc"
if [ ! -f "$BINARY" ]; then
    echo ""
    echo "🔨 编译 vidquality_hevc..."
    cargo build --release --manifest-path "$PROJECT_DIR/vidquality_hevc/Cargo.toml"
fi

echo ""
echo "=========================================="
echo "🧪 长视频 + VMAF启用 (无force)"
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
    2>&1 | tee "$TEST_DIR/test_long_cpu_stepping.log"

echo ""
echo "=========================================="
echo "📊 测试结果分析"
echo "=========================================="

OUTPUT_FILE=$(find "$TEST_DIR/long_output.mp4" -name "*.mp4" -type f 2>/dev/null | head -1)
if [ -f "$OUTPUT_FILE" ]; then
    OUTPUT_SIZE=$(stat -f%z "$OUTPUT_FILE")
    RATIO=$(echo "scale=2; $OUTPUT_SIZE * 100 / $INPUT_SIZE" | bc)
    echo "✅ 输出文件: $(numfmt --to=iec-i --suffix=B $OUTPUT_SIZE 2>/dev/null || echo $OUTPUT_SIZE bytes)"
    echo "📊 压缩率: $RATIO%"
    
    if [ "$OUTPUT_SIZE" -lt "$INPUT_SIZE" ]; then
        echo "✅ 大小约束满足: output < input"
    else
        echo "❌ 大小约束违反: output >= input"
    fi
else
    echo "⚠️  未找到输出文件"
fi

echo ""
echo "=========================================="
echo "📋 CPU步进过程检查"
echo "=========================================="

if grep -q "Phase 2" "$TEST_DIR/test_long_cpu_stepping.log"; then
    echo "✅ 检测到CPU步进 Phase 2"
    
    # 统计所有CRF测试
    CRF_COUNT=$(grep -c "CRF.*:" "$TEST_DIR/test_long_cpu_stepping.log" || echo 0)
    echo "📊 CRF测试次数: $CRF_COUNT"
    
    # 检查是否有OVERSHOOT
    if grep -q "OVERSHOOT" "$TEST_DIR/test_long_cpu_stepping.log"; then
        echo "⚠️  检测到OVERSHOOT (文件超过输入大小)"
        OVERSHOOT_COUNT=$(grep -c "OVERSHOOT" "$TEST_DIR/test_long_cpu_stepping.log" || echo 0)
        echo "   OVERSHOOT次数: $OVERSHOOT_COUNT"
    else
        echo "✅ 无OVERSHOOT - 所有迭代都满足压缩约束"
    fi
    
    # 检查最终结果
    if grep -q "Guarantee: output < input = ✅ YES" "$TEST_DIR/test_long_cpu_stepping.log"; then
        echo "✅ 最终保证: output < input = YES"
    elif grep -q "Guarantee: output < input = ❌ NO" "$TEST_DIR/test_long_cpu_stepping.log"; then
        echo "❌ 最终保证: output < input = NO"
    fi
else
    echo "⚠️  未检测到CPU步进过程"
fi

echo ""
echo "=========================================="
echo "🎉 测试完成"
echo "=========================================="
echo "日志文件: $TEST_DIR/test_long_cpu_stepping.log"
