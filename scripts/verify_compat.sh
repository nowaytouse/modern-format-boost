#!/usr/bin/env bash
# 向后兼容性验证脚本 - 简化版

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "🔍 向后兼容性验证"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

# 1. 检查二进制
echo "✓ 二进制文件存在"
ls -lh "$PROJECT_ROOT/target/release/imgquality-hevc" "$PROJECT_ROOT/target/release/vidquality-hevc" 2>/dev/null || exit 1
echo ""

# 2. 检查CLI参数
echo "✓ 检查 drag_and_drop_processor.sh 使用的参数："
HELP=$("$PROJECT_ROOT/target/release/imgquality-hevc" auto --help 2>&1)

for flag in "--output" "--recursive" "--in-place" "--explore" "--match-quality" "--compress" "--apple-compat" "--ultimate" "--verbose"; do
    if echo "$HELP" | grep -q -- "$flag"; then
        echo "  ✓ $flag"
    else
        echo "  ✗ $flag 缺失"
        exit 1
    fi
done
echo ""

# 3. 测试基本功能
echo "✓ 测试基本功能："
TEST_DIR="/tmp/compat_$$"
mkdir -p "$TEST_DIR"
trap "rm -rf $TEST_DIR" EXIT

cp "$PROJECT_ROOT/test_media/test_image.png" "$TEST_DIR/"

OUTPUT=$("$PROJECT_ROOT/target/release/imgquality-hevc" auto "$TEST_DIR/test_image.png" --output "$TEST_DIR/out" --verbose 2>&1)

if echo "$OUTPUT" | grep -qE "(Skipped|Converted|Copied|⏭️)"; then
    echo "  ✓ 程序正常运行并输出状态"
else
    echo "  ✗ 输出格式异常"
    echo "$OUTPUT"
    exit 1
fi
echo ""

# 4. 测试 drag_and_drop 参数组合
echo "✓ 测试 drag_and_drop_processor.sh 参数组合："
OUTPUT=$("$PROJECT_ROOT/target/release/imgquality-hevc" auto --explore --match-quality --compress --apple-compat --recursive --ultimate "$TEST_DIR" --output "$TEST_DIR/out2" --verbose 2>&1)

if echo "$OUTPUT" | grep -qE "(Skipped|Converted|Copied|⏭️|🔄)"; then
    echo "  ✓ 参数组合正常工作"
else
    echo "  ✗ 参数组合失败"
    exit 1
fi
echo ""

# 5. 测试错误处理
echo "✓ 测试错误处理："
ERROR_OUTPUT=$("$PROJECT_ROOT/target/release/imgquality-hevc" auto /nonexistent 2>&1 || true)
if echo "$ERROR_OUTPUT" | grep -qE "Error"; then
    echo "  ✓ 正确报告错误"
else
    echo "  ✗ 错误处理异常"
    echo "实际输出: $ERROR_OUTPUT"
    exit 1
fi
echo ""

echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "✅ 向后兼容性验证通过"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
