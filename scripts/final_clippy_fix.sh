#!/bin/bash
# 最终clippy修复脚本
set -e
cd "$(dirname "$0")/.."

echo "🔧 最终clippy修复..."

# 1. 运行自动修复（允许所有建议的修复）
echo "📝 步骤1: 自动修复..."
cargo clippy --fix --all-targets --allow-dirty --allow-staged 2>&1 | tee /tmp/clippy_auto.log || true

# 2. 再次自动修复（有些需要多次）
echo "📝 步骤2: 再次自动修复..."
cargo clippy --fix --all-targets --allow-dirty --allow-staged 2>&1 | tee /tmp/clippy_auto2.log || true

# 3. 检查剩余问题
echo "📊 步骤3: 检查剩余警告..."
cargo clippy --all-targets -- -D warnings 2>&1 | tee /tmp/clippy_remaining.log || {
    echo ""
    echo "⚠️  剩余警告数量:"
    grep "^error:" /tmp/clippy_remaining.log | wc -l
    echo ""
    echo "主要类型:"
    grep "^error:" /tmp/clippy_remaining.log | cut -d: -f2 | sort | uniq -c | sort -rn | head -10
    exit 1
}

echo "✅ 所有clippy警告已修复！"
