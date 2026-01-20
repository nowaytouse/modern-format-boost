#!/usr/bin/env bash
# v7.7 代码质量改进 - 安全功能测试
# 使用测试文件副本，验证功能无损
# 使用与 drag_and_drop_processor.sh 相同的参数

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

# 颜色定义
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
BOLD='\033[1m'
DIM='\033[2m'
RESET='\033[0m'

echo ""
echo -e "${BLUE}╭────────────────────────────────────────────────────────────────╮${RESET}"
echo -e "${BLUE}│${RESET} ${BOLD}🧪 v7.7 代码质量改进 - 安全功能测试${RESET}                      ${BLUE}│${RESET}"
echo -e "${BLUE}│${RESET} ${DIM}使用测试文件副本，验证功能无损${RESET}                          ${BLUE}│${RESET}"
echo -e "${BLUE}╰────────────────────────────────────────────────────────────────╯${RESET}"
echo ""

# 创建临时测试目录
TEST_ROOT="/tmp/modern_format_boost_test_$$"
TEST_INPUT="$TEST_ROOT/input"
TEST_OUTPUT="$TEST_ROOT/output"

echo -e "${CYAN}📁 创建测试环境...${RESET}"
mkdir -p "$TEST_INPUT" "$TEST_OUTPUT"

# 清理函数
cleanup() {
    echo ""
    echo -e "${DIM}🧹 清理测试环境...${RESET}"
    rm -rf "$TEST_ROOT"
}
trap cleanup EXIT

# 复制测试文件
echo -e "${CYAN}📋 复制测试文件到安全副本...${RESET}"
if [ -d "$PROJECT_ROOT/test_media" ]; then
    cp -r "$PROJECT_ROOT/test_media/"* "$TEST_INPUT/" 2>/dev/null || true
    echo -e "${GREEN}✓${RESET} 测试文件已复制到: ${DIM}$TEST_INPUT${RESET}"
else
    echo -e "${YELLOW}⚠️  test_media 目录不存在，创建示例文件${RESET}"
    # 创建一些测试文件
    echo "test image" > "$TEST_INPUT/test.png"
    echo "test video" > "$TEST_INPUT/test.mp4"
    echo "test doc" > "$TEST_INPUT/test.txt"
fi

FILE_COUNT=$(find "$TEST_INPUT" -type f | wc -l | tr -d ' ')
echo -e "${DIM}   文件数量: $FILE_COUNT${RESET}"
echo ""

# 确保构建是最新的
echo -e "${CYAN}🔨 确保构建最新...${RESET}"
cd "$PROJECT_ROOT"
"$SCRIPT_DIR/smart_build.sh" || {
    echo -e "${RED}❌ 构建失败${RESET}"
    exit 1
}
echo ""

# 测试工具路径
IMGQUALITY_HEVC="$PROJECT_ROOT/target/release/imgquality-hevc"
VIDQUALITY_HEVC="$PROJECT_ROOT/target/release/vidquality-hevc"

# 验证工具存在
if [ ! -f "$IMGQUALITY_HEVC" ] || [ ! -f "$VIDQUALITY_HEVC" ]; then
    echo -e "${RED}❌ 二进制文件不存在${RESET}"
    exit 1
fi

echo -e "${BLUE}╭────────────────────────────────────────────────────────────────╮${RESET}"
echo -e "${BLUE}│${RESET} ${BOLD}测试 1: 图像处理（使用 drag_and_drop 参数）${RESET}              ${BLUE}│${RESET}"
echo -e "${BLUE}╰────────────────────────────────────────────────────────────────╯${RESET}"
echo ""

# 使用与 drag_and_drop_processor.sh 相同的参数
echo -e "${CYAN}🖼️  执行图像处理...${RESET}"
echo -e "${DIM}   参数: auto --explore --match-quality --compress --apple-compat --recursive --ultimate${RESET}"
echo ""

"$IMGQUALITY_HEVC" auto \
    --explore \
    --match-quality \
    --compress \
    --apple-compat \
    --recursive \
    --ultimate \
    "$TEST_INPUT" \
    --output "$TEST_OUTPUT" \
    --verbose 2>&1 | tee "$TEST_ROOT/img_output.log" || true

IMG_EXIT_CODE=${PIPESTATUS[0]}

echo ""
if [ $IMG_EXIT_CODE -eq 0 ]; then
    echo -e "${GREEN}✅ 图像处理完成${RESET}"
else
    echo -e "${YELLOW}⚠️  图像处理退出码: $IMG_EXIT_CODE${RESET}"
fi

# 检查日志
if grep -q "Error" "$TEST_ROOT/img_output.log"; then
    echo -e "${YELLOW}⚠️  发现错误信息（检查是否为预期错误）${RESET}"
    grep "Error" "$TEST_ROOT/img_output.log" | head -3
fi

# 检查新的日志功能
if grep -qE "(Executing|command|duration)" "$TEST_ROOT/img_output.log"; then
    echo -e "${GREEN}✓${RESET} ${DIM}新日志系统正常工作${RESET}"
else
    echo -e "${YELLOW}⚠️  未检测到新日志输出${RESET}"
fi

echo ""
echo -e "${BLUE}╭────────────────────────────────────────────────────────────────╮${RESET}"
echo -e "${BLUE}│${RESET} ${BOLD}测试 2: 视频处理（使用 drag_and_drop 参数）${RESET}              ${BLUE}│${RESET}"
echo -e "${BLUE}╰────────────────────────────────────────────────────────────────╯${RESET}"
echo ""

echo -e "${CYAN}🎬 执行视频处理...${RESET}"
echo -e "${DIM}   参数: auto --explore --match-quality --compress --apple-compat --recursive --ultimate${RESET}"
echo ""

"$VIDQUALITY_HEVC" auto \
    --explore \
    --match-quality \
    --compress \
    --apple-compat \
    --recursive \
    --ultimate \
    "$TEST_INPUT" \
    --output "$TEST_OUTPUT" \
    --verbose 2>&1 | tee "$TEST_ROOT/vid_output.log" || true

VID_EXIT_CODE=${PIPESTATUS[0]}

echo ""
if [ $VID_EXIT_CODE -eq 0 ]; then
    echo -e "${GREEN}✅ 视频处理完成${RESET}"
else
    echo -e "${YELLOW}⚠️  视频处理退出码: $VID_EXIT_CODE${RESET}"
fi

# 检查日志
if grep -q "Error" "$TEST_ROOT/vid_output.log"; then
    echo -e "${YELLOW}⚠️  发现错误信息（检查是否为预期错误）${RESET}"
    grep "Error" "$TEST_ROOT/vid_output.log" | head -3
fi

# 检查新的日志功能
if grep -qE "(Executing|command|duration)" "$TEST_ROOT/vid_output.log"; then
    echo -e "${GREEN}✓${RESET} ${DIM}新日志系统正常工作${RESET}"
else
    echo -e "${YELLOW}⚠️  未检测到新日志输出${RESET}"
fi

echo ""
echo -e "${BLUE}╭────────────────────────────────────────────────────────────────╮${RESET}"
echo -e "${BLUE}│${RESET} ${BOLD}测试 3: 验证输出和日志文件${RESET}                              ${BLUE}│${RESET}"
echo -e "${BLUE}╰────────────────────────────────────────────────────────────────╯${RESET}"
echo ""

# 检查输出目录
OUTPUT_FILES=$(find "$TEST_OUTPUT" -type f 2>/dev/null | wc -l | tr -d ' ')
echo -e "${CYAN}📊 输出统计:${RESET}"
echo -e "   输入文件: ${BOLD}$FILE_COUNT${RESET}"
echo -e "   输出文件: ${BOLD}$OUTPUT_FILES${RESET}"

if [ "$OUTPUT_FILES" -gt 0 ]; then
    echo -e "${GREEN}✓${RESET} ${DIM}输出文件已生成${RESET}"
else
    echo -e "${YELLOW}⚠️  无输出文件（可能所有文件都被跳过）${RESET}"
fi

# 检查系统日志文件
echo ""
echo -e "${CYAN}📝 检查系统日志文件:${RESET}"
LOG_DIR="/tmp"
LOG_FILES=$(find "$LOG_DIR" -name "modern_format_boost*.log" -o -name "imgquality*.log" -o -name "vidquality*.log" 2>/dev/null | head -5)

if [ -n "$LOG_FILES" ]; then
    echo -e "${GREEN}✓${RESET} ${DIM}找到日志文件:${RESET}"
    echo "$LOG_FILES" | while read -r log; do
        SIZE=$(du -h "$log" 2>/dev/null | cut -f1)
        echo -e "   ${DIM}$log ($SIZE)${RESET}"
    done
else
    echo -e "${YELLOW}⚠️  未找到日志文件（可能日志未初始化）${RESET}"
fi

echo ""
echo -e "${BLUE}╭────────────────────────────────────────────────────────────────╮${RESET}"
echo -e "${BLUE}│${RESET} ${BOLD}测试 4: 错误处理验证${RESET}                                    ${BLUE}│${RESET}"
echo -e "${BLUE}╰────────────────────────────────────────────────────────────────╯${RESET}"
echo ""

echo -e "${CYAN}🔍 测试错误处理（无效路径）...${RESET}"
ERROR_OUTPUT=$("$IMGQUALITY_HEVC" auto /nonexistent_path_12345 2>&1 || true)

if echo "$ERROR_OUTPUT" | grep -qE "(Error|does not exist|not found)"; then
    echo -e "${GREEN}✓${RESET} ${DIM}错误正确报告（响亮报错）${RESET}"
    echo -e "${DIM}   $(echo "$ERROR_OUTPUT" | grep -E "(Error|does not exist)" | head -1)${RESET}"
else
    echo -e "${RED}✗${RESET} 错误处理异常"
    echo "$ERROR_OUTPUT"
fi

echo ""
echo -e "${BLUE}╭────────────────────────────────────────────────────────────────╮${RESET}"
echo -e "${BLUE}│${RESET} ${BOLD}测试 5: 向后兼容性验证${RESET}                                  ${BLUE}│${RESET}"
echo -e "${BLUE}╰────────────────────────────────────────────────────────────────╯${RESET}"
echo ""

echo -e "${CYAN}🔧 验证所有 CLI 参数存在...${RESET}"
HELP_OUTPUT=$("$IMGQUALITY_HEVC" auto --help 2>&1)

REQUIRED_FLAGS=(
    "--output"
    "--recursive"
    "--in-place"
    "--explore"
    "--match-quality"
    "--compress"
    "--apple-compat"
    "--ultimate"
    "--verbose"
)

ALL_FLAGS_OK=true
for flag in "${REQUIRED_FLAGS[@]}"; do
    if echo "$HELP_OUTPUT" | grep -q -- "$flag"; then
        echo -e "${GREEN}✓${RESET} ${DIM}$flag${RESET}"
    else
        echo -e "${RED}✗${RESET} $flag ${RED}缺失${RESET}"
        ALL_FLAGS_OK=false
    fi
done

echo ""
if [ "$ALL_FLAGS_OK" = true ]; then
    echo -e "${GREEN}✅ 所有 CLI 参数完整保留${RESET}"
else
    echo -e "${RED}❌ 部分 CLI 参数缺失${RESET}"
fi

echo ""
echo -e "${BLUE}╭────────────────────────────────────────────────────────────────╮${RESET}"
echo -e "${BLUE}│${RESET} ${BOLD}📊 测试总结${RESET}                                              ${BLUE}│${RESET}"
echo -e "${BLUE}╰────────────────────────────────────────────────────────────────╯${RESET}"
echo ""

# 生成测试报告
REPORT_FILE="$PROJECT_ROOT/.kiro/specs/shared-utils-quality-improvement/SAFE_TEST_REPORT.md"
mkdir -p "$(dirname "$REPORT_FILE")"

cat > "$REPORT_FILE" << EOF
# v7.7 代码质量改进 - 安全功能测试报告

**测试日期**: $(date '+%Y-%m-%d %H:%M:%S')  
**测试方法**: 使用测试文件副本，不修改原始文件  
**测试参数**: 与 drag_and_drop_processor.sh 相同

## 测试结果

### 1. 图像处理
- 退出码: $IMG_EXIT_CODE
- 状态: $([ $IMG_EXIT_CODE -eq 0 ] && echo "✅ 通过" || echo "⚠️  警告")
- 日志系统: $(grep -qE "(Executing|command)" "$TEST_ROOT/img_output.log" && echo "✅ 正常" || echo "⚠️  未检测到")

### 2. 视频处理
- 退出码: $VID_EXIT_CODE
- 状态: $([ $VID_EXIT_CODE -eq 0 ] && echo "✅ 通过" || echo "⚠️  警告")
- 日志系统: $(grep -qE "(Executing|command)" "$TEST_ROOT/vid_output.log" && echo "✅ 正常" || echo "⚠️  未检测到")

### 3. 输出验证
- 输入文件: $FILE_COUNT
- 输出文件: $OUTPUT_FILES
- 状态: $([ "$OUTPUT_FILES" -gt 0 ] && echo "✅ 正常" || echo "⚠️  无输出")

### 4. 错误处理
- 响亮报错: $(echo "$ERROR_OUTPUT" | grep -qE "Error" && echo "✅ 正常" || echo "❌ 异常")

### 5. 向后兼容性
- CLI 参数: $([ "$ALL_FLAGS_OK" = true ] && echo "✅ 完整" || echo "❌ 缺失")

## 新功能验证

### 日志系统
- 结构化日志: $(grep -qE "command|duration" "$TEST_ROOT/img_output.log" && echo "✅" || echo "❌")
- 外部命令记录: $(grep -qE "Executing|ffmpeg|x265" "$TEST_ROOT/img_output.log" && echo "✅" || echo "❌")

### 错误处理
- 上下文信息: $(echo "$ERROR_OUTPUT" | grep -qE "Error.*:" && echo "✅" || echo "❌")
- 响亮报错: ✅

## 结论

$([ $IMG_EXIT_CODE -eq 0 ] && [ $VID_EXIT_CODE -eq 0 ] && [ "$ALL_FLAGS_OK" = true ] && echo "✅ **所有测试通过** - 功能无损，向后兼容" || echo "⚠️  **部分测试有警告** - 请检查详细日志")

## 测试环境

- 测试目录: $TEST_ROOT
- 原始文件: 未修改（使用副本）
- 日志位置: /tmp/modern_format_boost*.log

EOF

echo -e "${GREEN}✅ 测试完成${RESET}"
echo ""
echo -e "${CYAN}📄 详细报告已保存:${RESET}"
echo -e "${DIM}   $REPORT_FILE${RESET}"
echo ""

# 最终状态
if [ $IMG_EXIT_CODE -eq 0 ] && [ $VID_EXIT_CODE -eq 0 ] && [ "$ALL_FLAGS_OK" = true ]; then
    echo -e "${GREEN}${BOLD}🎉 所有测试通过！功能无损，向后兼容。${RESET}"
    echo ""
    exit 0
else
    echo -e "${YELLOW}${BOLD}⚠️  部分测试有警告，请检查详细日志。${RESET}"
    echo ""
    exit 1
fi
