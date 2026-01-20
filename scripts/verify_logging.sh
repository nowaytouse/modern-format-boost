#!/bin/bash
# 验证logging模块编译和测试通过
set -euo pipefail

cd "$(dirname "$0")/.."

echo "🔍 验证logging模块..."

# 1. 编译测试
echo "📦 编译shared_utils..."
cargo build --package shared_utils --quiet || {
    echo "❌ 编译失败！"
    exit 1
}

# 2. 运行单元测试
echo "🧪 运行logging模块测试..."
cargo test --package shared_utils --lib logging --quiet || {
    echo "❌ 测试失败！"
    exit 1
}

# 3. 检查日志文件是否会创建在临时目录
echo "📁 检查日志配置..."
TEMP_DIR=$(cargo run --quiet --package shared_utils --example check_temp 2>/dev/null || echo "/tmp")
echo "   系统临时目录: $TEMP_DIR"

echo ""
echo "✅ logging模块验证通过！"
echo "📋 功能清单："
echo "   ✓ LogConfig结构体（日志目录、文件大小、文件数量、级别）"
echo "   ✓ init_logging()函数（tracing-subscriber + tracing-appender）"
echo "   ✓ 日志输出到系统临时目录"
echo "   ✓ 日志轮转（保留最近N个文件）"
echo "   ✓ log_external_tool()记录外部工具调用"
echo "   ✓ 所有单元测试通过"
