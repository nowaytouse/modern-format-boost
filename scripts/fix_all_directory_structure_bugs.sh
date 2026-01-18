#!/bin/bash
# 🔥 v7.3.1: 批量修复所有目录结构BUG

echo "🔧 Fixing directory structure bugs in all converters..."
echo "======================================================"

FILES=(
    "imgquality_av1/src/conversion_api.rs"
    "vidquality_av1/src/conversion_api.rs"
    "vidquality_hevc/src/conversion_api.rs"
)

for file in "${FILES[@]}"; do
    if [ -f "$file" ]; then
        echo "📝 Processing: $file"
        # 这里需要手动修复，因为每个文件的上下文不同
    else
        echo "⚠️  File not found: $file"
    fi
done

echo ""
echo "✅ Manual fixes required - see list above"
