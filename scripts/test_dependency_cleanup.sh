#!/bin/bash
# 测试依赖清理后的编译
set -euo pipefail

cd "$(dirname "$0")/.."

echo "🧹 测试依赖清理..."
echo ""

# 清理旧的构建
echo "📦 清理旧构建..."
cargo clean

# 检查编译
echo ""
echo "🔨 检查所有包编译..."
cargo check --all --all-targets 2>&1 | tee dependency_cleanup_test.log

if [ $? -eq 0 ]; then
    echo ""
    echo "✅ 所有包编译成功"
    
    # 运行测试
    echo ""
    echo "🧪 运行测试..."
    cargo test --all 2>&1 | tee -a dependency_cleanup_test.log
    
    if [ $? -eq 0 ]; then
        echo ""
        echo "✅ 所有测试通过"
        echo "✅ 依赖清理成功，项目正常工作"
    else
        echo ""
        echo "❌ 测试失败"
        exit 1
    fi
else
    echo ""
    echo "❌ 编译失败"
    exit 1
fi
