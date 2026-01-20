#!/bin/bash
# 测试外部命令日志功能
# Test external command logging utilities

set -euo pipefail

echo "🧪 Testing External Command Logging Utilities"
echo "=============================================="
echo ""

# 测试shared_utils库的logging模块
echo "📦 Running unit tests for logging module..."
cd "$(dirname "$0")/.."
cargo test -p shared_utils --lib logging --quiet

echo ""
echo "✅ All logging tests passed!"
echo ""
echo "📝 New features added:"
echo "  - log_external_tool(): 记录外部工具调用的详细信息"
echo "  - execute_external_command(): 执行外部命令并自动记录日志"
echo "  - execute_external_command_checked(): 执行命令并在失败时返回错误"
echo "  - ExternalCommandResult: 包含exit_code, stdout, stderr, duration"
echo ""
echo "🎯 Requirements validated:"
echo "  - Requirement 2.10: 记录所有外部工具调用"
echo "  - Requirement 16.2: 记录外部进程的启动、运行和退出状态"
echo "  - Requirement 16.3: 记录完整的命令行、标准输出和标准错误"
echo ""
