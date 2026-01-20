#!/bin/bash
# 快速测试心跳检测

echo "🔴 快速心跳检测测试"
echo ""

ORIGINAL="/Users/nyamiiko/Downloads/all/zz/鬼针草/OC14k60_1.mp4"
TEST_DIR="/tmp/quick_test_$$"
mkdir -p "$TEST_DIR"
COPY="$TEST_DIR/test.mp4"

cp "$ORIGINAL" "$COPY"
echo "✅ 副本: $COPY"
echo ""

BIN="./target/release/vidquality-hevc"

echo "🚀 开始测试 - 观察是否在5分钟后自动终止"
echo "🕐 开始: $(date +'%H:%M:%S')"
echo ""

# 直接运行，不加timeout，看心跳是否工作
"$BIN" auto --explore --match-quality --compress --apple-compat --ultimate "$COPY" 2>&1 | tee "$TEST_DIR/log.txt"

EXIT_CODE=$?
echo ""
echo "🕐 结束: $(date +'%H:%M:%S')"
echo "退出码: $EXIT_CODE"
echo ""
echo "日志: $TEST_DIR/log.txt"
