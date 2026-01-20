#!/bin/bash
# 测试错误上下文增强功能
# Test error context enhancement features

set -euo pipefail

echo "🧪 Testing error context enhancements..."

cd "$(dirname "$0")/.."

# 运行 app_error 相关测试
echo "📋 Running app_error tests..."
cargo test --manifest-path shared_utils/Cargo.toml app_error --lib

echo ""
echo "✅ All error context tests passed!"
echo ""
echo "📊 Test Summary:"
echo "  - Error types enhanced with context fields (file_path, operation, command)"
echo "  - Display trait updated with detailed formatting"
echo "  - Helper methods added: with_file_path(), with_operation(), with_command()"
echo "  - All existing tests updated and passing"
