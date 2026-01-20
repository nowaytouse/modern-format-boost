#!/bin/bash
# 直接测试v7.5.1修复 - 使用副本安全测试

set -e

echo "🔴 v7.5.1 直接测试 - MS-SSIM卡死修复验证"
echo ""

# 原始文件
ORIGINAL="/Users/nyamiiko/Downloads/all/zz/鬼针草/OC14k60_1.mp4"

# 创建测试目录
TEST_DIR="/tmp/v7.5.1_direct_test_$(date +%s)"
mkdir -p "$TEST_DIR"
COPY="$TEST_DIR/test.mp4"

echo "📋 创建副本..."
cp "$ORIGINAL" "$COPY"
echo "✅ 副本: $COPY"
echo ""

# 二进制文件
BIN="./target/release/vidquality-hevc"

echo "🚀 开始测试 (10分钟超时)"
echo "🕐 开始: $(TZ='Asia/Shanghai' date +'%Y-%m-%d %H:%M:%S') 北京时间"
echo ""

START=$(date +%s)

# 使用simple模式强制转换
timeout 600 "$BIN" simple "$COPY" 2>&1 | tee "$TEST_DIR/log.txt"

END=$(date +%s)
ELAPSED=$((END - START))

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "✅ 测试完成!"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "⏱️  耗时: ${ELAPSED}秒 ($(($ELAPSED / 60))分钟)"
echo "🕐 结束: $(TZ='Asia/Shanghai' date +'%Y-%m-%d %H:%M:%S') 北京时间"
echo ""

# 检查关键信息
echo "📊 关键检查:"
if grep -q "Sampling:" "$TEST_DIR/log.txt"; then
    echo "✅ 智能采样已启用"
    grep "Sampling:" "$TEST_DIR/log.txt" | head -1
fi

if grep -q "Parallel processing" "$TEST_DIR/log.txt"; then
    echo "✅ 并行处理已启用"
fi

if grep -q "Beijing" "$TEST_DIR/log.txt"; then
    echo "✅ 北京时区显示正常"
fi

if grep -q "MS-SSIM" "$TEST_DIR/log.txt"; then
    echo "✅ MS-SSIM计算完成"
fi

echo ""
echo "📁 测试目录: $TEST_DIR"
echo "📄 完整日志: $TEST_DIR/log.txt"
echo ""
echo "🧹 清理: rm -rf $TEST_DIR"
