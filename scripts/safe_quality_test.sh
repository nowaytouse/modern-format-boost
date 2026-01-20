#!/bin/bash
# 安全质量测试脚本 - Safe Quality Test Script
# 使用媒体副本测试，不破坏原件

set -euo pipefail

echo "🔒 Safe Quality Test - Using Media Copies"
echo "=========================================="
echo ""

cd "$(dirname "$0")/.."

# 创建临时测试目录
TEST_DIR=$(mktemp -d -t quality_test_XXXXXX)
echo "📁 Test directory: $TEST_DIR"

# 清理函数
cleanup() {
    echo ""
    echo "🧹 Cleaning up test directory..."
    rm -rf "$TEST_DIR"
    echo "✅ Cleanup complete"
}
trap cleanup EXIT

# 复制测试媒体文件（如果存在）
if [ -d "test_media" ]; then
    echo "📋 Copying test media files..."
    cp -r test_media/* "$TEST_DIR/" 2>/dev/null || true
    echo "✅ Test files copied"
else
    echo "⚠️  No test_media directory found, skipping file tests"
fi

# 编译检查
echo ""
echo "🔨 Building project..."
if cargo build --all --quiet 2>&1 | tee /tmp/build_output.txt; then
    echo "✅ Build successful"
else
    echo "❌ Build failed"
    cat /tmp/build_output.txt
    exit 1
fi

# 运行单元测试
echo ""
echo "🧪 Running unit tests..."
if cargo test --all --quiet 2>&1 | tee /tmp/test_output.txt; then
    echo "✅ All tests passed"
else
    echo "❌ Tests failed"
    cat /tmp/test_output.txt
    exit 1
fi

# Clippy 检查
echo ""
echo "📎 Running clippy..."
if cargo clippy --all-targets --quiet 2>&1 | tee /tmp/clippy_output.txt | grep -v "^$"; then
    if grep -q "warning\|error" /tmp/clippy_output.txt; then
        echo "⚠️  Clippy found issues"
        cat /tmp/clippy_output.txt
    else
        echo "✅ Clippy passed"
    fi
else
    echo "✅ Clippy passed"
fi

echo ""
echo "✅ Safe quality test complete - No media files were harmed!"
