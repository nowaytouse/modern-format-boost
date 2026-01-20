#!/bin/bash
# 测试 video_explorer 子模块结构创建

set -euo pipefail

echo "🔍 测试 video_explorer 子模块结构..."

cd "$(dirname "$0")/.."

# 检查目录结构
echo "✅ 检查目录结构..."
if [ ! -d "shared_utils/src/video_explorer" ]; then
    echo "❌ 目录不存在: shared_utils/src/video_explorer"
    exit 1
fi

# 检查必需文件
echo "✅ 检查必需文件..."
required_files=(
    "shared_utils/src/video_explorer/mod.rs"
    "shared_utils/src/video_explorer/metadata.rs"
    "shared_utils/src/video_explorer/stream_analysis.rs"
    "shared_utils/src/video_explorer/codec_detection.rs"
)

for file in "${required_files[@]}"; do
    if [ ! -f "$file" ]; then
        echo "❌ 文件不存在: $file"
        exit 1
    fi
    echo "  ✓ $file"
done

# 尝试编译 shared_utils
echo "✅ 编译测试..."
cd shared_utils
if cargo check 2>&1 | tee /tmp/video_explorer_check.log; then
    echo "✅ 编译成功！"
else
    echo "❌ 编译失败，查看日志："
    cat /tmp/video_explorer_check.log
    exit 1
fi

echo ""
echo "🎉 任务 6.1 完成！video_explorer 子模块结构创建成功！"
echo ""
echo "📁 创建的文件："
echo "  - shared_utils/src/video_explorer/mod.rs (公共 API)"
echo "  - shared_utils/src/video_explorer/metadata.rs (元数据解析)"
echo "  - shared_utils/src/video_explorer/stream_analysis.rs (流分析)"
echo "  - shared_utils/src/video_explorer/codec_detection.rs (编解码器检测)"
echo ""
echo "⚠️  注意：这些文件目前是空的，实际函数迁移将在任务 6.2 中完成"
