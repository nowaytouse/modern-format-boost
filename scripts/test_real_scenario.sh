#!/usr/bin/env bash
# 测试真实场景：使用双击脚本的参数
# 🔥 使用副本测试，避免破坏原始数据

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
BINARY="$PROJECT_ROOT/target/release/imgquality-hevc"

# 颜色
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

echo -e "${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo -e "${BLUE}🧪 Real Scenario Test (Drag & Drop Script Parameters)${NC}"
echo -e "${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo ""

# 检查二进制
if [ ! -f "$BINARY" ]; then
    echo -e "${RED}❌ Binary not found: $BINARY${NC}"
    exit 1
fi

# 创建测试目录
TEST_BASE="/tmp/real_scenario_test_$$"
mkdir -p "$TEST_BASE"
echo -e "${GREEN}✓${NC} Created test directory: $TEST_BASE"

# 清理函数
cleanup() {
    if [ -d "$TEST_BASE" ]; then
        rm -rf "$TEST_BASE"
        echo -e "${GREEN}✓${NC} Cleaned up test directory"
    fi
}
trap cleanup EXIT

# 创建测试源目录
echo ""
echo -e "${YELLOW}📁 Creating test source directory...${NC}"
mkdir -p "$TEST_BASE/test_source/sub1/sub2"

# 创建测试文件
echo "test" > "$TEST_BASE/test_source/test.txt"
echo "test" > "$TEST_BASE/test_source/sub1/test.txt"

# 设置过去的时间戳
touch -t 202001010000 "$TEST_BASE/test_source"
touch -t 202002020000 "$TEST_BASE/test_source/sub1"
touch -t 202003030000 "$TEST_BASE/test_source/sub1/sub2"

echo -e "${GREEN}✓${NC} Created test structure with old timestamps"

# 显示源目录时间戳
echo ""
echo -e "${BLUE}=== Source Directory Timestamps ===${NC}"
ls -ld "$TEST_BASE/test_source"
ls -ld "$TEST_BASE/test_source/sub1"
ls -ld "$TEST_BASE/test_source/sub1/sub2"

# 模拟脚本的 create_directory_structure 函数（修复后的版本）
echo ""
echo -e "${YELLOW}🔧 Creating output structure (with timestamp preservation)...${NC}"

OUTPUT_DIR="$TEST_BASE/test_source_optimized"
mkdir -p "$OUTPUT_DIR"

# 🔥 立即复制根目录时间戳
touch -r "$TEST_BASE/test_source" "$OUTPUT_DIR"

# 递归创建并复制时间戳
find "$TEST_BASE/test_source" -type d | while read -r dir; do
    rel="${dir#$TEST_BASE/test_source}"
    rel="${rel#/}"
    if [ -n "$rel" ]; then
        mkdir -p "$OUTPUT_DIR/$rel"
        touch -r "$dir" "$OUTPUT_DIR/$rel"
    fi
done

echo -e "${GREEN}✓${NC} Output structure created"

# 运行工具（模拟处理）
echo ""
echo -e "${YELLOW}🔄 Running tool (simulated)...${NC}"
"$BINARY" auto --explore --match-quality --compress --apple-compat --recursive \
    "$TEST_BASE/test_source" --output "$OUTPUT_DIR" 2>&1 | grep -E "(DEBUG|Preserving|preserved|Processing|Setting times)"

# 显示输出目录时间戳
echo ""
echo -e "${BLUE}=== Output Directory Timestamps ===${NC}"
ls -ld "$OUTPUT_DIR"
if [ -d "$OUTPUT_DIR/sub1" ]; then
    ls -ld "$OUTPUT_DIR/sub1"
fi
if [ -d "$OUTPUT_DIR/sub1/sub2" ]; then
    ls -ld "$OUTPUT_DIR/sub1/sub2"
fi

# 比较时间戳
echo ""
echo -e "${BLUE}=== Timestamp Comparison ===${NC}"

SRC_TIME=$(stat -f "%Sm" -t "%Y-%m-%d %H:%M" "$TEST_BASE/test_source" 2>/dev/null)
DST_TIME=$(stat -f "%Sm" -t "%Y-%m-%d %H:%M" "$OUTPUT_DIR" 2>/dev/null)

echo "Source:      $SRC_TIME"
echo "Destination: $DST_TIME"

if [ "$SRC_TIME" = "$DST_TIME" ]; then
    echo -e "${GREEN}✅ PASS: Root directory timestamp preserved!${NC}"
else
    echo -e "${RED}❌ FAIL: Root directory timestamp NOT preserved!${NC}"
    echo -e "${YELLOW}Expected: $SRC_TIME${NC}"
    echo -e "${YELLOW}Got:      $DST_TIME${NC}"
fi

echo ""
echo -e "${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo -e "${BLUE}Test completed${NC}"
echo -e "${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
