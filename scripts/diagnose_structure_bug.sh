#!/bin/bash
# 诊断文件夹结构BUG
# 检查二进制文件和代码是否匹配

set -e

echo "🔍 Diagnosing directory structure bug..."
echo ""

# 1. 检查二进制文件时间戳
BINARY="target/release/imgquality-hevc"
if [ -f "$BINARY" ]; then
    echo "📦 Binary info:"
    ls -lh "$BINARY"
    echo "   Timestamp: $(date -r $(stat -f "%m" "$BINARY") '+%Y-%m-%d %H:%M:%S')"
else
    echo "❌ Binary not found: $BINARY"
    exit 1
fi
echo ""

# 2. 检查代码中是否包含 base_dir 逻辑
echo "🔍 Checking code for base_dir logic..."
if grep -q "let rel_path = input.strip_prefix(base)" imgquality_hevc/src/lossless_converter.rs; then
    echo "   ✅ lossless_converter.rs has base_dir logic"
else
    echo "   ❌ lossless_converter.rs missing base_dir logic"
fi

if grep -q "let rel_path = input.strip_prefix(base)" imgquality_hevc/src/main.rs; then
    echo "   ✅ main.rs has base_dir logic"
else
    echo "   ❌ main.rs missing base_dir logic"
fi
echo ""

# 3. 提取二进制中的字符串检查
echo "🔍 Checking binary strings..."
if strings "$BINARY" | grep -q "strip_prefix"; then
    echo "   ✅ Binary contains 'strip_prefix' (likely has fix)"
else
    echo "   ⚠️  Binary may not contain directory structure fix"
fi
echo ""

# 4. 重新编译并比较
echo "🔨 Rebuilding to ensure latest code..."
cargo build --release --manifest-path imgquality_hevc/Cargo.toml 2>&1 | tail -5
echo ""

NEW_TIMESTAMP=$(stat -f "%m" "$BINARY")
echo "📦 New binary timestamp: $(date -r $NEW_TIMESTAMP '+%Y-%m-%d %H:%M:%S')"
echo ""

# 5. 测试用例
echo "🧪 Creating test case..."
TEST_DIR=$(mktemp -d)
mkdir -p "$TEST_DIR/input/subdir"
mkdir -p "$TEST_DIR/output"

# 创建测试文件
echo "Test" > "$TEST_DIR/input/subdir/test.txt"

echo "   Input: $TEST_DIR/input/subdir/test.txt"
echo "   Output dir: $TEST_DIR/output"
echo ""

# 6. 运行测试（使用 --help 先验证二进制可用）
echo "🚀 Testing binary..."
if ./"$BINARY" --version 2>/dev/null; then
    echo "   ✅ Binary is executable"
else
    echo "   ❌ Binary execution failed"
fi
echo ""

echo "✅ Diagnosis complete!"
echo ""
echo "💡 Next steps:"
echo "   1. Check if binary timestamp changed after rebuild"
echo "   2. If not changed, code was already compiled"
echo "   3. Test with actual file to verify structure preservation"
