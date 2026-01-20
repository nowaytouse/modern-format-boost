#!/bin/bash
# 为无法修复的clippy警告添加allow属性
set -e
cd "$(dirname "$0")/.."

echo "🔧 添加clippy allow属性..."

# 获取所有"too many arguments"的位置
cargo clippy --all-targets -- -D warnings 2>&1 | \
    grep -A 2 "too many arguments" | \
    grep "^  -->" | \
    awk '{print $2}' | \
    sort -u > /tmp/too_many_args.txt

echo "📝 找到需要添加allow的函数:"
cat /tmp/too_many_args.txt

# 对于"too many arguments"，这通常是设计决定，添加allow
# 对于"very complex type"，也添加allow
# 这些是合理的设计选择，不应强制修改

echo ""
echo "✅ 请手动在这些函数上添加 #[allow(clippy::too_many_arguments)]"
echo "或者运行: cargo clippy --fix --allow-dirty --allow-staged"
