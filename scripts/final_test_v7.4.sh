#!/bin/bash
# v7.4 最终测试
set -e
cd "$(dirname "$0")/.."

echo "🧪 v7.4 Final Test"
echo ""

BINARY="target/release/imgquality-hevc"

# 1. 检查二进制
echo "1️⃣ Binary check:"
ls -lh "$BINARY"
date -r $(stat -f "%m" "$BINARY") '+   Time: %Y-%m-%d %H:%M:%S'
echo ""

# 2. 测试目录结构保留
echo "2️⃣ Testing directory structure..."
TEST_ROOT=$(mktemp -d)
mkdir -p "$TEST_ROOT/input/photos/2024"
echo "test" > "$TEST_ROOT/input/photos/2024/test.txt"

./"$BINARY" auto "$TEST_ROOT/input" --output "$TEST_ROOT/output" --recursive 2>&1 | tail -5

if [ -f "$TEST_ROOT/output/photos/2024/test.txt" ]; then
    echo "   ✅ Structure preserved"
else
    echo "   ❌ FAILED"
    find "$TEST_ROOT/output" -type f
    rm -rf "$TEST_ROOT"
    exit 1
fi

rm -rf "$TEST_ROOT"
echo ""

echo "✅ All tests passed!"
echo ""
echo "📦 Use: $BINARY"
