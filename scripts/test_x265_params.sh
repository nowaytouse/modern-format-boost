#!/bin/bash
# 测试 x265-params 参数格式

# 使用实际的 GIF 文件（从日志中看到的）
TEST_INPUT="/Users/nyamiiko/Downloads/1/参考/内容 猎奇/4h8uh4vkss9clo2wfiy30kach.gif"
OUTPUT="/tmp/test_x265_output.mp4"

if [ ! -f "$TEST_INPUT" ]; then
    echo "❌ 测试文件不存在: $TEST_INPUT"
    echo "使用备用测试..."
    # 创建一个有效的测试视频
    ffmpeg -f lavfi -i testsrc=duration=1:size=320x240:rate=10 -pix_fmt yuv420p -y /tmp/test_input.mp4 2>/dev/null
    TEST_INPUT="/tmp/test_input.mp4"
fi

echo "🧪 测试 x265-params 参数格式..."
echo "输入文件: $TEST_INPUT"
echo ""

# 测试 1: 不使用 x265-params
echo "=== 测试 1: 不使用 x265-params ==="
ffmpeg -y -i "$TEST_INPUT" \
    -c:v libx265 \
    -crf 19.9 \
    -preset medium \
    -tag:v hvc1 \
    "$OUTPUT" 2>&1 | grep -E "(error|Error|Invalid|failed|success)" | head -n 5

if [ -f "$OUTPUT" ] && [ -s "$OUTPUT" ]; then
    echo "✅ 测试 1 成功 (文件大小: $(stat -f%z "$OUTPUT") bytes)"
    rm -f "$OUTPUT"
else
    echo "❌ 测试 1 失败"
fi

echo ""

# 测试 2: 使用 x265-params（引号包裹）
echo "=== 测试 2: x265-params 带引号 ==="
ffmpeg -y -i "$TEST_INPUT" \
    -c:v libx265 \
    -crf 19.9 \
    -preset medium \
    -x265-params "log-level=error:pools=2" \
    -tag:v hvc1 \
    "$OUTPUT" 2>&1 | grep -E "(error|Error|Invalid|failed|success)" | head -n 5

if [ -f "$OUTPUT" ] && [ -s "$OUTPUT" ]; then
    echo "✅ 测试 2 成功 (文件大小: $(stat -f%z "$OUTPUT") bytes)"
    rm -f "$OUTPUT"
else
    echo "❌ 测试 2 失败"
fi

echo ""

# 测试 3: 使用 x265-params（无引号）
echo "=== 测试 3: x265-params 无引号 ==="
ffmpeg -y -i "$TEST_INPUT" \
    -c:v libx265 \
    -crf 19.9 \
    -preset medium \
    -x265-params log-level=error:pools=2 \
    -tag:v hvc1 \
    "$OUTPUT" 2>&1 | grep -E "(error|Error|Invalid|failed|success)" | head -n 5

if [ -f "$OUTPUT" ] && [ -s "$OUTPUT" ]; then
    echo "✅ 测试 3 成功 (文件大小: $(stat -f%z "$OUTPUT") bytes)"
    rm -f "$OUTPUT"
else
    echo "❌ 测试 3 失败"
fi

echo ""

# 测试 4: 参数在 CRF 之前
echo "=== 测试 4: x265-params 在 CRF 之前 ==="
ffmpeg -y -i "$TEST_INPUT" \
    -c:v libx265 \
    -preset medium \
    -x265-params "log-level=error:pools=2" \
    -crf 19.9 \
    -tag:v hvc1 \
    "$OUTPUT" 2>&1 | grep -E "(error|Error|Invalid|failed|success)" | head -n 5

if [ -f "$OUTPUT" ] && [ -s "$OUTPUT" ]; then
    echo "✅ 测试 4 成功 (文件大小: $(stat -f%z "$OUTPUT") bytes)"
    rm -f "$OUTPUT"
else
    echo "❌ 测试 4 失败"
fi

# 清理
rm -f "$OUTPUT" /tmp/test_input.mp4
