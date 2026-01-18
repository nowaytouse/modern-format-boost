#!/bin/bash
# 分析 /Users/user/Downloads/1 转换情况

SOURCE="/Users/user/Downloads/1"
OUTPUT="/Users/user/Downloads/1_converted"

echo "📊 分析转换完整性..."
echo ""

# 统计源目录文件
echo "=== 源目录统计 ==="
total_source=$(find "$SOURCE" -type f | wc -l | xargs)
echo "总文件数: $total_source"

# 按类型统计
echo ""
echo "图像文件:"
find "$SOURCE" -type f \( -iname "*.jpg" -o -iname "*.jpeg" -o -iname "*.jpe" -o -iname "*.jfif" -o -iname "*.png" -o -iname "*.webp" -o -iname "*.heic" -o -iname "*.avif" \) | wc -l | xargs

echo "视频文件:"
find "$SOURCE" -type f \( -iname "*.mp4" -o -iname "*.mov" -o -iname "*.avi" -o -iname "*.mkv" -o -iname "*.flv" -o -iname "*.wmv" -o -iname "*.m4v" \) | wc -l | xargs

echo "其他文件:"
find "$SOURCE" -type f ! \( -iname "*.jpg" -o -iname "*.jpeg" -o -iname "*.jpe" -o -iname "*.jfif" -o -iname "*.png" -o -iname "*.webp" -o -iname "*.heic" -o -iname "*.avif" -o -iname "*.mp4" -o -iname "*.mov" -o -iname "*.avi" -o -iname "*.mkv" -o -iname "*.flv" -o -iname "*.wmv" -o -iname "*.m4v" \) | wc -l | xargs

# 统计输出目录
echo ""
echo "=== 输出目录统计 ==="
total_output=$(find "$OUTPUT" -type f | wc -l | xargs)
echo "总文件数: $total_output"

# 计算差异
echo ""
echo "=== 差异分析 ==="
diff=$((total_source - total_output))
if [ $diff -eq 0 ]; then
    echo "✅ 文件数量匹配！无遗漏"
elif [ $diff -gt 0 ]; then
    echo "⚠️  缺少 $diff 个文件"
else
    echo "ℹ️  多出 ${diff#-} 个文件（可能是动图转换）"
fi

# 查找可能遗漏的文件
if [ $diff -gt 0 ]; then
    echo ""
    echo "=== 查找遗漏文件 ==="
    
    # 创建临时文件列表
    find "$SOURCE" -type f -exec basename {} \; | sort > /tmp/source_files.txt
    find "$OUTPUT" -type f -exec basename {} \; | sort > /tmp/output_files.txt
    
    # 找出源目录有但输出目录没有的文件
    missing=$(comm -23 /tmp/source_files.txt /tmp/output_files.txt | wc -l | xargs)
    echo "基于文件名的遗漏数: $missing"
    
    if [ $missing -gt 0 ] && [ $missing -le 50 ]; then
        echo ""
        echo "遗漏的文件名:"
        comm -23 /tmp/source_files.txt /tmp/output_files.txt | head -20
        if [ $missing -gt 20 ]; then
            echo "... (还有 $((missing - 20)) 个)"
        fi
    fi
    
    # 按扩展名分析遗漏
    echo ""
    echo "=== 按扩展名分析遗漏 ==="
    for ext in jpg jpeg jpe jfif png webp heic avif mp4 mov avi mkv flv wmv m4v psd txt xmp; do
        src_count=$(find "$SOURCE" -type f -iname "*.$ext" | wc -l | xargs)
        out_count=$(find "$OUTPUT" -type f -iname "*.$ext" -o -iname "*.jxl" | wc -l | xargs)
        if [ $src_count -gt 0 ]; then
            echo ".$ext: $src_count → $out_count"
        fi
    done
    
    rm -f /tmp/source_files.txt /tmp/output_files.txt
fi

echo ""
echo "✅ 分析完成"
