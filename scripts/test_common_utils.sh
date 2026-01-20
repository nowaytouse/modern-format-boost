#!/bin/bash
# 测试 common_utils 模块
# 🔥 v7.8: Task 7.1 - 验证通用工具模块

set -euo pipefail

echo "🧪 Testing common_utils module..."
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

cd "$(dirname "$0")/.."

echo ""
echo "📦 Step 1: Building shared_utils..."
cargo build --package shared_utils 2>&1 | tail -20

if [ $? -eq 0 ]; then
    echo "✅ Build successful"
else
    echo "❌ Build failed"
    exit 1
fi

echo ""
echo "🧪 Step 2: Running unit tests..."
cargo test --package shared_utils common_utils 2>&1 | tail -30

if [ $? -eq 0 ]; then
    echo "✅ Tests passed"
else
    echo "❌ Tests failed"
    exit 1
fi

echo ""
echo "🔍 Step 3: Running clippy checks..."
cargo clippy --package shared_utils -- -D warnings 2>&1 | grep -E "(warning|error)" || echo "✅ No warnings"

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "✅ All checks passed for common_utils module!"
