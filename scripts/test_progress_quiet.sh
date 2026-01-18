#!/usr/bin/env bash
# 测试进度条 quiet_mode 功能

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

echo "🧪 测试进度条 quiet_mode 功能"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

# 创建测试目录
TEST_DIR="/tmp/progress_test_$$"
mkdir -p "$TEST_DIR"

# 复制一些测试文件
echo "📁 准备测试文件..."
cp "$PROJECT_ROOT/README.md" "$TEST_DIR/test1.md" 2>/dev/null || echo "test" > "$TEST_DIR/test1.md"
cp "$PROJECT_ROOT/README.md" "$TEST_DIR/test2.md" 2>/dev/null || echo "test" > "$TEST_DIR/test2.md"
cp "$PROJECT_ROOT/README.md" "$TEST_DIR/test3.md" 2>/dev/null || echo "test" > "$TEST_DIR/test3.md"

echo "✅ 测试文件准备完成"
echo ""
echo "🔍 运行并行处理测试（应该只显示一个总进度条）..."
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

# 运行 imgquality-hevc 处理测试文件
"$PROJECT_ROOT/imgquality_hevc/target/release/imgquality-hevc" \
    --input "$TEST_DIR" \
    --output "$TEST_DIR/output" \
    --adjacent \
    --threads 3 \
    2>&1 | head -50

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "✅ 测试完成"
echo ""
echo "📊 检查输出："
echo "   - 应该只看到一个总进度条"
echo "   - 不应该看到多个子进度条混乱输出"
echo ""

# 清理
rm -rf "$TEST_DIR"
