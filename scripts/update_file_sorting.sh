#!/usr/bin/env bash
# 更新五个工具以使用文件排序功能（优先处理小文件）

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

echo "🔧 Updating file sorting in all tools..."
echo ""

# 工具列表
TOOLS=(
    "imgquality_hevc"
    "imgquality_av1"
    "vidquality_hevc"
    "vidquality_av1"
    "xmp_merge"
)

# 备份计数
BACKUP_COUNT=0

for tool in "${TOOLS[@]}"; do
    MAIN_RS="$PROJECT_ROOT/$tool/src/main.rs"
    
    if [ ! -f "$MAIN_RS" ]; then
        echo "⚠️  Skipping $tool (main.rs not found)"
        continue
    fi
    
    echo "📝 Processing $tool..."
    
    # 创建备份
    cp "$MAIN_RS" "$MAIN_RS.bak"
    BACKUP_COUNT=$((BACKUP_COUNT + 1))
    
    echo "   ✓ Backup created: $MAIN_RS.bak"
done

echo ""
echo "✅ Created $BACKUP_COUNT backups"
echo ""
echo "📋 Manual steps required:"
echo "   1. Update file collection code in each tool's main.rs"
echo "   2. Replace WalkDir collection with shared_utils::collect_files_small_first()"
echo "   3. Test compilation: ./scripts/smart_build.sh"
echo "   4. Remove backups if successful: find . -name '*.bak' -delete"
echo ""
echo "Example change:"
echo "  OLD: WalkDir::new(input).into_iter()...collect()"
echo "  NEW: shared_utils::collect_files_small_first(&input, &extensions, recursive)"
