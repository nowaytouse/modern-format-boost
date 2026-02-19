#!/usr/bin/env bash
set -e
cd "$(dirname "$0")"

echo "=== 清理 scripts 目录 ==="

# 删除所有临时测试脚本
find scripts -type f \( \
    -name "test_*.sh" -o -name "*_test*.sh" -o -name "quick_*.sh" -o \
    -name "safe_*.sh" -o -name "verify_*.sh" -o -name "diagnose_*.sh" -o \
    -name "fix_*.sh" -o -name "final_*.sh" -o -name "apply_*.sh" -o \
    -name "emergency_*.sh" -o -name "remediate_*.sh" -o -name "commit_*.sh" -o \
    -name "git_*.sh" -o -name "implement_*.sh" -o -name "complete_*.sh" -o \
    -name "comprehensive_*.sh" -o -name "deep_*.sh" -o -name "design_*.sh" -o \
    -name "direct_*.sh" -o -name "force_*.sh" -o -name "prepare_*.sh" -o \
    -name "rebuild_*.sh" -o -name "remove_*.sh" -o -name "run_*.sh" -o \
    -name "update_*.sh" -o -name "audit_*.sh" -o -name "analyze_*.sh" -o \
    -name "*.py" \
\) -delete

echo "✅ 清理完成"
find scripts -type f | wc -l
