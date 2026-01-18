#!/usr/bin/env bash
# 测试目录时间戳保留功能
# 🔥 使用副本进行测试，避免污染原始文件

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
BINARY="$PROJECT_ROOT/target/release/imgquality-hevc"

# 颜色定义
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

echo -e "${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo -e "${BLUE}🧪 Directory Timestamp Preservation Test${NC}"
echo -e "${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo ""

# 检查二进制文件
if [ ! -f "$BINARY" ]; then
    echo -e "${RED}❌ Binary not found: $BINARY${NC}"
    echo -e "${YELLOW}Please run: ./scripts/smart_build.sh${NC}"
    exit 1
fi

# 创建测试目录
TEST_BASE="/tmp/dir_timestamp_test_$$"
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

# 创建测试源目录结构
echo ""
echo -e "${YELLOW}📁 Creating test source directory...${NC}"
mkdir -p "$TEST_BASE/source/sub1/sub2"

# 从实际文件复制一个测试文件（如果存在）
SOURCE_FILE="/Users/user/Downloads/all/1/参考/内容 猎奇/4h8uh4vkss9clo2wfiy30kach.gif"
if [ -f "$SOURCE_FILE" ]; then
    # 只复制前100KB作为测试
    dd if="$SOURCE_FILE" of="$TEST_BASE/source/test.gif" bs=1024 count=100 2>/dev/null
    echo -e "${GREEN}✓${NC} Created test file (100KB sample)"
else
    # 创建一个简单的测试文件
    echo "test" > "$TEST_BASE/source/test.txt"
    echo -e "${YELLOW}⚠${NC}  Using dummy test file (original not found)"
fi

# 设置目录时间戳为过去的时间
echo ""
echo -e "${YELLOW}⏰ Setting directory timestamps...${NC}"
touch -t 202001010000 "$TEST_BASE/source"
touch -t 202002020000 "$TEST_BASE/source/sub1"
touch -t 202003030000 "$TEST_BASE/source/sub1/sub2"

# 显示源目录时间戳
echo ""
echo -e "${BLUE}=== Source Directory Timestamps ===${NC}"
ls -ld "$TEST_BASE/source"
ls -ld "$TEST_BASE/source/sub1"
ls -ld "$TEST_BASE/source/sub1/sub2"

# 运行转换
echo ""
echo -e "${YELLOW}🔄 Running conversion...${NC}"
cd "$TEST_BASE"

# 检查输入是否是目录
if [ -d "source" ]; then
    echo -e "${GREEN}✓${NC} Input is a directory"
else
    echo -e "${RED}❌${NC} Input is not a directory!"
fi

# 捕获所有输出到文件
"$BINARY" auto -o source_optimized source > /tmp/conversion_output_$$.log 2>&1

# 显示相关输出
echo ""
echo -e "${BLUE}=== Conversion Output (filtered) ===${NC}"
grep -E "(DEBUG|Preserving|preserved|Processed|Files Processed)" /tmp/conversion_output_$$.log || echo "(No matching output)"

# 检查调试文件
echo ""
echo -e "${BLUE}=== Debug Files ===${NC}"
if [ -f "/tmp/debug_function_entry.log" ]; then
    echo -e "${GREEN}✓${NC} Function entry log found:"
    cat /tmp/debug_function_entry.log
else
    echo -e "${RED}❌${NC} Function entry log NOT found (function not called?)"
fi

if [ -f "/tmp/debug_base_dir.log" ]; then
    echo -e "${GREEN}✓${NC} Base dir log found:"
    cat /tmp/debug_base_dir.log
else
    echo -e "${YELLOW}⚠${NC}  Base dir log not found"
fi

if [ -f "/tmp/debug_metadata.log" ]; then
    echo -e "${GREEN}✓${NC} Metadata log found:"
    cat /tmp/debug_metadata.log
else
    echo -e "${YELLOW}⚠${NC}  Metadata log not found"
fi

# 检查输出目录
echo ""
if [ -d "$TEST_BASE/source_optimized" ]; then
    echo -e "${GREEN}✓${NC} Output directory created"
    
    echo ""
    echo -e "${BLUE}=== Output Directory Timestamps ===${NC}"
    ls -ld "$TEST_BASE/source_optimized"
    
    if [ -d "$TEST_BASE/source_optimized/sub1" ]; then
        ls -ld "$TEST_BASE/source_optimized/sub1"
    fi
    
    if [ -d "$TEST_BASE/source_optimized/sub1/sub2" ]; then
        ls -ld "$TEST_BASE/source_optimized/sub1/sub2"
    fi
    
    # 比较时间戳
    echo ""
    echo -e "${BLUE}=== Timestamp Comparison ===${NC}"
    
    SRC_TIME=$(stat -f "%Sm" -t "%Y-%m-%d %H:%M" "$TEST_BASE/source" 2>/dev/null || stat -c "%y" "$TEST_BASE/source" 2>/dev/null | cut -d. -f1)
    DST_TIME=$(stat -f "%Sm" -t "%Y-%m-%d %H:%M" "$TEST_BASE/source_optimized" 2>/dev/null || stat -c "%y" "$TEST_BASE/source_optimized" 2>/dev/null | cut -d. -f1)
    
    echo "Source:      $SRC_TIME"
    echo "Destination: $DST_TIME"
    
    if [ "$SRC_TIME" = "$DST_TIME" ]; then
        echo -e "${GREEN}✅ PASS: Root directory timestamp preserved!${NC}"
    else
        echo -e "${RED}❌ FAIL: Root directory timestamp NOT preserved!${NC}"
        echo -e "${YELLOW}Expected: $SRC_TIME${NC}"
        echo -e "${YELLOW}Got:      $DST_TIME${NC}"
    fi
    
else
    echo -e "${RED}❌ Output directory not created${NC}"
    echo ""
    echo -e "${YELLOW}Full conversion output:${NC}"
    cat /tmp/conversion_output_$$.log
fi

# 清理临时日志
rm -f /tmp/conversion_output_$$.log

echo ""
echo -e "${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo -e "${BLUE}Test completed${NC}"
echo -e "${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
