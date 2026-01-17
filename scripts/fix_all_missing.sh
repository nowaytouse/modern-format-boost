#!/opt/homebrew/bin/bash
# 修复所有遗漏文件 - 确保无遗漏转换
# 
# 遗漏原因：
# 1. GIF 文件：被跳过因为重新编码会增大文件 → 直接复制
# 2. PNG 文件：可能转换失败 → 重新尝试转换，失败则复制
# 3. MP4 视频：短视频被跳过 → 直接复制原始文件
#
# 🔥 无遗漏设计：所有遗漏文件都会被处理到输出目录

SOURCE="/Users/user/Downloads/1"
OUTPUT="/Users/user/Downloads/1_converted"

# 颜色定义
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
BOLD='\033[1m'
DIM='\033[2m'
NC='\033[0m'

echo -e "${CYAN}${BOLD}"
echo "╔══════════════════════════════════════════════════════════════════════════╗"
echo "║  🔧 修复遗漏文件 - 确保无遗漏转换                                         ║"
echo "╚══════════════════════════════════════════════════════════════════════════╝"
echo -e "${NC}"

# 创建临时文件存储遗漏列表
MISSING_LIST=$(mktemp)

# 获取遗漏文件列表
python3 << 'PYEOF' > "$MISSING_LIST"
import os
from pathlib import Path

source = Path("/Users/user/Downloads/1")
converted = Path("/Users/user/Downloads/1_converted")

# Get converted files (stem + directory)
converted_files = set()
for f in converted.rglob("*"):
    if f.is_file() and f.name != ".DS_Store":
        rel_path = f.relative_to(converted)
        rel_dir = rel_path.parent
        stem = f.stem
        converted_files.add((str(rel_dir), stem))

# Find missing files
for f in source.rglob("*"):
    if f.is_file() and f.name not in [".DS_Store"] and not f.suffix.lower() == ".xmp":
        rel_path = f.relative_to(source)
        rel_dir = str(rel_path.parent)
        stem = f.stem
        if (rel_dir, stem) not in converted_files:
            print(str(f))
PYEOF

TOTAL_MISSING=$(wc -l < "$MISSING_LIST" | tr -d ' ')

if [[ "$TOTAL_MISSING" -eq 0 ]]; then
    echo -e "${GREEN}✅ 没有遗漏文件！${NC}"
    rm -f "$MISSING_LIST"
    exit 0
fi

# 分类统计
GIF_COUNT=$(grep -i '\.gif$' "$MISSING_LIST" | wc -l | tr -d ' ')
PNG_COUNT=$(grep -i '\.png$' "$MISSING_LIST" | wc -l | tr -d ' ')
MP4_COUNT=$(grep -i '\.mp4$' "$MISSING_LIST" | wc -l | tr -d ' ')
OTHER_COUNT=$((TOTAL_MISSING - GIF_COUNT - PNG_COUNT - MP4_COUNT))

echo -e "${BLUE}📊 统计遗漏文件...${NC}"
echo -e "  ${BOLD}总遗漏: $TOTAL_MISSING${NC}"
echo -e "  🖼️  PNG: $PNG_COUNT | 🎞️  GIF: $GIF_COUNT | 📹 MP4: $MP4_COUNT | 📦 其他: $OTHER_COUNT"
echo ""

# 计数器
copied=0
converted=0

echo -e "${CYAN}🔧 处理遗漏文件...${NC}"
echo ""

current=0
while IFS= read -r file; do
    [[ -z "$file" ]] && continue
    ((current++))
    
    rel_path="${file#$SOURCE/}"
    out_dir="$OUTPUT/$(dirname "$rel_path")"
    filename=$(basename "$file")
    stem="${filename%.*}"
    ext="${filename##*.}"
    ext_lower=$(echo "$ext" | tr '[:upper:]' '[:lower:]')
    
    # 创建输出目录
    mkdir -p "$out_dir"
    
    case "$ext_lower" in
        gif)
            # GIF: 直接复制（重新编码会增大文件）
            cp -p "$file" "$out_dir/$filename"
            echo -e "[$current/$TOTAL_MISSING] ${GREEN}✓${NC} [复制] $rel_path"
            ((copied++))
            ;;
        
        png)
            # PNG: 尝试转换为 JXL，失败则复制
            out_file="$out_dir/$stem.jxl"
            if [[ -f "$out_file" ]]; then
                echo -e "[$current/$TOTAL_MISSING] ${DIM}⏭  [已存在] $rel_path${NC}"
            else
                # 使用 cjxl 直接转换
                if command -v cjxl &> /dev/null; then
                    if cjxl "$file" "$out_file" -q 100 --lossless_jpeg=0 2>/dev/null; then
                        echo -e "[$current/$TOTAL_MISSING] ${GREEN}✓${NC} [JXL] $rel_path"
                        ((converted++))
                    else
                        # 转换失败，复制原始文件
                        cp -p "$file" "$out_dir/$filename"
                        echo -e "[$current/$TOTAL_MISSING] ${YELLOW}⚠${NC} [复制] $rel_path (JXL失败)"
                        ((copied++))
                    fi
                else
                    # 没有 cjxl，直接复制
                    cp -p "$file" "$out_dir/$filename"
                    echo -e "[$current/$TOTAL_MISSING] ${GREEN}✓${NC} [复制] $rel_path"
                    ((copied++))
                fi
            fi
            ;;
        
        mp4|mov|avi|mkv|webm|m4v)
            # 视频: 直接复制（短视频或无法压缩的视频）
            out_file="$out_dir/$filename"
            if [[ -f "$out_file" ]]; then
                echo -e "[$current/$TOTAL_MISSING] ${DIM}⏭  [已存在] $rel_path${NC}"
            else
                cp -p "$file" "$out_file"
                echo -e "[$current/$TOTAL_MISSING] ${GREEN}✓${NC} [复制] $rel_path"
                ((copied++))
            fi
            ;;
        
        *)
            # 其他文件: 直接复制
            out_file="$out_dir/$filename"
            if [[ -f "$out_file" ]]; then
                echo -e "[$current/$TOTAL_MISSING] ${DIM}⏭  [已存在] $rel_path${NC}"
            else
                cp -p "$file" "$out_file"
                echo -e "[$current/$TOTAL_MISSING] ${GREEN}✓${NC} [复制] $rel_path"
                ((copied++))
            fi
            ;;
    esac
done < "$MISSING_LIST"

rm -f "$MISSING_LIST"

echo ""
echo -e "${GREEN}${BOLD}╭─────────────────────────────────────────────────────────────────────────╮"
echo -e "│     ✅ 遗漏修复完成！                                                   │"
echo -e "╰─────────────────────────────────────────────────────────────────────────╯${NC}"
echo -e "  已复制: $copied 个"
echo -e "  已转换: $converted 个"
echo ""

# 最终验证
echo -e "${BLUE}📊 最终验证...${NC}"

FINAL_MISSING=$(python3 << 'PYEOF'
import os
from pathlib import Path

source = Path("/Users/user/Downloads/1")
converted = Path("/Users/user/Downloads/1_converted")

converted_files = set()
for f in converted.rglob("*"):
    if f.is_file() and f.name != ".DS_Store":
        rel_path = f.relative_to(converted)
        rel_dir = rel_path.parent
        stem = f.stem
        converted_files.add((str(rel_dir), stem))

count = 0
for f in source.rglob("*"):
    if f.is_file() and f.name not in [".DS_Store"] and not f.suffix.lower() == ".xmp":
        rel_path = f.relative_to(source)
        rel_dir = str(rel_path.parent)
        stem = f.stem
        if (rel_dir, stem) not in converted_files:
            count += 1
print(count)
PYEOF
)

if [[ "$FINAL_MISSING" -eq 0 ]]; then
    echo -e "${GREEN}✅ 验证通过：无遗漏文件！${NC}"
else
    echo -e "${YELLOW}⚠️  仍有 $FINAL_MISSING 个遗漏文件${NC}"
fi
