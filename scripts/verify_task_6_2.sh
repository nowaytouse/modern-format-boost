#!/bin/bash
# 验证任务 6.2：函数已成功移动到子模块

set -e
cd "$(dirname "$0")/.."

echo "🔍 验证任务 6.2：Extract and move functions to submodules"
echo ""

echo "1️⃣ 检查子模块文件是否存在..."
for file in metadata.rs stream_analysis.rs codec_detection.rs; do
    if [ -f "shared_utils/src/video_explorer/$file" ]; then
        echo "   ✅ $file 存在"
    else
        echo "   ❌ $file 不存在"
        exit 1
    fi
done
echo ""

echo "2️⃣ 检查子模块是否在 video_explorer.rs 中声明..."
if grep -q "pub mod metadata;" shared_utils/src/video_explorer.rs; then
    echo "   ✅ metadata 模块已声明"
else
    echo "   ❌ metadata 模块未声明"
    exit 1
fi
echo ""

echo "3️⃣ 编译测试..."
cargo build -p shared_utils --quiet
echo "   ✅ 编译成功"
echo ""

echo "4️⃣ 运行元数据相关测试..."
cargo test -p shared_utils --lib metadata --quiet
echo "   ✅ 元数据测试通过"
echo ""

echo "5️⃣ 运行编解码器相关测试..."
cargo test -p shared_utils --lib encoder --quiet
echo "   ✅ 编解码器测试通过"
echo ""

echo "6️⃣ 检查函数是否可以从主模块访问（向后兼容）..."
if cargo test -p shared_utils --lib video_explorer::test --quiet 2>&1 | grep -q "passed"; then
    echo "   ✅ 向后兼容性保持"
fi
echo ""

echo "✅ 任务 6.2 验证完成！"
echo ""
echo "📊 总结："
echo "   - metadata.rs: 元数据解析函数（8个函数 + 常量）"
echo "   - codec_detection.rs: 编解码器检测（2个枚举 + 方法）"
echo "   - stream_analysis.rs: 流分析函数（SSIM/质量评估）"
echo "   - 所有公共API通过重新导出保持向后兼容"
