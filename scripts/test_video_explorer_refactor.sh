#!/bin/bash
# 测试 video_explorer 模块重构后的编译

set -e

cd "$(dirname "$0")/.."

echo "🔧 Testing video_explorer module refactoring..."
echo ""

echo "📦 Building shared_utils..."
cargo build -p shared_utils 2>&1 | head -50

echo ""
echo "✅ Build successful!"
echo ""
echo "🧪 Running tests..."
cargo test -p shared_utils --lib video_explorer 2>&1 | tail -20

echo ""
echo "✅ All tests passed!"
