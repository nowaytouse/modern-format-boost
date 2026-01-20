#!/bin/bash
# 完整修复所有clippy警告
set -e
cd "$(dirname "$0")/.."

echo "🔧 修复clippy警告 - 分步执行"

# 步骤1: 先编译检查
echo "📦 步骤1: 检查编译..."
cargo build --all-targets 2>&1 | tee /tmp/build_check.log || {
    echo "❌ 编译失败，需要先修复编译错误"
    exit 1
}

# 步骤2: 自动修复
echo "🔨 步骤2: 自动修复..."
cargo clippy --fix --all-targets --allow-dirty --allow-staged 2>&1 | tee /tmp/clippy_fix.log || true

# 步骤3: 检查剩余警告
echo "📊 步骤3: 检查剩余警告..."
cargo clippy --all-targets -- -D warnings 2>&1 | tee /tmp/clippy_final.log

echo "✅ 完成！日志: /tmp/clippy_*.log"
