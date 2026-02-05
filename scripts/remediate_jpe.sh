#!/bin/bash

# ═══════════════════════════════════════════════════════════════
# 🔥 .jpe Remediation Patch v1.0
# ═══════════════════════════════════════════════════════════════
# 
# Purpose:
#   Fixes .jpe files that were incorrectly handled due to extension issues.
#   1. Removes the raw copy from the _optimized directory.
#   2. Re-projects the file into the _optimized directory using correct JXL conversion.
#   3. Maintains original structure and metadata.
#
# Usage:
#   ./remediate_jpe.sh <target_directory>
#   ./remediate_jpe.sh --list <file_with_paths> <target_directory>

set -e

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
RESET='\033[0m'
BOLD='\033[1m'

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
BIN_PATH="$PROJECT_ROOT/target/debug/imgquality-hevc"

show_help() {
    echo -e "${BOLD}Usage:${RESET}"
    echo -e "  $0 <target_directory>             Scan directory for .jpe files"
    echo -e "  $0 --list <file> <target_dir>     Process specific list of files"
    echo ""
}

if [[ $# -lt 1 ]]; then
    show_help
    exit 1
fi

TARGET_DIR=""
LIST_FILE=""

if [[ "$1" == "--list" ]]; then
    LIST_FILE="$2"
    TARGET_DIR="$3"
else
    TARGET_DIR="$1"
fi

if [[ ! -d "$TARGET_DIR" ]]; then
    echo -e "${RED}❌ Error: Target directory does not exist: $TARGET_DIR${RESET}"
    exit 1
fi

# 确保二进制文件存在
if [[ ! -f "$BIN_PATH" ]]; then
    echo -e "${YELLOW}⚙️  Building imgquality-hevc...${RESET}"
    (cd "$PROJECT_ROOT" && cargo build -p imgquality-hevc)
fi

echo -e "${CYAN}🚀 Starting .jpe Remediation Patch...${RESET}"
echo -e "${CYAN}📂 Target: ${BOLD}$TARGET_DIR${RESET}"
[[ -n "$LIST_FILE" ]] && echo -e "${CYAN}📋 List: ${BOLD}$LIST_FILE${RESET}"

# 寻找 _optimized 目录
OPTIMIZED_DIR="${TARGET_DIR}_optimized"
if [[ ! -d "$OPTIMIZED_DIR" ]]; then
    echo -e "${RED}❌ Error: Optimized directory not found: $OPTIMIZED_DIR${RESET}"
    echo -e "   Did you run the original process first?"
    exit 1
fi

# 获取文件列表
if [[ -n "$LIST_FILE" ]]; then
    FILES=$(cat "$LIST_FILE")
else
    # 递归搜索目标目录中的 .jpe 文件
    FILES=$(find "$TARGET_DIR" -type f -name "*.jpe")
fi

COUNT=$(echo "$FILES" | grep -v "^$" | wc -l | tr -d ' ')
if [[ $COUNT -eq 0 ]]; then
    echo -e "${GREEN}✅ No .jpe files found to remediate.${RESET}"
    exit 0
fi

echo -e "📦 Found ${BOLD}$COUNT${RESET} files to re-process."
echo ""

SUCCESS_COUNT=0
CLEANUP_COUNT=0

for FILE in $FILES; do
    if [[ ! -f "$FILE" ]]; then continue; fi
    
    # 1. 计算相对路径
    REL_PATH=$(echo "$FILE" | sed "s|^$TARGET_DIR/||")
    
    # 2. 检查 _optimized 目录中是否存在错误的原始副本
    OPT_FILE_RAW="$OPTIMIZED_DIR/$REL_PATH"
    
    if [[ -f "$OPT_FILE_RAW" ]]; then
        # 验证这是否真的是一个原始副本（而不是已经转换好的文件）
        # .jpe -> .jxl 才是正确结果，所以如果 _optimized 里还有 .jpe，那一定是错的
        if [[ "$OPT_FILE_RAW" == *.jpe ]]; then
            echo -e "${YELLOW}🧹 Cleaning up erroneous copy:${RESET} $REL_PATH"
            rm "$OPT_FILE_RAW"
            CLEANUP_COUNT=$((CLEANUP_COUNT + 1))
        fi
    fi
    
    # 3. 运行修复后的工具进行正确转换
    # 参数：
    # - --output $OPTIMIZED_DIR (指定输出根目录)
    # - --base-dir $TARGET_DIR (保持相对路径层级)
    # - --ultimate (保持原始任务的最高质量要求)
    echo -ne "✨ Re-processing: ${CYAN}$REL_PATH${RESET}... "
    
    # 🔥 调用修复后的工具
    # 使用 Auto 命令进行 JXL 转换
    # v7.9.6 Spec: --ultimate MUST be used with --explore --match-quality --compress
    "$BIN_PATH" auto "$FILE" \
        --output "$OPTIMIZED_DIR" \
        --base-dir "$TARGET_DIR" \
        --explore --match-quality --compress --ultimate \
        --force \
        --verbose > /dev/null 2>&1
        
    # 验证生成是否成功
    JXL_FILE="${OPT_FILE_RAW%.*}.jxl"
    if [[ -f "$JXL_FILE" ]]; then
        echo -e "${GREEN}SUCCESS${RESET}"
        SUCCESS_COUNT=$((SUCCESS_COUNT + 1))
    else
        echo -e "${RED}FAILED${RESET}"
    fi
done

echo ""
echo -e "${GREEN}🎉 Remediation Complete!${RESET}"
echo -e "   - Erroneous copies cleaned: ${BOLD}$CLEANUP_COUNT${RESET}"
echo -e "   - Correctly re-processed:   ${BOLD}$SUCCESS_COUNT / $COUNT${RESET}"
echo ""
