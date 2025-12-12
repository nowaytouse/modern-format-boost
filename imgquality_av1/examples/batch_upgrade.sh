#!/opt/homebrew/bin/bash

# ============================================================================
# 🚀 批量格式升级示例
# ============================================================================
#
# 批量分析目录中的图像，并根据质量评估自动升级到JXL格式
#
# Usage: ./batch_upgrade.sh <input_directory> [output_directory]
# ============================================================================

set -e

# 检查参数
if [ $# -eq 0 ]; then
    echo "❌ Error: No input directory specified"
    echo "Usage: $0 <input_directory> [output_directory]"
    exit 1
fi

INPUT_DIR="$1"
OUTPUT_DIR="${2:-./jxl_output}"

# 检查输入目录是否存在
if [ ! -d "$INPUT_DIR" ]; then
    echo "❌ Error: Directory not found: $INPUT_DIR"
    exit 1
fi

# 检查 imgquality 是否可用
if ! command -v imgquality &> /dev/null; then
    echo "❌ Error: imgquality not found"
    echo "Please build the project first and add to PATH"
    exit 1
fi

# 创建输出目录
mkdir -p "$OUTPUT_DIR"

echo "╔══════════════════════════════════════════════╗"
echo "║   🚀 Batch Format Upgrade                    ║"
echo "╚══════════════════════════════════════════════╝"
echo ""
echo "📁 Input:  $INPUT_DIR"
echo "📁 Output: $OUTPUT_DIR"
echo ""

# 计数器
TOTAL=0
CONVERTED=0
SKIPPED=0
FAILED=0

# 遍历图像文件
for img in "$INPUT_DIR"/*.{png,jpg,jpeg,webp,gif,PNG,JPG,JPEG,WEBP,GIF} 2>/dev/null; do
    # 跳过通配符本身（没有匹配文件时）
    [ -e "$img" ] || continue
    
    ((TOTAL++)) || true
    
    echo "──────────────────────────────────────────────"
    echo "📷 Processing: $(basename "$img")"
    
    # 获取分析结果和推荐
    ANALYSIS=$(imgquality analyze "$img" --output json --recommend 2>/dev/null)
    
    if [ $? -eq 0 ]; then
        # 提取推荐格式
        RECOMMENDED=$(echo "$ANALYSIS" | jq -r '.recommendation.recommended_format // "N/A"')
        
        if [ "$RECOMMENDED" = "JXL" ]; then
            echo "💡 Recommendation: Upgrade to JXL"
            
            # 执行转换
            if imgquality convert "$img" --to jxl --output "$OUTPUT_DIR" 2>/dev/null; then
                ((CONVERTED++)) || true
                echo "✅ Converted successfully"
            else
                ((FAILED++)) || true
                echo "❌ Conversion failed"
            fi
        else
            ((SKIPPED++)) || true
            echo "⏭️  Skipped: No upgrade recommended (format: $RECOMMENDED)"
        fi
    else
        ((FAILED++)) || true
        echo "❌ Analysis failed"
    fi
done

# 最终报告
echo ""
echo "╔══════════════════════════════════════════════╗"
echo "║   📊 Batch Processing Complete               ║"
echo "╚══════════════════════════════════════════════╝"
echo ""
echo "📈 Statistics:"
echo "   Total files:     $TOTAL"
echo "   ✅ Converted:    $CONVERTED"
echo "   ⏭️  Skipped:     $SKIPPED"
echo "   ❌ Failed:       $FAILED"
echo ""
echo "📁 Output directory: $OUTPUT_DIR"
