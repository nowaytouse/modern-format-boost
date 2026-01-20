#!/bin/bash
# 检查死代码的脚本
# Dead code detection script

set -euo pipefail

echo "=========================================="
echo "🔍 检查未使用的代码 (Checking for dead code)"
echo "=========================================="

cd "$(dirname "$0")/.."

echo ""
echo "1️⃣ 运行 cargo clippy 检查未使用的代码..."
echo "Running cargo clippy to find unused code..."
cargo clippy --all-targets --all-features -- -W dead_code -W unused_imports -W unused_variables 2>&1 | tee /tmp/dead_code_clippy.txt

echo ""
echo "2️⃣ 检查未使用的依赖 (需要 cargo-udeps)..."
echo "Checking for unused dependencies (requires cargo-udeps)..."
if command -v cargo-udeps &> /dev/null; then
    cargo +nightly udeps --all-targets 2>&1 | tee /tmp/dead_code_udeps.txt
else
    echo "⚠️  cargo-udeps 未安装，跳过依赖检查"
    echo "   安装命令: cargo install cargo-udeps"
fi

echo ""
echo "3️⃣ 查找注释掉的代码块..."
echo "Finding commented-out code blocks..."
find . -name "*.rs" -type f ! -path "./target/*" ! -path "./.git/*" -exec grep -l "^[[:space:]]*//.*fn\|^[[:space:]]*//.*struct\|^[[:space:]]*//.*impl" {} \; | tee /tmp/dead_code_comments.txt

echo ""
echo "✅ 检查完成！结果已保存到 /tmp/dead_code_*.txt"
echo "Check complete! Results saved to /tmp/dead_code_*.txt"
