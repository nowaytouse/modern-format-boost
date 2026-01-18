#!/bin/bash
# 审计所有文件复制位置，确保都使用 smart_file_copier
set -e
cd "$(dirname "$0")/.."

echo "🔍 Auditing all file copy locations..."
echo ""

# 查找所有可疑的文件复制代码
echo "❌ Problematic patterns (should use smart_file_copier):"
echo ""

grep -rn "out_dir.join(file_name)" --include="*.rs" . 2>/dev/null | grep -v "target/" | grep -v "smart_file_copier" || echo "None found"

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

# 查找所有使用 smart_file_copier 的位置
echo "✅ Using smart_file_copier:"
echo ""

grep -rn "copy_on_skip_or_fail\|smart_copy_with_structure" --include="*.rs" . 2>/dev/null | grep -v "target/" | grep -v "^./shared_utils/src/smart_file_copier.rs" || echo "None found"

echo ""
