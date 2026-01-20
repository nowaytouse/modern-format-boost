#!/bin/bash
# 测试 ffmpeg_process.rs 的日志功能
# 验证：Requirements 2.10, 16.3

set -euo pipefail

echo "🔍 测试 FFmpeg 进程日志功能..."

cd "$(dirname "$0")/.."

# 编译测试
echo "📦 编译 shared_utils..."
cargo build --package shared_utils --quiet

# 运行单元测试
echo "🧪 运行单元测试..."
cargo test --package shared_utils --lib ffmpeg_process --quiet

# 运行属性测试
echo "🎲 运行属性测试..."
cargo test --package shared_utils --lib ffmpeg_process::prop_tests --quiet

echo "✅ 所有测试通过！"
echo ""
echo "📝 日志功能已集成："
echo "   - FFmpeg命令执行前记录完整命令行"
echo "   - 成功时记录退出码和输出长度"
echo "   - 失败时记录完整的stderr和stdout"
echo "   - 使用tracing框架替代println!"
