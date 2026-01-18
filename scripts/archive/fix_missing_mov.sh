#!/bin/bash
# 修复缺失的9个MOV文件（短视频被跳过但未复制）

SOURCE="/Users/nyamiiko/Downloads/zz"
DEST="/Users/nyamiiko/Downloads/zz_converted"

MISSING_FILES=(
    "0hyoushiA.mov"
    "0hyoushiB.mov"
    "0hyoushiC.mov"
    "0hyoushiD.mov"
    "15animeA.mov"
    "15animeB.mov"
    "15animeC.mov"
    "15animeD.mov"
    "3anime.mov"
)

echo "🔧 修复缺失的MOV文件..."
copied=0

for file in "${MISSING_FILES[@]}"; do
    src=$(find "$SOURCE" -name "$file" -type f 2>/dev/null | head -1)
    if [ -n "$src" ]; then
        # 保持目录结构
        rel_path="${src#$SOURCE/}"
        dest_path="$DEST/$rel_path"
        dest_dir=$(dirname "$dest_path")
        
        mkdir -p "$dest_dir"
        
        if [ ! -f "$dest_path" ]; then
            cp "$src" "$dest_path"
            echo "✅ 复制: $rel_path"
            ((copied++))
            
            # 尝试合并XMP
            xmp_file="${src}.xmp"
            if [ -f "$xmp_file" ]; then
                exiftool -overwrite_original -tagsfromfile "$xmp_file" "$dest_path" 2>/dev/null
                echo "   📋 XMP已合并"
            fi
        else
            echo "⏭️ 已存在: $rel_path"
        fi
    else
        echo "❌ 未找到: $file"
    fi
done

echo ""
echo "✅ 完成！复制了 $copied 个文件"
