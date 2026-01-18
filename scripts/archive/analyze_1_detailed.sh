#!/bin/bash
SOURCE="/Users/nyamiiko/Downloads/1"
OUTPUT="/Users/nyamiiko/Downloads/1_converted"

echo "🔍 详细分析..."
echo ""

# 1. 检查XMP情况
echo "=== XMP文件分析 ==="
src_xmp=$(find "$SOURCE" -type f -iname "*.xmp" | wc -l | xargs)
out_xmp=$(find "$OUTPUT" -type f -iname "*.xmp" | wc -l | xargs)
echo "源XMP: $src_xmp"
echo "输出XMP: $out_xmp"
echo "已合并: $((src_xmp - out_xmp))"

# 2. 检查媒体文件（排除XMP）
echo ""
echo "=== 媒体文件统计（排除XMP）==="
src_media=$(find "$SOURCE" -type f ! -iname "*.xmp" | wc -l | xargs)
out_media=$(find "$OUTPUT" -type f ! -iname "*.xmp" | wc -l | xargs)
echo "源媒体文件: $src_media"
echo "输出文件: $out_media"
echo "差异: $((src_media - out_media))"

# 3. 检查非媒体文件
echo ""
echo "=== 非媒体文件类型 ==="
find "$SOURCE" -type f ! \( -iname "*.jpg" -o -iname "*.jpeg" -o -iname "*.jpe" -o -iname "*.jfif" -o -iname "*.png" -o -iname "*.webp" -o -iname "*.heic" -o -iname "*.avif" -o -iname "*.mp4" -o -iname "*.mov" -o -iname "*.avi" -o -iname "*.mkv" -o -iname "*.flv" -o -iname "*.wmv" -o -iname "*.m4v" -o -iname "*.xmp" \) -exec basename {} \; | sed 's/.*\.//' | sort | uniq -c | sort -rn

# 4. 查找具体遗漏的媒体文件
echo ""
echo "=== 查找遗漏的媒体文件 ==="
find "$SOURCE" -type f \( -iname "*.jpg" -o -iname "*.jpeg" -o -iname "*.png" -o -iname "*.mp4" -o -iname "*.mov" \) > /tmp/src_media.txt
find "$OUTPUT" -type f \( -iname "*.jxl" -o -iname "*.mp4" -o -iname "*.mov" \) > /tmp/out_media.txt

src_media_count=$(wc -l < /tmp/src_media.txt | xargs)
out_media_count=$(wc -l < /tmp/out_media.txt | xargs)

echo "源媒体文件数: $src_media_count"
echo "输出媒体文件数: $out_media_count"

# 检查是否有文件完全没被处理
echo ""
echo "=== 检查前20个源文件是否存在于输出 ==="
head -20 /tmp/src_media.txt | while read src_file; do
    basename=$(basename "$src_file" | sed 's/\.[^.]*$//')
    if ! grep -q "$basename" /tmp/out_media.txt; then
        echo "❌ 遗漏: $(basename "$src_file")"
    fi
done

rm -f /tmp/src_media.txt /tmp/out_media.txt
