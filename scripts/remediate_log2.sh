#!/bin/bash

# ═══════════════════════════════════════════════════════════════
# 🔥 LOG 2 Targeted Remediation Script v1.0
# ═══════════════════════════════════════════════════════════════
# 
# Purpose:
#   Fixes failures identified in LOG 2 (mainly .jpe errors).
#   Uses production-grade parameters from drag_and_drop_processor.sh.
#
# Target Files (17 total, HEIC excluded):
#   - List exported to: /Users/nyamiiko/Downloads/GitHub/final_fix_log2.list
#
# Parameters:
#   auto --explore --match-quality --compress --apple-compat --allow-size-tolerance --ultimate --force --verbose

set -e

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
RESET='\033[0m'
BOLD='\033[1m'

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
# LOG 2 processed into this list
FIX_LIST="/Users/nyamiiko/Downloads/GitHub/final_fix_log2.list"
# Production target from LOG 2
DEFAULT_TARGET_BASE="/Users/nyamiiko/Downloads/all/1"
DEFAULT_OUTPUT_BASE="/Users/nyamiiko/Downloads/all/1_optimized"

# Binary Check
BIN_PATH="$PROJECT_ROOT/target/release/imgquality-hevc"

echo -e "${CYAN}🚀 Starting Targeted Remediation for LOG 2...${RESET}"

# Ensure list exists
if [[ ! -f "$FIX_LIST" ]]; then
    echo -e "${RED}❌ Error: Fix list not found at $FIX_LIST${RESET}"
    exit 1
fi

# Ensure binary is optimized and up-to-date
echo -e "${YELLOW}⚙️  Building imgquality-hevc (release) to ensure latest fixes...${RESET}"
(cd "$PROJECT_ROOT" && cargo build --release -p imgquality-hevc)

# Counts
COUNT=$(wc -l < "$FIX_LIST" | tr -d ' ')
echo -e "📦 Found ${BOLD}$COUNT${RESET} files to remediate."
echo ""

SUCCESS_COUNT=0
FAILED_COUNT=0

# Loop through the list
IFS=$'\n'
for FILE in $(cat "$FIX_LIST"); do
    if [[ ! -f "$FILE" ]]; then
        echo -e "${YELLOW}⚠️  File not found (already moved or deleted?):${RESET} $FILE"
        continue
    fi
    
    REL_PATH=$(echo "$FILE" | sed "s|^$DEFAULT_TARGET_BASE/||")
    echo -ne "✨ Remediating: ${CYAN}$REL_PATH${RESET}... "
    
    # Execute with production parameters
    # --base-dir is crucial for directory structure preservation
    # --output is the optimized folder
    # --force to ensure we overwrite failures
    # --ultimate for saturation search
    "$BIN_PATH" auto --output "$DEFAULT_OUTPUT_BASE" \
                    --base-dir "$DEFAULT_TARGET_BASE" \
                    --force --verbose \
                    --explore --match-quality --compress --apple-compat --allow-size-tolerance --ultimate \
                    "$FILE" > /dev/null 2>&1
                    
    # Verification
    # Expected extension is .jxl for these image files
    EXPECTED_OUTPUT="$DEFAULT_OUTPUT_BASE/${REL_PATH%.*}.jxl"
    
    if [[ -f "$EXPECTED_OUTPUT" ]]; then
        echo -e "${GREEN}DONE${RESET}"
        SUCCESS_COUNT=$((SUCCESS_COUNT + 1))
    else
        echo -e "${RED}FAILED${RESET}"
        FAILED_COUNT=$((FAILED_COUNT + 1))
    fi
done

echo ""
echo -e "${GREEN}🎉 Targeted Remediation Complete!${RESET}"
echo -e "   - Successfully remediated: ${BOLD}$SUCCESS_COUNT / $COUNT${RESET}"
[[ $FAILED_COUNT -gt 0 ]] && echo -e "   - Failed:                  ${RED}${BOLD}$FAILED_COUNT${RESET}"
echo ""
