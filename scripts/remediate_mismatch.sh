#!/bin/bash

# ═══════════════════════════════════════════════════════════════
# 🔥 PNG/JPEG Mismatch Remediation Patch v1.0
# ═══════════════════════════════════════════════════════════════
# 
# Purpose:
#   Fixes .png files that are actually JPEG data and were incorrectly 
#   handled (copied instead of converted) due to extension mismatch.
#   1. Detects real content type using 'file' command.
#   2. Removes the erroneous .png copy from the _optimized directory.
#   3. Re-processes the file into JXL using fixed tools.
#   4. Maintains original structure and metadata.
#
# Usage:
#   ./remediate_mismatch.sh <target_directory> [--av1]
#
# Options:
#   --av1    Use imgquality-av1 instead of imgquality-hevc

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

# Default tool
TOOL_NAME="imgquality-hevc"
BIN_PATH="$PROJECT_ROOT/target/debug/imgquality-hevc"

show_help() {
    echo -e "${BOLD}Usage:${RESET}"
    echo -e "  $0 <target_directory> [--av1]     Scan directory and fix mismatches"
    echo ""
}

if [[ $# -lt 1 ]]; then
    show_help
    exit 1
fi

TARGET_DIR=""
USE_AV1=false

for arg in "$@"; do
    if [[ "$arg" == "--av1" ]]; then
        USE_AV1=true
        TOOL_NAME="imgquality-av1"
        BIN_PATH="$PROJECT_ROOT/target/debug/imgquality-av1"
    else
        TARGET_DIR="$arg"
    fi
done

if [[ ! -d "$TARGET_DIR" ]]; then
    echo -e "${RED}❌ Error: Target directory does not exist: $TARGET_DIR${RESET}"
    exit 1
fi

# Ensure binary is up to date
echo -e "${YELLOW}⚙️  Building $TOOL_NAME to ensure latest fixes...${RESET}"
(cd "$PROJECT_ROOT" && cargo build -p "$TOOL_NAME")

echo -e "${CYAN}🚀 Starting PNG/JPEG Mismatch Remediation...${RESET}"
echo -e "${CYAN}📂 Target: ${BOLD}$TARGET_DIR${RESET}"
echo -e "${CYAN}🛠️  Tool:   ${BOLD}$TOOL_NAME${RESET}"

# Find _optimized directory
OPTIMIZED_DIR="${TARGET_DIR}_optimized"
if [[ ! -d "$OPTIMIZED_DIR" ]]; then
    echo -e "${RED}❌ Error: Optimized directory not found: $OPTIMIZED_DIR${RESET}"
    exit 1
fi

# Detect mismatched files
echo -e "${YELLOW}🔍 Scanning for mismatched .png files...${RESET}"
# Find all .png files and check content with 'file' command
MISMATCHED_FILES=$(find "$TARGET_DIR" -type f -name "*.png" -exec file {} + | grep "JPEG image data" | cut -d: -f1)

COUNT=$(echo "$MISMATCHED_FILES" | grep -v "^$" | wc -l | tr -d ' ')
if [[ $COUNT -eq 0 ]]; then
    echo -e "${GREEN}✅ No mismatched .png files found.${RESET}"
    exit 0
fi

echo -e "📦 Found ${BOLD}$COUNT${RESET} mismatched files to re-process."
echo ""

SUCCESS_COUNT=0
CLEANUP_COUNT=0

# Use IFS to handle spaces in filenames
IFS=$'\n'
for FILE in $MISMATCHED_FILES; do
    if [[ ! -f "$FILE" ]]; then continue; fi
    
    # calculation of relative path
    REL_PATH=$(echo "$FILE" | sed "s|^$TARGET_DIR/||")
    
    # Check if erroneous copy exists in _optimized
    OPT_FILE_RAW="$OPTIMIZED_DIR/$REL_PATH"
    
    if [[ -f "$OPT_FILE_RAW" ]]; then
        # If it's still a .png in _optimized, it's an erroneous copy
        if [[ "$OPT_FILE_RAW" == *.png ]]; then
            echo -e "${YELLOW}🧹 Cleaning up erroneous copy:${RESET} $REL_PATH"
            rm "$OPT_FILE_RAW"
            CLEANUP_COUNT=$((CLEANUP_COUNT + 1))
        fi
    fi
    
    # Re-process with fixed tool
    echo -ne "✨ Re-processing: ${CYAN}$REL_PATH${RESET}... "
    
    # v7.9.6 Spec: --ultimate MUST be used with --explore --match-quality --compress
    ARGS="auto \"$FILE\" --output \"$OPTIMIZED_DIR\" --base-dir \"$TARGET_DIR\" --force --verbose"
    if [[ "$USE_AV1" == false ]]; then
        ARGS="$ARGS --explore --match-quality --compress --ultimate"
    else
        ARGS="$ARGS --explore --match-quality --compress"
    fi

    eval "$BIN_PATH" $ARGS > /dev/null 2>&1
        
    # Verify success (should be converted to .jxl)
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
