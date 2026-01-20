#!/usr/bin/env bash
# 临时脚本：运行向后兼容性测试
set -e
cd "$(dirname "$0")/.."
./scripts/test_backward_compatibility.sh 2>&1 | tee /tmp/compat_test_output.log
echo ""
echo "完整日志: /tmp/compat_test_output.log"
