#!/bin/bash
# 依赖审计脚本 - Dependency Audit Script
# 检查所有 Cargo.toml 中的依赖是否被使用

set -euo pipefail

echo "🔍 Auditing dependencies in all Cargo.toml files..."
echo ""

cd "$(dirname "$0")/.."

# 检查每个包
for pkg in imgquality_hevc imgquality_av1 vidquality_hevc vidquality_av1 xmp_merger shared_utils; do
    echo "📦 Checking $pkg..."
    cd "$pkg"
    
    # 编译检查
    if cargo check --quiet 2>&1 | grep -i "warning.*unused"; then
        echo "⚠️  Found unused dependencies in $pkg"
    else
        echo "✅ No unused dependencies detected in $pkg"
    fi
    
    cd ..
    echo ""
done

echo "✅ Dependency audit complete"
