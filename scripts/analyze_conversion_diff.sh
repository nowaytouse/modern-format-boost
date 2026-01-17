#!/bin/bash
# 分析转换前后目录差异，找出遗漏的文件
# Usage: ./analyze_conversion_diff.sh <source_dir> <converted_dir>

SOURCE="${1:-/Users/user/Downloads/zz}"
CONVERTED="${2:-/Users/user/Downloads/zz_converted}"

echo "═══════════════════════════════════════════════════════════════"
echo "📊 转换差异分析报告"
echo "═══════════════════════════════════════════════════════════════"
echo "📁 源目录: $SOURCE"
echo "📁 转换目录: $CONVERTED"
echo ""

# 统计源目录
echo "【源目录统计】"
src_total=$(find "$SOURCE" -type f ! -name ".*" | wc -l | tr -d ' ')
src_xmp=$(find "$SOURCE" -type f -iname "*.xmp" | wc -l | tr -d ' ')
echo "  全部文件: $src_total"
echo "  XMP边车: $src_xmp"
echo "  预期输出: $((src_total - src_xmp))"
echo ""

# 统计转换目录
echo "【转换目录统计】"
conv_total=$(find "$CONVERTED" -type f ! -name ".*" | wc -l | tr -d ' ')
echo "  实际输出: $conv_total"
echo ""

diff=$((src_total - src_xmp - conv_total))
if [ $diff -eq 0 ]; then
    echo "✅ 数量匹配！无遗漏"
elif [ $diff -gt 0 ]; then
    echo "❌ 缺少 $diff 个文件！"
else
    echo "⚠️ 多出 $((-diff)) 个文件"
fi
echo ""

# 按扩展名统计源目录
echo "【源目录扩展名分布】"
find "$SOURCE" -type f ! -name ".*" | sed 's/.*\.//' | tr '[:upper:]' '[:lower:]' | sort | uniq -c | sort -rn | head -30
echo ""

# 按扩展名统计转换目录
echo "【转换目录扩展名分布】"
find "$CONVERTED" -type f ! -name ".*" | sed 's/.*\.//' | tr '[:upper:]' '[:lower:]' | sort | uniq -c | sort -rn | head -30
echo ""

# 找出源目录中存在但转换目录中不存在的文件（基于文件名stem）
echo "【遗漏文件分析】"
echo "正在分析..."

# 创建临时文件
src_stems=$(mktemp)
conv_stems=$(mktemp)

# 提取源目录文件名stem（不含扩展名）
find "$SOURCE" -type f ! -name ".*" ! -iname "*.xmp" -exec basename {} \; | sed 's/\.[^.]*$//' | sort -u > "$src_stems"

# 提取转换目录文件名stem
find "$CONVERTED" -type f ! -name ".*" -exec basename {} \; | sed 's/\.[^.]*$//' | sort -u > "$conv_stems"

# 找出差异
missing=$(comm -23 "$src_stems" "$conv_stems")
missing_count=$(echo "$missing" | grep -c . || echo 0)

echo "  源文件stem数: $(wc -l < "$src_stems" | tr -d ' ')"
echo "  转换文件stem数: $(wc -l < "$conv_stems" | tr -d ' ')"
echo "  缺失stem数: $missing_count"
echo ""

if [ $missing_count -gt 0 ] && [ $missing_count -lt 100 ]; then
    echo "【缺失文件列表（前50个）】"
    echo "$missing" | head -50 | while read stem; do
        # 找到源文件的完整路径和扩展名
        found=$(find "$SOURCE" -type f -name "${stem}.*" ! -iname "*.xmp" | head -1)
        if [ -n "$found" ]; then
            ext="${found##*.}"
            echo "  ❌ $stem.$ext"
        fi
    done
fi

# 按扩展名统计缺失文件
echo ""
echo "【缺失文件扩展名分布】"
for stem in $missing; do
    find "$SOURCE" -type f -name "${stem}.*" ! -iname "*.xmp" 2>/dev/null
done | sed 's/.*\.//' | tr '[:upper:]' '[:lower:]' | sort | uniq -c | sort -rn

# 清理
rm -f "$src_stems" "$conv_stems"

echo ""
echo "═══════════════════════════════════════════════════════════════"
