#!/bin/bash
# 批量应用 smart_file_copier 到所有4个工具
set -e
cd "$(dirname "$0")/.."

echo "🔧 Applying smart_file_copier to all 4 tools..."
echo ""

# 审计当前状态
echo "📊 Current status:"
bash scripts/audit_all_copy_locations.sh 2>&1 | grep -E "^./|✅|❌" | head -20

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

# 需要修复的文件
FILES=(
    "vidquality_hevc/src/conversion_api.rs"
    "imgquality_av1/src/conversion_api.rs"
    "vidquality_av1/src/conversion_api.rs"
    "shared_utils/src/cli_runner.rs"
)

echo "📝 Files to fix:"
for file in "${FILES[@]}"; do
    echo "   - $file"
done

echo ""
echo "⚠️  Manual fixes required - patterns vary by context"
echo "   Use: shared_utils::copy_on_skip_or_fail()"
