#!/usr/bin/env bash
# 🔥 安全的向后兼容性测试 v1.0
# 使用测试文件副本，与 drag_and_drop_processor.sh 相同的参数

set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
NC='\033[0m'

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

IMGQUALITY_HEVC="$PROJECT_ROOT/target/release/imgquality-hevc"
VIDQUALITY_HEVC="$PROJECT_ROOT/target/release/vidquality-hevc"

TOTAL=0
PASSED=0
FAILED=0

echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo -e "${BLUE}🔍 向后兼容性测试 (安全模式)${NC}"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

log_test() {
    echo -e "${CYAN}[Test $((TOTAL + 1))]${NC} $1"
}

pass() {
    TOTAL=$((TOTAL + 1))
    PASSED=$((PASSED + 1))
    echo -e "  ${GREEN}✅ PASS${NC}: $1"
}

fail() {
    TOTAL=$((TOTAL + 1))
    FAILED=$((FAILED + 1))
    echo -e "  ${RED}❌ FAIL${NC}: $1"
}

# 创建测试目录
TEST_DIR="/tmp/compat_test_$$"
mkdir -p "$TEST_DIR"

cleanup() {
    rm -rf "$TEST_DIR"
}
trap cleanup EXIT

# Phase 1: 二进制检查
echo -e "${BLUE}━━━ Phase 1: 二进制文件检查 ━━━${NC}"
echo ""

log_test "检查 imgquality-hevc"
if [ -f "$IMGQUALITY_HEVC" ]; then
    pass "存在"
else
    fail "不存在"
fi

log_test "检查 vidquality-hevc"
if [ -f "$VIDQUALITY_HEVC" ]; then
    pass "存在"
else
    fail "不存在"
fi
echo ""

# Phase 2: CLI 接口检查
echo -e "${BLUE}━━━ Phase 2: CLI 接口检查 ━━━${NC}"
echo ""

log_test "imgquality-hevc auto --help"
if "$IMGQUALITY_HEVC" auto --help 2>&1 | grep -q "Usage:"; then
    pass "帮助信息正常"
    
    HELP=$("$IMGQUALITY_HEVC" auto --help 2>&1)
    
    # 检查关键参数（drag_and_drop_processor.sh 使用的）
    for flag in "--output" "--recursive" "--in-place" "--explore" "--match-quality" "--compress" "--apple-compat" "--ultimate" "--verbose"; do
        if echo "$HELP" | grep -q -- "$flag"; then
            pass "参数 $flag 存在"
        else
            fail "参数 $flag 缺失"
        fi
    done
else
    fail "帮助信息异常"
fi
echo ""

log_test "vidquality-hevc auto --help"
if "$VIDQUALITY_HEVC" auto --help 2>&1 | grep -q "Usage:"; then
    pass "帮助信息正常"
else
    fail "帮助信息异常"
fi
echo ""

# Phase 3: 使用真实测试文件
echo -e "${BLUE}━━━ Phase 3: 真实文件测试 ━━━${NC}"
echo ""

TEST_MEDIA="$PROJECT_ROOT/test_media"
if [ ! -d "$TEST_MEDIA" ]; then
    echo -e "${YELLOW}⚠️  test_media 目录不存在，跳过真实文件测试${NC}"
else
    # 复制测试文件到临时目录
    cp -r "$TEST_MEDIA" "$TEST_DIR/test_media"
    
    log_test "测试图片转换 (基本模式)"
    if [ -f "$TEST_DIR/test_media/test_image.png" ]; then
        OUTPUT_DIR="$TEST_DIR/output1"
        mkdir -p "$OUTPUT_DIR"
        
        if "$IMGQUALITY_HEVC" auto "$TEST_DIR/test_media/test_image.png" --output "$OUTPUT_DIR" --verbose 2>&1 | grep -qE "(Processing|Converted|Skipped|Copied|⏭️)"; then
            pass "基本转换成功"
        else
            fail "基本转换失败"
        fi
    fi
    echo ""
    
    log_test "测试图片转换 (递归模式)"
    OUTPUT_DIR="$TEST_DIR/output2"
    mkdir -p "$OUTPUT_DIR"
    
    if "$IMGQUALITY_HEVC" auto --recursive "$TEST_DIR/test_media" --output "$OUTPUT_DIR" --verbose 2>&1 | grep -qE "(Processing|Complete|Finished|Skipped|⏭️|🔄)"; then
        pass "递归模式成功"
    else
        fail "递归模式失败"
    fi
    echo ""
    
    log_test "测试图片转换 (drag_and_drop 参数组合)"
    OUTPUT_DIR="$TEST_DIR/output3"
    mkdir -p "$OUTPUT_DIR"
    
    # 使用与 drag_and_drop_processor.sh 相同的参数
    if "$IMGQUALITY_HEVC" auto --explore --match-quality --compress --apple-compat --recursive --ultimate "$TEST_DIR/test_media" --output "$OUTPUT_DIR" --verbose 2>&1 | grep -qE "(Processing|Complete|Finished|Skipped|⏭️|🔄)"; then
        pass "drag_and_drop 参数组合成功"
    else
        fail "drag_and_drop 参数组合失败"
    fi
    echo ""
    
    log_test "测试视频转换 (基本模式)"
    if [ -f "$TEST_DIR/test_media/short_test.mp4" ]; then
        OUTPUT_DIR="$TEST_DIR/output4"
        mkdir -p "$OUTPUT_DIR"
        
        if timeout 60s "$VIDQUALITY_HEVC" auto "$TEST_DIR/test_media/short_test.mp4" --output "$OUTPUT_DIR" --verbose 2>&1 | grep -qE "(Processing|Converted|Skipped|Copied|⏭️|🔄)"; then
            pass "视频转换成功"
        else
            fail "视频转换失败或超时"
        fi
    fi
    echo ""
fi

# Phase 4: 输出格式验证
echo -e "${BLUE}━━━ Phase 4: 输出格式验证 ━━━${NC}"
echo ""

log_test "检查输出消息格式"
if [ -f "$TEST_DIR/test_media/test_image.png" ]; then
    OUTPUT=$("$IMGQUALITY_HEVC" auto "$TEST_DIR/test_media/test_image.png" --output "$TEST_DIR/output_format" --verbose 2>&1)
    
    if echo "$OUTPUT" | grep -qE "(Processing|Converted|Skipped|Copied|⏭️|🔄)"; then
        pass "输出包含预期状态消息"
    else
        fail "输出格式可能改变"
    fi
    
    if echo "$OUTPUT" | grep -qiE "(error|fatal|panic)" && ! echo "$OUTPUT" | grep -q "No such file"; then
        fail "检测到错误输出"
    else
        pass "无异常错误"
    fi
fi
echo ""

# Phase 5: 错误处理
echo -e "${BLUE}━━━ Phase 5: 错误处理验证 ━━━${NC}"
echo ""

log_test "不存在的文件处理"
if "$IMGQUALITY_HEVC" auto "/nonexistent/file.png" 2>&1 | grep -qiE "(not found|does not exist|no such file|Error:)"; then
    pass "正确报告文件不存在"
else
    fail "未正确处理不存在的文件"
fi

log_test "无效参数处理"
if "$IMGQUALITY_HEVC" auto --invalid-flag 2>&1 | grep -qiE "(error|invalid|unknown|unexpected)"; then
    pass "正确报告无效参数"
else
    fail "未正确处理无效参数"
fi
echo ""

# 总结
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo -e "${BLUE}📊 测试总结${NC}"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""
echo "总测试数: $TOTAL"
echo -e "${GREEN}通过: $PASSED${NC}"
echo -e "${RED}失败: $FAILED${NC}"
echo ""

PASS_RATE=$((PASSED * 100 / TOTAL))
echo "通过率: ${PASS_RATE}%"
echo ""

if [ "$FAILED" -eq 0 ]; then
    echo -e "${GREEN}✅ 所有测试通过！向后兼容性良好。${NC}"
    exit 0
elif [ "$PASS_RATE" -ge 80 ]; then
    echo -e "${YELLOW}⚠️  大部分测试通过，但存在 $FAILED 个失败。${NC}"
    exit 1
else
    echo -e "${RED}❌ 测试失败过多，存在兼容性问题。${NC}"
    exit 2
fi
