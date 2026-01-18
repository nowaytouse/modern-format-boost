#!/bin/bash
# 🔥 v7.3.3: 验证并测试最新二进制文件

set -e

PROJECT_ROOT="/Users/user/Downloads/GitHub/modern_format_boost"
cd "$PROJECT_ROOT"

echo "🔍 Verification & Test v7.3.3"
echo "=============================="

# 1. 构建最新版本
echo ""
echo "🔨 Building latest version..."
bash scripts/smart_build.sh

# 2. 验证二进制文件
echo ""
echo "📋 Binary verification:"
BINARY="$PROJECT_ROOT/target/release/imgquality-hevc"

if [ ! -f "$BINARY" ]; then
    echo "❌ Binary not found: $BINARY"
    exit 1
fi

BINARY_TIME=$(stat -f "%m" "$BINARY")
echo "   Path: $BINARY"
echo "   Built: $(date -r $BINARY_TIME '+%Y-%m-%d %H:%M:%S')"
echo "   Size: $(ls -lh "$BINARY" | awk '{print $5}')"

# 3. 测试目录结构保留
echo ""
echo "🧪 Testing directory structure preservation..."

TEST_DIR="/tmp/test_v7.3.3_$$"
rm -rf "$TEST_DIR"
mkdir -p "$TEST_DIR"/{input/photos/2024,output}

# 创建测试文件
echo "test" > "$TEST_DIR/input/photos/2024/test.txt"
convert -size 100x100 xc:blue "$TEST_DIR/input/photos/2024/test.png" 2>/dev/null || {
    echo "⚠️  ImageMagick not available, skipping image test"
}

# 运行转换
echo "   Running conversion..."
"$BINARY" auto \
    "$TEST_DIR/input" \
    --output "$TEST_DIR/output" \
    --recursive \
    --verbose 2>&1 | tail -20

# 验证结果
echo ""
echo "📊 Results:"
if [ -d "$TEST_DIR/output/photos/2024" ]; then
    echo "   ✅ Directory structure preserved"
    ls -la "$TEST_DIR/output/photos/2024/" | head -10
else
    echo "   ❌ Directory structure LOST"
    echo "   Output contents:"
    find "$TEST_DIR/output" -type f
    exit 1
fi

# 清理
rm -rf "$TEST_DIR"

echo ""
echo "✅ All tests passed!"
echo ""
echo "💡 To use this binary:"
echo "   $BINARY auto <input> --output <output> --recursive"
