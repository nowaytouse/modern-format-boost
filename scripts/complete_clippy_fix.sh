#!/bin/bash
# 完整修复所有clippy警告
set -e
cd "$(dirname "$0")/.."

echo "🔧 完整clippy修复流程"

# 步骤1: 运行自动修复（多次以确保完全修复）
echo "📝 步骤1: 自动修复（第1轮）..."
cargo clippy --fix --all-targets --allow-dirty --allow-staged 2>&1 | tee /tmp/fix1.log || true

echo "📝 步骤2: 自动修复（第2轮）..."
cargo clippy --fix --all-targets --allow-dirty --allow-staged 2>&1 | tee /tmp/fix2.log || true

echo "📝 步骤3: 自动修复（第3轮）..."
cargo clippy --fix --all-targets --allow-dirty --allow-staged 2>&1 | tee /tmp/fix3.log || true

# 步骤2: 检查剩余问题
echo ""
echo "📊 步骤4: 检查剩余警告..."
if cargo clippy --all-targets -- -D warnings 2>&1 | tee /tmp/final_check.log; then
    echo ""
    echo "✅ 所有clippy警告已修复！"
    exit 0
else
    echo ""
    echo "⚠️  剩余警告统计:"
    grep "^error:" /tmp/final_check.log | cut -d: -f2 | sort | uniq -c | sort -rn
    echo ""
    echo "详细日志: /tmp/final_check.log"
    exit 1
fi
