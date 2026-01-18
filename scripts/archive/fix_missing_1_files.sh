#!/bin/bash
# 修复 /Users/user/Downloads/1 转换中遗漏的文件

SOURCE="/Users/user/Downloads/1"
OUTPUT="/Users/user/Downloads/1_converted"

echo "🔧 修复遗漏文件..."
echo ""

# 1. 复制所有GIF文件
echo "=== 复制GIF文件 ==="
gif_count=0
while IFS= read -r -d '' gif_file; do
    rel_path="${gif_file#$SOURCE/}"
    out_file="$OUTPUT/$rel_path"
    out_dir=$(dirname "$out_file")
    
    mkdir -p "$out_dir"
    cp -p "$gif_file" "$out_file"
    ((gif_count++))
    echo "✓ $rel_path"
done < <(find "$SOURCE" -type f -iname "*.gif" -print0)
echo "已复制 $gif_count 个GIF文件"

# 2. 复制所有HEIF文件
echo ""
echo "=== 复制HEIF文件 ==="
heif_count=0
while IFS= read -r -d '' heif_file; do
    rel_path="${heif_file#$SOURCE/}"
    out_file="$OUTPUT/$rel_path"
    out_dir=$(dirname "$out_file")
    
    mkdir -p "$out_dir"
    cp -p "$heif_file" "$out_file"
    ((heif_count++))
    echo "✓ $rel_path"
done < <(find "$SOURCE" -type f -iname "*.heif" -print0)
echo "已复制 $heif_count 个HEIF文件"

# 3. 合并XMP到复制的文件
echo ""
echo "=== 合并XMP到复制的文件 ==="
xmp_merged=0
for file in "$OUTPUT"/*.{gif,heif} "$OUTPUT"/**/*.{gif,heif}; do
    [ -f "$file" ] || continue
    
    xmp_file="${file}.xmp"
    if [ -f "$xmp_file" ]; then
        echo "合并XMP: $(basename "$file")"
        exiftool -overwrite_original -tagsfromfile "$xmp_file" -all:all "$file" 2>/dev/null && rm "$xmp_file"
        ((xmp_merged++))
    fi
done
echo "已合并 $xmp_merged 个XMP文件"

echo ""
echo "✅ 修复完成！"
echo "   GIF: $gif_count"
echo "   HEIF: $heif_count"
echo "   XMP合并: $xmp_merged"
