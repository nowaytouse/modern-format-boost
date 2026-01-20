#!/usr/bin/env bash
# 🔥 向后兼容性测试脚本 v1.0
# 
# 测试目标：
# 1. 验证所有二进制程序的命令行接口未改变
# 2. 验证输出格式保持一致
# 3. 验证现有工作流程正常运行
#
# 测试范围：
# - imgquality-hevc
# - imgquality-av1
# - vidquality-hevc
# - vidquality-av1

set -e

# 颜色定义
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
NC='\033[0m' # No Color

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

# 测试计数器
TOTAL_TESTS=0
PASSED_TESTS=0
FAILED_TESTS=0

# 测试结果数组
declare -a TEST_RESULTS

echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo -e "${BLUE}🔍 向后兼容性测试 - Modern Format Boost${NC}"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

# 辅助函数
log_test() {
    local test_name="$1"
    TOTAL_TESTS=$((TOTAL_TESTS + 1))
    echo -e "${CYAN}[Test $TOTAL_TESTS]${NC} $test_name"
}

pass_test() {
    local message="$1"
    PASSED_TESTS=$((PASSED_TESTS + 1))
    TEST_RESULTS+=("✅ PASS: $message")
    echo -e "  ${GREEN}✅ PASS${NC}: $message"
}

fail_test() {
    local message="$1"
    FAILED_TESTS=$((FAILED_TESTS + 1))
    TEST_RESULTS+=("❌ FAIL: $message")
    echo -e "  ${RED}❌ FAIL${NC}: $message"
}

warn_test() {
    local message="$1"
    echo -e "  ${YELLOW}⚠️  WARN${NC}: $message"
}

# 1. 检查二进制文件存在
echo -e "${BLUE}━━━ Phase 1: 二进制文件检查 ━━━${NC}"
echo ""

BINARIES=(
    "imgquality-hevc"
    "imgquality-av1"
    "vidquality-hevc"
    "vidquality-av1"
)

for binary in "${BINARIES[@]}"; do
    log_test "检查 $binary 是否存在"
    
    BINARY_PATH="$PROJECT_ROOT/target/release/$binary"
    if [ -f "$BINARY_PATH" ]; then
        pass_test "$binary 存在于 $BINARY_PATH"
    else
        fail_test "$binary 不存在，尝试构建..."
        echo -e "  ${YELLOW}正在构建 $binary...${NC}"
        cd "$PROJECT_ROOT"
        if cargo build --release --package "${binary//-/_}" 2>&1 | grep -q "Finished"; then
            pass_test "$binary 构建成功"
        else
            fail_test "$binary 构建失败"
        fi
    fi
    echo ""
done

# 2. 测试命令行接口 - 帮助信息
echo -e "${BLUE}━━━ Phase 2: 命令行接口测试 ━━━${NC}"
echo ""

for binary in "${BINARIES[@]}"; do
    BINARY_PATH="$PROJECT_ROOT/target/release/$binary"
    [ ! -f "$BINARY_PATH" ] && continue
    
    log_test "$binary --help 输出"
    
    # 测试 --help 参数
    if "$BINARY_PATH" --help 2>&1 | grep -q "Usage:"; then
        pass_test "--help 参数正常工作"
    else
        fail_test "--help 参数不工作或输出格式改变"
    fi
    
    # 检查关键参数是否存在
    HELP_OUTPUT=$("$BINARY_PATH" --help 2>&1)
    
    EXPECTED_FLAGS=(
        "--output"
        "--force"
        "--recursive"
        "--delete-original"
        "--in-place"
        "--explore"
        "--match-quality"
        "--compress"
        "--apple-compat"
    )
    
    for flag in "${EXPECTED_FLAGS[@]}"; do
        if echo "$HELP_OUTPUT" | grep -q -- "$flag"; then
            pass_test "参数 $flag 存在"
        else
            fail_test "参数 $flag 缺失或名称改变"
        fi
    done
    
    echo ""
done

# 3. 测试基本功能 - 使用测试文件
echo -e "${BLUE}━━━ Phase 3: 基本功能测试 ━━━${NC}"
echo ""

# 创建临时测试目录
TEST_DIR="/tmp/backward_compat_test_$$"
mkdir -p "$TEST_DIR"

cleanup() {
    if [ -d "$TEST_DIR" ]; then
        rm -rf "$TEST_DIR"
        echo -e "${GREEN}✓${NC} 清理测试目录"
    fi
}
trap cleanup EXIT

# 创建测试文件
log_test "创建测试文件"

# 创建一个简单的PNG图片（1x1像素）
echo -e "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==" | base64 -d > "$TEST_DIR/test.png"

if [ -f "$TEST_DIR/test.png" ]; then
    pass_test "测试PNG文件创建成功"
else
    fail_test "测试PNG文件创建失败"
fi
echo ""

# 测试 imgquality-hevc 基本转换
log_test "imgquality-hevc 基本转换功能"

BINARY_PATH="$PROJECT_ROOT/target/release/imgquality-hevc"
if [ -f "$BINARY_PATH" ]; then
    OUTPUT_DIR="$TEST_DIR/output_hevc"
    mkdir -p "$OUTPUT_DIR"
    
    # 使用最简单的参数
    if "$BINARY_PATH" auto "$TEST_DIR/test.png" --output "$OUTPUT_DIR" 2>&1 | grep -qE "(Processing|Converted|Skipped|Copied)"; then
        pass_test "基本转换命令执行成功"
        
        # 检查输出文件
        if [ -f "$OUTPUT_DIR/test.jxl" ] || [ -f "$OUTPUT_DIR/test.png" ]; then
            pass_test "输出文件生成成功"
        else
            warn_test "输出文件未找到（可能被跳过）"
        fi
    else
        fail_test "基本转换命令执行失败"
    fi
else
    warn_test "imgquality-hevc 二进制不存在，跳过测试"
fi
echo ""

# 4. 测试输出格式
echo -e "${BLUE}━━━ Phase 4: 输出格式测试 ━━━${NC}"
echo ""

log_test "检查输出消息格式"

BINARY_PATH="$PROJECT_ROOT/target/release/imgquality-hevc"
if [ -f "$BINARY_PATH" ]; then
    OUTPUT=$("$BINARY_PATH" auto "$TEST_DIR/test.png" --output "$TEST_DIR/output_format" 2>&1)
    
    # 检查关键输出模式
    if echo "$OUTPUT" | grep -qE "(Processing|Converted|Skipped|Copied|✅|❌)"; then
        pass_test "输出包含预期的状态消息"
    else
        fail_test "输出格式可能已改变"
    fi
    
    # 检查是否有错误输出到stderr
    if echo "$OUTPUT" | grep -qE "(ERROR|FATAL|panic)"; then
        fail_test "检测到错误输出"
    else
        pass_test "无错误输出"
    fi
else
    warn_test "imgquality-hevc 二进制不存在，跳过测试"
fi
echo ""

# 5. 测试工作流程兼容性
echo -e "${BLUE}━━━ Phase 5: 工作流程兼容性测试 ━━━${NC}"
echo ""

log_test "测试典型工作流程 1: 基本转换"

BINARY_PATH="$PROJECT_ROOT/target/release/imgquality-hevc"
if [ -f "$BINARY_PATH" ]; then
    WORKFLOW_DIR="$TEST_DIR/workflow1"
    mkdir -p "$WORKFLOW_DIR/input"
    cp "$TEST_DIR/test.png" "$WORKFLOW_DIR/input/"
    
    # 典型工作流程：递归转换到输出目录
    if "$BINARY_PATH" auto --recursive "$WORKFLOW_DIR/input" --output "$WORKFLOW_DIR/output" 2>&1 | grep -qE "(Processing|Complete|Finished)"; then
        pass_test "工作流程 1 执行成功"
    else
        fail_test "工作流程 1 执行失败"
    fi
else
    warn_test "跳过工作流程测试"
fi
echo ""

log_test "测试典型工作流程 2: 探索模式"

if [ -f "$BINARY_PATH" ]; then
    WORKFLOW_DIR="$TEST_DIR/workflow2"
    mkdir -p "$WORKFLOW_DIR/input"
    cp "$TEST_DIR/test.png" "$WORKFLOW_DIR/input/"
    
    # 探索模式工作流程
    if "$BINARY_PATH" auto --explore --match-quality "$WORKFLOW_DIR/input/test.png" --output "$WORKFLOW_DIR/output" 2>&1 | grep -qE "(Processing|Exploring|Complete)"; then
        pass_test "工作流程 2 (探索模式) 执行成功"
    else
        fail_test "工作流程 2 (探索模式) 执行失败"
    fi
else
    warn_test "跳过工作流程测试"
fi
echo ""

log_test "测试典型工作流程 3: 压缩模式"

if [ -f "$BINARY_PATH" ]; then
    WORKFLOW_DIR="$TEST_DIR/workflow3"
    mkdir -p "$WORKFLOW_DIR/input"
    cp "$TEST_DIR/test.png" "$WORKFLOW_DIR/input/"
    
    # 压缩模式工作流程
    if "$BINARY_PATH" auto --compress "$WORKFLOW_DIR/input/test.png" --output "$WORKFLOW_DIR/output" 2>&1 | grep -qE "(Processing|Compress|Complete)"; then
        pass_test "工作流程 3 (压缩模式) 执行成功"
    else
        fail_test "工作流程 3 (压缩模式) 执行失败"
    fi
else
    warn_test "跳过工作流程测试"
fi
echo ""

# 6. 测试参数组合兼容性
echo -e "${BLUE}━━━ Phase 6: 参数组合兼容性测试 ━━━${NC}"
echo ""

PARAM_COMBINATIONS=(
    "auto"
    "auto --compress"
    "auto --explore"
    "auto --match-quality"
    "auto --explore --match-quality"
    "auto --explore --match-quality --compress"
    "auto --explore --match-quality --compress --ultimate"
)

for params in "${PARAM_COMBINATIONS[@]}"; do
    log_test "参数组合: $params"
    
    BINARY_PATH="$PROJECT_ROOT/target/release/imgquality-hevc"
    if [ -f "$BINARY_PATH" ]; then
        COMBO_DIR="$TEST_DIR/combo_$(echo "$params" | tr ' ' '_')"
        mkdir -p "$COMBO_DIR"
        cp "$TEST_DIR/test.png" "$COMBO_DIR/"
        
        # 执行命令（添加超时保护）
        if timeout 30s "$BINARY_PATH" $params "$COMBO_DIR/test.png" --output "$COMBO_DIR/output" 2>&1 | grep -qE "(Processing|Complete|Skipped)"; then
            pass_test "参数组合有效"
        else
            fail_test "参数组合失败或超时"
        fi
    else
        warn_test "跳过参数组合测试"
    fi
    echo ""
done

# 7. 测试错误处理兼容性
echo -e "${BLUE}━━━ Phase 7: 错误处理兼容性测试 ━━━${NC}"
echo ""

log_test "测试无效输入处理"

BINARY_PATH="$PROJECT_ROOT/target/release/imgquality-hevc"
if [ -f "$BINARY_PATH" ]; then
    # 测试不存在的文件
    if "$BINARY_PATH" auto "/nonexistent/file.png" 2>&1 | grep -qE "(not found|does not exist|No such file|ERROR)"; then
        pass_test "正确处理不存在的文件"
    else
        fail_test "未正确报告文件不存在错误"
    fi
    
    # 测试无效参数
    if "$BINARY_PATH" --invalid-flag 2>&1 | grep -qE "(error|invalid|unknown|unrecognized)"; then
        pass_test "正确处理无效参数"
    else
        fail_test "未正确报告无效参数错误"
    fi
else
    warn_test "跳过错误处理测试"
fi
echo ""

# 8. 生成测试报告
echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo -e "${BLUE}📊 测试报告${NC}"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

echo -e "${CYAN}测试统计:${NC}"
echo "  总测试数: $TOTAL_TESTS"
echo -e "  ${GREEN}通过: $PASSED_TESTS${NC}"
echo -e "  ${RED}失败: $FAILED_TESTS${NC}"
echo ""

PASS_RATE=$((PASSED_TESTS * 100 / TOTAL_TESTS))
echo -e "通过率: ${PASS_RATE}%"
echo ""

# 显示所有测试结果
echo -e "${CYAN}详细结果:${NC}"
for result in "${TEST_RESULTS[@]}"; do
    echo "  $result"
done
echo ""

# 生成报告文件
REPORT_FILE="$PROJECT_ROOT/backward_compatibility_report_$(date +%Y%m%d_%H%M%S).txt"
cat > "$REPORT_FILE" <<EOF
向后兼容性测试报告
==================

测试时间: $(date "+%Y-%m-%d %H:%M:%S")
项目路径: $PROJECT_ROOT

测试统计
--------
总测试数: $TOTAL_TESTS
通过: $PASSED_TESTS
失败: $FAILED_TESTS
通过率: ${PASS_RATE}%

详细结果
--------
$(printf '%s\n' "${TEST_RESULTS[@]}")

测试范围
--------
1. 二进制文件存在性检查
2. 命令行接口完整性检查
3. 基本功能测试
4. 输出格式验证
5. 工作流程兼容性测试
6. 参数组合兼容性测试
7. 错误处理兼容性测试

结论
----
$(if [ "$FAILED_TESTS" -eq 0 ]; then
    echo "✅ 所有测试通过，向后兼容性良好"
elif [ "$PASS_RATE" -ge 80 ]; then
    echo "⚠️  大部分测试通过，但存在一些兼容性问题"
else
    echo "❌ 多个测试失败，存在严重的兼容性问题"
fi)

EOF

echo -e "${GREEN}✓${NC} 测试报告已保存: $REPORT_FILE"
echo ""

# 最终结论
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
if [ "$FAILED_TESTS" -eq 0 ]; then
    echo -e "${GREEN}✅ 向后兼容性测试通过！${NC}"
    echo -e "${GREEN}所有功能和接口保持兼容。${NC}"
    EXIT_CODE=0
elif [ "$PASS_RATE" -ge 80 ]; then
    echo -e "${YELLOW}⚠️  向后兼容性测试部分通过${NC}"
    echo -e "${YELLOW}存在 $FAILED_TESTS 个失败的测试，请检查详细报告。${NC}"
    EXIT_CODE=1
else
    echo -e "${RED}❌ 向后兼容性测试失败${NC}"
    echo -e "${RED}存在严重的兼容性问题，请立即修复。${NC}"
    EXIT_CODE=2
fi
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

exit $EXIT_CODE
