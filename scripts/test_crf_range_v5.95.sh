#!/opt/homebrew/bin/bash
# 🔥 v5.95 CRF Range Test - 验证 -15.0 是否过于激进
#
# 测试目的：
# 1. 验证 cpu_start - 15.0 的搜索范围是否合理
# 2. 对比 -3.0 vs -15.0 的迭代次数和最终质量
# 3. 确保算法能真正"撞墙"而不是提前停止

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

# 颜色
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
BOLD='\033[1m'
DIM='\033[2m'
NC='\033[0m'

echo -e "${CYAN}${BOLD}"
echo "╔══════════════════════════════════════════════════════════════════════════╗"
echo "║     🔬 v5.95 CRF Range Test - 验证 -15.0 搜索范围                        ║"
echo "╚══════════════════════════════════════════════════════════════════════════╝"
echo -e "${NC}"

# 编译
echo -e "${CYAN}📦 Building...${NC}"
"$PROJECT_ROOT/smart_build.sh" || {
    echo -e "${RED}❌ Build failed${NC}"
    exit 1
}

VIDQUALITY_HEVC="$PROJECT_ROOT/vidquality_hevc/target/release/vidquality-hevc"

# 测试文件（使用用户提供的或默认）
TEST_FILE="${1:-}"
if [[ -z "$TEST_FILE" ]]; then
    echo -e "${YELLOW}⚠️  请提供测试视频文件路径${NC}"
    echo -e "${DIM}用法: $0 <video_file>${NC}"
    echo ""
    echo -e "${CYAN}示例:${NC}"
    echo "  $0 ~/Movies/test.mp4"
    echo "  $0 /path/to/video.mov"
    exit 1
fi

if [[ ! -f "$TEST_FILE" ]]; then
    echo -e "${RED}❌ 文件不存在: $TEST_FILE${NC}"
    exit 1
fi

# 获取文件信息
FILE_SIZE=$(stat -f%z "$TEST_FILE" 2>/dev/null || stat -c%s "$TEST_FILE" 2>/dev/null)
FILE_SIZE_MB=$(echo "scale=2; $FILE_SIZE / 1024 / 1024" | bc)
DURATION=$(ffprobe -v error -show_entries format=duration -of default=noprint_wrappers=1:nokey=1 "$TEST_FILE" 2>/dev/null | cut -d. -f1)

echo ""
echo -e "${CYAN}📁 测试文件:${NC} $TEST_FILE"
echo -e "${CYAN}📊 文件大小:${NC} ${FILE_SIZE_MB} MB"
echo -e "${CYAN}⏱️  时长:${NC} ${DURATION}s"
echo ""

# 创建临时输出目录
OUTPUT_DIR=$(mktemp -d)
trap "rm -rf $OUTPUT_DIR" EXIT

echo -e "${CYAN}${BOLD}═══════════════════════════════════════════════════════════════════════════${NC}"
echo -e "${CYAN}📍 测试 1: 使用双击脚本参数 (--explore --match-quality --compress)${NC}"
echo -e "${CYAN}${BOLD}═══════════════════════════════════════════════════════════════════════════${NC}"
echo ""

# 记录开始时间
START_TIME=$(date +%s)

# 执行转换（使用双击脚本的参数）
"$VIDQUALITY_HEVC" auto "$TEST_FILE" \
    --explore \
    --match-quality true \
    --compress \
    --apple-compat \
    --output "$OUTPUT_DIR" 2>&1 | tee "$OUTPUT_DIR/log.txt"

END_TIME=$(date +%s)
ELAPSED=$((END_TIME - START_TIME))

echo ""
echo -e "${CYAN}${BOLD}═══════════════════════════════════════════════════════════════════════════${NC}"
echo -e "${CYAN}📊 测试结果分析${NC}"
echo -e "${CYAN}${BOLD}═══════════════════════════════════════════════════════════════════════════${NC}"
echo ""

# 分析日志
echo -e "${YELLOW}🔍 关键指标:${NC}"

# 提取 CPU 搜索范围
CPU_RANGE=$(grep "CPU search range" "$OUTPUT_DIR/log.txt" 2>/dev/null | tail -1)
if [[ -n "$CPU_RANGE" ]]; then
    echo -e "   ${CYAN}CPU搜索范围:${NC} $CPU_RANGE"
fi

# 提取迭代次数
ITERATIONS=$(grep -E "iterations|迭代" "$OUTPUT_DIR/log.txt" 2>/dev/null | tail -1)
if [[ -n "$ITERATIONS" ]]; then
    echo -e "   ${CYAN}迭代次数:${NC} $ITERATIONS"
fi

# 提取最终 CRF
FINAL_CRF=$(grep -E "Final.*CRF|optimal.*CRF" "$OUTPUT_DIR/log.txt" 2>/dev/null | tail -1)
if [[ -n "$FINAL_CRF" ]]; then
    echo -e "   ${CYAN}最终CRF:${NC} $FINAL_CRF"
fi

# 提取 SSIM
FINAL_SSIM=$(grep -E "SSIM.*0\.[0-9]+" "$OUTPUT_DIR/log.txt" 2>/dev/null | tail -1)
if [[ -n "$FINAL_SSIM" ]]; then
    echo -e "   ${CYAN}最终SSIM:${NC} $FINAL_SSIM"
fi

# 检查是否撞墙
WALL_HIT=$(grep -E "WALL|OVERSHOOT|BOUNDARY" "$OUTPUT_DIR/log.txt" 2>/dev/null | tail -3)
if [[ -n "$WALL_HIT" ]]; then
    echo ""
    echo -e "${YELLOW}🧱 撞墙检测:${NC}"
    echo "$WALL_HIT" | while read line; do
        echo -e "   $line"
    done
fi

# 输出文件大小
OUTPUT_FILE=$(find "$OUTPUT_DIR" -name "*.mp4" -o -name "*.mov" 2>/dev/null | head -1)
if [[ -n "$OUTPUT_FILE" && -f "$OUTPUT_FILE" ]]; then
    OUTPUT_SIZE=$(stat -f%z "$OUTPUT_FILE" 2>/dev/null || stat -c%s "$OUTPUT_FILE" 2>/dev/null)
    OUTPUT_SIZE_MB=$(echo "scale=2; $OUTPUT_SIZE / 1024 / 1024" | bc)
    COMPRESSION_PCT=$(echo "scale=1; ($OUTPUT_SIZE - $FILE_SIZE) * 100 / $FILE_SIZE" | bc)
    
    echo ""
    echo -e "${GREEN}📊 压缩结果:${NC}"
    echo -e "   输入: ${FILE_SIZE_MB} MB"
    echo -e "   输出: ${OUTPUT_SIZE_MB} MB"
    echo -e "   压缩率: ${COMPRESSION_PCT}%"
fi

echo ""
echo -e "${CYAN}⏱️  总耗时:${NC} ${ELAPSED}s"
echo ""

# 分析 -15.0 是否过于激进
echo -e "${YELLOW}${BOLD}═══════════════════════════════════════════════════════════════════════════${NC}"
echo -e "${YELLOW}📋 -15.0 范围分析${NC}"
echo -e "${YELLOW}${BOLD}═══════════════════════════════════════════════════════════════════════════${NC}"
echo ""

# 检查是否有过多迭代
ITER_COUNT=$(grep -c "CRF [0-9]" "$OUTPUT_DIR/log.txt" 2>/dev/null || echo "0")
if [[ $ITER_COUNT -gt 40 ]]; then
    echo -e "${RED}⚠️  迭代次数过多 ($ITER_COUNT 次)${NC}"
    echo -e "${RED}   -15.0 可能过于激进，导致搜索范围过大${NC}"
    echo -e "${YELLOW}   建议: 考虑缩小到 -10.0 或 -8.0${NC}"
elif [[ $ITER_COUNT -gt 25 ]]; then
    echo -e "${YELLOW}⚠️  迭代次数较多 ($ITER_COUNT 次)${NC}"
    echo -e "${YELLOW}   -15.0 范围可能略大，但仍在可接受范围${NC}"
else
    echo -e "${GREEN}✅ 迭代次数合理 ($ITER_COUNT 次)${NC}"
    echo -e "${GREEN}   -15.0 范围设置合适${NC}"
fi

# 检查是否真正撞墙
if grep -q "OVERSHOOT\|SIZE WALL" "$OUTPUT_DIR/log.txt" 2>/dev/null; then
    echo -e "${GREEN}✅ 算法成功撞墙 (SIZE WALL)${NC}"
elif grep -q "QUALITY WALL" "$OUTPUT_DIR/log.txt" 2>/dev/null; then
    echo -e "${GREEN}✅ 算法触发质量墙 (QUALITY WALL)${NC}"
elif grep -q "MIN_CRF\|BOUNDARY" "$OUTPUT_DIR/log.txt" 2>/dev/null; then
    echo -e "${YELLOW}⚠️  算法到达 min_crf 边界${NC}"
    echo -e "${YELLOW}   这可能意味着 -15.0 范围不够大，或视频高度可压缩${NC}"
else
    echo -e "${RED}⚠️  未检测到明确的停止条件${NC}"
fi

echo ""
echo -e "${DIM}完整日志: $OUTPUT_DIR/log.txt${NC}"
echo ""
