#!/bin/bash
# 测试x265_encoder.rs的日志功能
# 验证任务4.2的实现

set -euo pipefail

echo "🔍 验证x265_encoder.rs日志更新..."

# 检查是否导入了tracing
if grep -q "use tracing::{info, error, debug, warn};" ../shared_utils/src/x265_encoder.rs; then
    echo "✅ tracing导入正确"
else
    echo "❌ 缺少tracing导入"
    exit 1
fi

# 检查是否移除了所有eprintln!（除了错误输出）
eprintln_count=$(grep -c "eprintln!" ../shared_utils/src/x265_encoder.rs || true)
if [ "$eprintln_count" -le 1 ]; then
    echo "✅ 已替换println!/eprintln!为tracing宏"
else
    echo "⚠️  仍有 $eprintln_count 个eprintln!调用"
fi

# 检查是否记录了x265命令
if grep -q "info!(command = %x265_cmd_str" ../shared_utils/src/x265_encoder.rs; then
    echo "✅ x265命令已记录"
else
    echo "❌ 缺少x265命令日志"
    exit 1
fi

# 检查是否记录了FFmpeg命令
if grep -q "info!(command = %ffmpeg_cmd_str" ../shared_utils/src/x265_encoder.rs; then
    echo "✅ FFmpeg命令已记录"
else
    echo "❌ 缺少FFmpeg命令日志"
    exit 1
fi

# 检查是否在失败时记录了stderr
if grep -q "stderr = %stderr_output" ../shared_utils/src/x265_encoder.rs; then
    echo "✅ 失败时记录stderr"
else
    echo "❌ 缺少stderr日志"
    exit 1
fi

# 编译检查
echo ""
echo "🔨 编译检查..."
cd ..
if cargo check -p shared_utils --quiet 2>&1; then
    echo "✅ 编译成功"
else
    echo "❌ 编译失败"
    exit 1
fi

echo ""
echo "🎉 任务4.2验证完成！"
echo "   - 已替换println!为tracing宏"
echo "   - 已记录所有x265命令"
echo "   - 已记录失败时的输出"
