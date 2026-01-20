#!/bin/bash
# 测试错误处理器的新功能
set -euo pipefail

cd "$(dirname "$0")/.."

echo "🧪 测试错误处理器模块..."
cargo test -p shared_utils error_handler --lib -- --nocapture

echo ""
echo "✅ 错误处理器测试完成"
