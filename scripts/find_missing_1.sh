#!/bin/bash
SOURCE="/Users/nyamiiko/Downloads/1"
OUTPUT="/Users/nyamiiko/Downloads/1_converted"

echo "🔍 查找具体遗漏文件..."
echo ""

# 统计各类型文件
echo "=== 源目录文件类型统计 ==="
for ext in gif heif heic webp avif jpg jpeg png mp4 mov; do
    count=$(find "$SOURCE" -type f -iname "*.$ext" 2>/dev/null | wc -l | xargs)
    if [ $count -gt 0 ]; then
        echo ".$ext: $count"
    fi
done

echo ""
echo "=== 检查GIF文件 ==="
gif_count=$(find "$SOURCE" -type f -iname "*.gif" | wc -l | xargs)
echo "源GIF数量: $gif_count"
echo "前10个GIF文件:"
find "$SOURCE" -type f -iname "*.gif" | head -10

echo ""
echo "=== 检查HEIF文件 ==="
heif_count=$(find "$SOURCE" -type f -iname "*.heif" | wc -l | xargs)
echo "源HEIF数量: $heif_count"
if [ $heif_count -gt 0 ]; then
    echo "HEIF文件列表:"
    find "$SOURCE" -type f -iname "*.heif"
fi

echo ""
echo "=== 检查输出目录JXL数量 ==="
jxl_count=$(find "$OUTPUT" -type f -iname "*.jxl" | wc -l | xargs)
echo "JXL文件数: $jxl_count"

echo ""
echo "=== 计算预期输出 ==="
jpg_count=$(find "$SOURCE" -type f \( -iname "*.jpg" -o -iname "*.jpeg" -o -iname "*.jpe" -o -iname "*.jfif" \) | wc -l | xargs)
png_count=$(find "$SOURCE" -type f -iname "*.png" | wc -l | xargs)
webp_count=$(find "$SOURCE" -type f -iname "*.webp" | wc -l | xargs)
heic_count=$(find "$SOURCE" -type f -iname "*.heic" | wc -l | xargs)
avif_count=$(find "$SOURCE" -type f -iname "*.avif" | wc -l | xargs)

echo "应转换为JXL的图像:"
echo "  JPG/JPEG: $jpg_count"
echo "  PNG: $png_count"
echo "  WebP: $webp_count"
echo "  HEIC: $heic_count"
echo "  AVIF: $avif_count"
total_img=$((jpg_count + png_count + webp_count + heic_count + avif_count))
echo "  总计: $total_img"

echo ""
echo "应复制的文件:"
echo "  GIF: $gif_count"
echo "  HEIF: $heif_count"
total_copy=$((gif_count + heif_count))
echo "  总计: $total_copy"

mp4_count=$(find "$SOURCE" -type f -iname "*.mp4" | wc -l | xargs)
mov_count=$(find "$SOURCE" -type f -iname "*.mov" | wc -l | xargs)
echo ""
echo "视频文件:"
echo "  MP4: $mp4_count"
echo "  MOV: $mov_count"

expected=$((total_img + total_copy + mp4_count + mov_count))
echo ""
echo "预期输出总数: $expected"
echo "实际输出: $(find "$OUTPUT" -type f | wc -l | xargs)"
echo "差异: $((expected - $(find "$OUTPUT" -type f | wc -l | xargs)))"
