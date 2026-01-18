#!/bin/bash
# 测试目录结构保留功能
set -e

cd "$(dirname "$0")/.."

echo "🧪 Testing Directory Structure Preservation"
echo ""

# 等待编译完成
while ps aux | grep -q "[c]argo build.*imgquality_hevc"; do
    echo "⏳ Waiting for build to complete..."
    sleep 2
done

BINARY="target/release/imgquality-hevc"

if [ ! -f "$BINARY" ]; then
    echo "❌ Binary not found: $BINARY"
    exit 1
fi

echo "✅ Binary ready: $BINARY"
echo "   Timestamp: $(date -r $(stat -f "%m" "$BINARY") '+%Y-%m-%d %H:%M:%S')"
echo ""

# 创建测试环境
TEST_ROOT=$(mktemp -d)
TEST_INPUT="$TEST_ROOT/input"
TEST_OUTPUT="$TEST_ROOT/output"

mkdir -p "$TEST_INPUT/subdir1/subdir2"
mkdir -p "$TEST_OUTPUT"

# 创建测试图片（1x1 PNG）
echo "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==" | base64 -d > "$TEST_INPUT/subdir1/subdir2/test.png"

echo "📁 Test structure:"
echo "   Input:  $TEST_INPUT/subdir1/subdir2/test.png"
echo "   Output: $TEST_OUTPUT"
echo ""

# 运行测试
echo "🚀 Running conversion..."
./"$BINARY" auto "$TEST_INPUT" --output "$TEST_OUTPUT" --recursive --verbose 2>&1 | tail -20

echo ""
echo "🔍 Checking results..."

# 检查输出文件位置
if [ -f "$TEST_OUTPUT/subdir1/subdir2/test.png" ] || [ -f "$TEST_OUTPUT/subdir1/subdir2/test.heic" ]; then
    echo "✅ SUCCESS: Directory structure preserved!"
    echo "   Found: $(find "$TEST_OUTPUT" -type f -name "test.*")"
elif [ -f "$TEST_OUTPUT/test.png" ] || [ -f "$TEST_OUTPUT/test.heic" ]; then
    echo "❌ FAILED: File in root directory (structure NOT preserved)"
    echo "   Found: $TEST_OUTPUT/test.*"
    echo ""
    echo "📂 Output structure:"
    find "$TEST_OUTPUT" -type f
    exit 1
else
    echo "⚠️  No output file found"
    echo "📂 Output structure:"
    find "$TEST_OUTPUT" -type f
    exit 1
fi

# 清理
rm -rf "$TEST_ROOT"

echo ""
echo "✅ Test passed!"
