#!/bin/bash
# 测试错误处理和日志模块
# Test error handling and logging modules

set -euo pipefail

echo "🧪 Testing Error Handling and Logging Modules"
echo "=============================================="
echo ""

cd "$(dirname "$0")/.."

# 测试错误处理模块
echo "📦 1. Testing error_handler module..."
cargo test -p shared_utils --lib error_handler -- --nocapture --test-threads=1

echo ""
echo "📦 2. Testing app_error module..."
cargo test -p shared_utils --lib app_error -- --nocapture --test-threads=1

echo ""
echo "📦 3. Testing logging module..."
cargo test -p shared_utils --lib logging -- --nocapture --test-threads=1

echo ""
echo "✅ All error handling and logging tests passed!"
echo ""
echo "📊 Test Summary:"
echo "  ✓ error_handler: 错误报告和上下文添加"
echo "  ✓ app_error: 错误类型和上下文信息"
echo "  ✓ logging: 日志初始化、轮转和外部命令记录"
echo ""
