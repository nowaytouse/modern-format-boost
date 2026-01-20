#!/bin/bash
# 验证死代码移除 - Verify dead code removal
set -euo pipefail

cd "$(dirname "$0")/.."

echo "=========================================="
echo "🧪 验证死代码移除 - Verify Dead Code Removal"
echo "=========================================="
echo ""

# 1. 编译检查
echo "1️⃣ 编译检查..."
if cargo build --all-targets 2>&1 | tee /tmp/build_output.txt; then
    echo "   ✅ 编译成功"
else
    echo "   ❌ 编译失败"
    exit 1
fi

# 2. 运行测试
echo ""
echo "2️⃣ 运行测试..."
if cargo test --all 2>&1 | tee /tmp/test_output.txt; then
    echo "   ✅ 测试通过"
else
    echo "   ❌ 测试失败"
    exit 1
fi

# 3. Clippy检查
echo ""
echo "3️⃣ Clippy检查..."
cargo clippy --all-targets --all-features 2>&1 | tee /tmp/clippy_final.txt
WARNINGS=$(grep -c "warning:" /tmp/clippy_final.txt || echo "0")
echo "   发现 $WARNINGS 个警告"

# 4. 总结
echo ""
echo "=========================================="
echo "✅ 验证完成！"
echo "=========================================="
echo "修改内容："
echo "  - 移除未使用的依赖: ctrlc"
echo "  - 修复clippy警告: manual_range_contains"
echo "  - 添加allow属性消除误报警告"
echo "  - 修复测试中的常量近似值问题"
echo ""
echo "详细日志："
echo "  - /tmp/build_output.txt"
echo "  - /tmp/test_output.txt"
echo "  - /tmp/clippy_final.txt"
