#!/bin/bash
# 🚨 v7.4.1: 最终修复、编译、测试
set -e

cd "$(dirname "$0")/.."

echo "🚨 v7.4.1: Final Fix, Build & Test"
echo ""

# 1. 强制清理并重新编译
echo "1️⃣ Force clean build..."
cargo clean
cargo build --release --manifest-path imgquality_hevc/Cargo.toml

BINARY="target/release/imgquality-hevc"
echo ""
echo "✅ Build complete!"
ls -lh "$BINARY"
echo "   Timestamp: $(date -r $(stat -f "%m" "$BINARY") '+%Y-%m-%d %H:%M:%S')"
echo ""

# 2. 测试目录结构保留
echo "2️⃣ Testing directory structure preservation..."
TEST_ROOT=$(mktemp -d)
TEST_INPUT="$TEST_ROOT/input"
TEST_OUTPUT="$TEST_ROOT/output"

mkdir -p "$TEST_INPUT/photos/2024"
echo "test" > "$TEST_INPUT/photos/2024/test.txt"

echo "   Input:  $TEST_INPUT/photos/2024/test.txt"
echo "   Output: $TEST_OUTPUT"

# 运行测试
./"$BINARY" auto "$TEST_INPUT" --output "$TEST_OUTPUT" --recursive 2>&1 | tail -10

# 检查结果
if [ -f "$TEST_OUTPUT/photos/2024/test.txt" ]; then
    echo ""
    echo "✅ SUCCESS: Directory structure preserved!"
    echo "   Found: $TEST_OUTPUT/photos/2024/test.txt"
else
    echo ""
    echo "❌ FAILED: File not in correct location"
    find "$TEST_OUTPUT" -type f
    rm -rf "$TEST_ROOT"
    exit 1
fi

rm -rf "$TEST_ROOT"
echo ""

# 3. 显示二进制信息
echo "3️⃣ Binary info:"
echo "   Path: $BINARY"
echo "   Size: $(ls -lh "$BINARY" | awk '{print $5}')"
echo "   Time: $(date -r $(stat -f "%m" "$BINARY") '+%Y-%m-%d %H:%M:%S')"
echo ""

echo "✅ All tests passed!"
echo ""
echo "💡 Next steps:"
echo "   1. Use this binary: $BINARY"
echo "   2. Test with real data"
echo "   3. Verify 4h8uh4vkss9clo2wfiy30kach.gif goes to correct subdir"
