#!/bin/bash
# 🔥 v6.9.11: 修复被跳过文件的XMP合并
# 从日志中提取被跳过的文件，为输出目录中的副本合并XMP

set -e

LOG_FILE="${1:-log3}"
SOURCE_DIR="/Users/user/Downloads/zz"
OUTPUT_DIR="/Users/user/Downloads/zz_converted"

if [ ! -f "$LOG_FILE" ]; then
    echo "❌ 日志文件不存在: $LOG_FILE"
    exit 1
fi

echo "🔧 XMP修复脚本 v6.9.11"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "📂 源目录: $SOURCE_DIR"
echo "📂 输出目录: $OUTPUT_DIR"
echo ""

# 统计计数器
TOTAL=0
FOUND_XMP=0
MERGED=0
NO_XMP=0
FAILED=0

# 提取被跳过的文件路径
echo "📋 提取被跳过的文件..."

# 1. 短动画跳过 - 提取冒号后的路径
grep "Skipping short animation" "$LOG_FILE" | sed 's/.*: //' > /tmp/skipped_files.txt 2>/dev/null || true

# 2. 现代格式跳过  
grep "Skipping modern lossy format" "$LOG_FILE" | sed 's/.*: //' >> /tmp/skipped_files.txt 2>/dev/null || true

# 3. 复制到输出目录的文件（质量失败等）
grep "Copied original to output dir:" "$LOG_FILE" | sed 's/.*: //' >> /tmp/skipped_files.txt 2>/dev/null || true

# 4. 质量保护的文件 - 从 "Copied to output dir" 提取
grep "Copied to output dir:" "$LOG_FILE" | sed 's/.*: //' >> /tmp/skipped_files.txt 2>/dev/null || true

# 去重
sort -u /tmp/skipped_files.txt > /tmp/skipped_files_unique.txt
TOTAL=$(wc -l < /tmp/skipped_files_unique.txt | tr -d ' ')

echo "📊 找到 $TOTAL 个被跳过的文件"
echo ""
echo "🔄 开始合并XMP..."
echo ""

while IFS= read -r file; do
    [ -z "$file" ] && continue
    
    # 判断是源文件还是输出文件
    if [[ "$file" == "$OUTPUT_DIR"* ]]; then
        # 已经是输出目录的文件，需要找到对应的源文件
        rel_path="${file#$OUTPUT_DIR/}"
        source_file="$SOURCE_DIR/$rel_path"
        dest_file="$file"
    else
        # 源文件，需要找到对应的输出文件
        source_file="$file"
        rel_path="${file#$SOURCE_DIR/}"
        dest_file="$OUTPUT_DIR/$rel_path"
    fi
    
    # 检查输出文件是否存在
    if [ ! -f "$dest_file" ]; then
        continue
    fi
    
    # 获取文件名（不含扩展名）和目录
    filename=$(basename "$source_file")
    stem="${filename%.*}"
    dir=$(dirname "$source_file")
    
    # 查找XMP边车
    xmp_file=""
    for candidate in "$dir/$stem.xmp" "$dir/$stem.XMP" "$dir/$filename.xmp"; do
        if [ -f "$candidate" ]; then
            xmp_file="$candidate"
            break
        fi
    done
    
    if [ -n "$xmp_file" ]; then
        ((FOUND_XMP++))
        # 使用exiftool合并XMP
        if exiftool -overwrite_original -tagsfromfile "$xmp_file" -all:all "$dest_file" 2>/dev/null; then
            echo "✅ $filename"
            ((MERGED++))
        else
            echo "⚠️ $filename (合并失败)"
            ((FAILED++))
        fi
    else
        ((NO_XMP++))
    fi
done < /tmp/skipped_files_unique.txt

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "📊 修复完成"
echo "   总文件数: $TOTAL"
echo "   找到XMP: $FOUND_XMP"
echo "   成功合并: $MERGED"
echo "   无XMP: $NO_XMP"
echo "   合并失败: $FAILED"

# 清理临时文件
rm -f /tmp/skipped_files.txt /tmp/skipped_files_unique.txt
