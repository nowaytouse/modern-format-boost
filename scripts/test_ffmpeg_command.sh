#!/bin/bash
# 测试 FFmpeg 命令参数

# 创建一个测试 GIF
TEST_GIF="/tmp/test.gif"
OUTPUT="/tmp/test_output.mp4"

# 创建一个简单的测试 GIF（如果不存在）
if [ ! -f "$TEST_GIF" ]; then
    ffmpeg -f lavfi -i testsrc=duration=1:size=100x100:rate=1 -y "$TEST_GIF" 2>/dev/null
fi

echo "🧪 测试 FFmpeg 命令..."
echo ""

# 测试命令 1: 正常的 x265-params 格式
echo "测试 1: 标准格式"
ffmpeg -y -i "$TEST_GIF" \
    -c:v libx265 \
    -crf 19.9 \
    -preset medium \
    -x265-params "log-level=error:pools=2" \
    -tag:v hvc1 \
    "$OUTPUT" 2>&1 | tee /tmp/test1.log | tail -n 5

if [ -f "$OUTPUT" ] && [ -s "$OUTPUT" ]; then
    echo "✅ 测试 1 成功 (文件大小: $(stat -f%z "$OUTPUT") bytes)"
    rm -f "$OUTPUT"
else
    echo "❌ 测试 1 失败 - 输出文件不存在或为空"
    echo "完整日志:"
    cat /tmp/test1.log | grep -i error
fi

echo ""

# 测试命令 2: 参数顺序不同
echo "测试 2: CRF 在 preset 之后"
ffmpeg -y -i "$TEST_GIF" \
    -c:v libx265 \
    -preset medium \
    -crf 19.9 \
    -x265-params "log-level=error:pools=2" \
    -tag:v hvc1 \
    "$OUTPUT" 2>&1 | tee /tmp/test2.log | tail -n 5

if [ -f "$OUTPUT" ] && [ -s "$OUTPUT" ]; then
    echo "✅ 测试 2 成功 (文件大小: $(stat -f%z "$OUTPUT") bytes)"
    rm -f "$OUTPUT"
else
    echo "❌ 测试 2 失败 - 输出文件不存在或为空"
    echo "完整日志:"
    cat /tmp/test2.log | grep -i error
fi

# 清理
rm -f "$TEST_GIF" "$OUTPUT"
