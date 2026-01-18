#!/bin/bash
# 🚨 紧急修复所有目录结构BUG
# 这些BUG导致文件被复制到根目录而不是保留子目录结构

set -e
cd "$(dirname "$0")/.."

echo "🚨 Emergency Fix: Directory Structure Bugs"
echo ""

# 需要修复的文件列表
FILES=(
    "imgquality_hevc/src/conversion_api.rs:168"
    "imgquality_av1/src/conversion_api.rs:178"
    "vidquality_av1/src/conversion_api.rs:175"
    "vidquality_hevc/src/conversion_api.rs:181,454,522,629"
    "shared_utils/src/cli_runner.rs:143"
)

echo "📋 Files to fix:"
for file in "${FILES[@]}"; do
    echo "   - $file"
done
echo ""

echo "⚠️  This script will show the problematic code."
echo "   Manual fixes required due to context differences."
echo ""

# 显示每个文件的问题代码
for file_info in "${FILES[@]}"; do
    file="${file_info%%:*}"
    lines="${file_info##*:}"
    
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "📄 $file (lines: $lines)"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    
    # 显示问题代码
    IFS=',' read -ra LINE_ARRAY <<< "$lines"
    for line in "${LINE_ARRAY[@]}"; do
        echo ""
        echo "Line $line context:"
        sed -n "$((line-5)),$((line+10))p" "$file" | cat -n
    done
    echo ""
done

echo ""
echo "🔧 Required fix pattern:"
echo ""
cat << 'EOF'
❌ WRONG (loses directory structure):
    let file_name = input.file_name().unwrap_or_default();
    let dest = out_dir.join(file_name);

✅ CORRECT (preserves directory structure):
    let dest = if let Some(ref base_dir) = config.base_dir {
        let rel_path = input.strip_prefix(base_dir).unwrap_or(input);
        let dest_path = out_dir.join(rel_path);
        
        if let Some(parent) = dest_path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        dest_path
    } else {
        let file_name = input.file_name().unwrap_or_default();
        out_dir.join(file_name)
    };
EOF

echo ""
echo "💡 Use this pattern for ALL file copying in fallback scenarios!"
