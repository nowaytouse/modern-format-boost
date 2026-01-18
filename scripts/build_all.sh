#!/bin/bash
# 🔥 v5.70: 强制编译所有工具 (向后兼容)
# 
# 这个脚本现在是 smart_build.sh --force 的别名
# 保留此脚本以确保向后兼容性

cd "$(dirname "$0")"

echo "🔧 build_all.sh: Forwarding to smart_build.sh --force"
echo ""

exec ./smart_build.sh --force "$@"
