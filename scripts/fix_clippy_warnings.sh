#!/bin/bash
# 批量修复clippy警告的脚本
# 避免IDE终端中断问题

set -e

cd "$(dirname "$0")/.."

echo "🔧 开始修复clippy警告..."

# 1. 自动修复可以自动修复的警告
echo "📝 步骤1: 运行 cargo clippy --fix..."
cargo clippy --fix --all-targets --allow-dirty --allow-staged 2>&1 | tee /tmp/clippy_fix.log || true

# 2. 再次检查剩余警告
echo ""
echo "📊 步骤2: 检查剩余警告..."
cargo clippy --all-targets -- -D warnings 2>&1 | tee /tmp/clippy_check.log || {
    echo ""
    echo "⚠️  仍有警告需要手动修复"
    echo "详细日志: /tmp/clippy_check.log"
    exit 1
}

echo ""
echo "✅ 所有clippy警告已修复！"
