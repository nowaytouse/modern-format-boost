#!/bin/bash
# 检查未使用的依赖 - Check unused dependencies
set -euo pipefail

cd "$(dirname "$0")/.."

echo "🔍 检查shared_utils中可能未使用的依赖..."
echo ""

# 检查每个依赖是否在代码中被使用
DEPS=(
    "lazy_static"
    "ctrlc"
    "log"
    "console"
)

for dep in "${DEPS[@]}"; do
    echo -n "检查 $dep ... "
    # 搜索use语句或extern crate
    if grep -r "use $dep" shared_utils/src/ > /dev/null 2>&1 || \
       grep -r "extern crate $dep" shared_utils/src/ > /dev/null 2>&1 || \
       grep -r "$dep::" shared_utils/src/ > /dev/null 2>&1; then
        echo "✅ 使用中"
    else
        echo "⚠️  可能未使用"
    fi
done

echo ""
echo "✅ 检查完成"
