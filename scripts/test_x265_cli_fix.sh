#!/bin/bash
# 测试 x265 CLI 修复

echo "🧪 Testing x265 CLI Fix"
echo "======================="
echo ""

# 测试文件
TEST_GIF="/Users/nyamiiko/Downloads/1/参考/内容 猎奇/4h8uh4vkss9clo2wfiy30kach.gif"
OUTPUT_DIR="/tmp/x265_cli_test"

# 创建输出目录
mkdir -p "$OUTPUT_DIR"

# 检查 x265 是否可用
if ! command -v x265 &> /dev/null; then
    echo "❌ x265 CLI not found"
    echo "   Install with: brew install x265"
    exit 1
fi

echo "✅ x265 CLI is available"
x265 --version 2>&1 | head -n 1
echo ""

# 测试 imgquality_hevc
echo "📝 Testing imgquality_hevc with problematic GIF..."
echo "   Input: $TEST_GIF"
echo "   Output: $OUTPUT_DIR"
echo ""

# 运行测试
modern_format_boost/imgquality_hevc/target/release/imgquality-hevc auto \
    "$TEST_GIF" \
    --output "$OUTPUT_DIR" \
    --explore \
    --match-quality \
    --compress \
    2>&1 | tee /tmp/test_output.log

# 检查结果
if [ -f "$OUTPUT_DIR/4h8uh4vkss9clo2wfiy30kach.mp4" ]; then
    echo ""
    echo "✅ Test PASSED - Output file created"
    ls -lh "$OUTPUT_DIR/4h8uh4vkss9clo2wfiy30kach.mp4"
else
    echo ""
    echo "❌ Test FAILED - No output file"
    echo "Check log: /tmp/test_output.log"
    exit 1
fi

# 检查是否有错误
if grep -q "CPU calibration encoding failed" /tmp/test_output.log; then
    echo "⚠️  Warning: CPU calibration still failing"
fi

if grep -q "Error splitting the argument list" /tmp/test_output.log; then
    echo "❌ FAILED: Still getting FFmpeg parameter error"
    exit 1
fi

if grep -q "x265 CLI" /tmp/test_output.log; then
    echo "✅ Confirmed: Using x265 CLI for encoding"
fi

echo ""
echo "🎉 All tests passed!"
