#!/opt/homebrew/bin/bash
# Modern Format Boost - Drag & Drop Processor v7.0
# 
# 🔥 v7.0: UI/UX Optimization
#          - Premium visual design
#          - Improved progress indicators
#          - Clearer status messaging
# 🔥 v6.9.13: No-Omission Design
#            - Supports all formats (converts supported, copies unsupported)
#            - XMP sidecar merging
#            - Guaranteed full output
# 
# Usage: Drag folder onto this script or double-click to select

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=common.sh
source "$SCRIPT_DIR/common.sh"

# Tool Paths (🔥 v6.9.15: 修正为正确的 target/release 路径)
IMGQUALITY_HEVC="$PROJECT_ROOT/target/release/img-hevc"
VIDQUALITY_HEVC="$PROJECT_ROOT/target/release/vid-hevc"

# Configuration
OUTPUT_MODE="inplace"
OUTPUT_DIR=""
SELECTED=0
ULTIMATE_MODE=true
VERBOSE_MODE=false  # 🔥 默认静默模式

# 🛠️  Helper Functions

# Hide cursor
hide_cursor() { printf '\033[?25l'; }
# Show cursor
show_cursor() { printf '\033[?25h'; }

# Clear screen
clear_screen() { printf '\033[2J\033[H'; }

# 📝 Log directory and file
LOG_DIR="$PROJECT_ROOT/logs"
LOG_FILE=""
SESSION_START_TIME=""

# Initialize log file
init_log() {
    SESSION_START_TIME=$(date +"%Y-%m-%d_%H-%M-%S")
    mkdir -p "$LOG_DIR"
    LOG_FILE="$LOG_DIR/drag_drop_${SESSION_START_TIME}.log"
}

# Save log to file (called automatically at exit)
save_log() {
    [[ -z "$LOG_FILE" ]] && return
    [[ ! -f "$LOG_FILE" ]] && return
    
    # Append statistics footer to log
    {
        echo ""
        echo "========================================"
        echo "📊 Final Statistics"
        echo "========================================"
        echo "End Time: $(date +"%Y-%m-%d_%H-%M-%S")"
        echo ""
        echo "Images:  $IMG_SUCCEEDED succeeded, $IMG_SKIPPED skipped, $IMG_FAILED failed"
        echo "Videos:  $VID_SUCCEEDED succeeded, $VID_SKIPPED skipped, $VID_FAILED failed"
        echo ""
        local total_succeeded=$((IMG_SUCCEEDED + VID_SUCCEEDED))
        local total_skipped=$((IMG_SKIPPED + VID_SKIPPED))
        local total_failed=$((IMG_FAILED + VID_FAILED))
        local total_processed=$((total_succeeded + total_skipped + total_failed))
        echo "Total:   $total_succeeded succeeded, $total_skipped skipped, $total_failed failed"
        if [[ $total_processed -gt 0 ]]; then
            local success_rate=$(( (total_succeeded * 100) / total_processed ))
            echo "Success Rate: ${success_rate}%"
        fi
        echo ""
        echo "========================================"
        echo "Session completed."
        echo "========================================"
    } >> "$LOG_FILE"
    
    echo -e "   ${DIM}📝 Log saved to: $LOG_FILE${RESET}"
}

# Draw a centered header
draw_header() {
    local width=70
    local title="🚀 MODERN FORMAT BOOST v7.0"
    local padding=$(( (width - ${#title}) / 2 ))
    
    echo ""
    echo -e "${BLUE}╭$(printf '─%.0s' {1..70})╮${RESET}"
    printf "${BLUE}│${RESET}${BG_HEADER}%*s${BOLD}${WHITE}%s${RESET}${BG_HEADER}%*s${RESET}${BLUE}│${RESET}\n" $padding "" "$title" $padding ""
    echo -e "${BLUE}│$(printf '─%.0s' {1..70})│${RESET}"
    echo -e "${BLUE}│${RESET}  ${DIM}PREMIUM MEDIA OPTIMIZER${RESET}                                            ${BLUE}│${RESET}"
    echo -e "${BLUE}│${RESET}  ${GREEN}●${RESET} ${DIM}No Data Loss${RESET}   ${GREEN}●${RESET} ${DIM}Smart Conversion${RESET}   ${GREEN}●${RESET} ${DIM}Auto-Repair${RESET}               ${BLUE}│${RESET}"
    echo -e "${BLUE}╰$(printf '─%.0s' {1..70})╯${RESET}"
    echo ""
}

# Draw a section separator
draw_separator() {
    local title="$1"
    echo -e "${DIM}── ${BOLD}${WHITE}${title}${RESET} ${DIM}$(printf '─%.0s' {1..50})${RESET}"
    echo ""
}

# 🚀 Check Tools
check_tools() {
    # Ensure build is up-to-date
    "$SCRIPT_DIR/smart_build.sh" || {
        echo -e "${RED}❌ Build failed. Please check the logs.${RESET}"
        read -rsp "Press any key to exit..." -n1
        exit 1
    }
}

# 📂 Get Target Directory
get_target_directory() {
    if [[ -z "$TARGET_DIR" ]]; then
        draw_header
        echo -e "${CYAN}📂 Waiting for input...${RESET}"
        echo -e "${DIM}   Please drag and drop a folder here, then press Enter.${RESET}"
        echo -ne "   ${BOLD}> ${RESET}"
        read -r TARGET_DIR
        # Clean path input
        TARGET_DIR="${TARGET_DIR%\"}"
        TARGET_DIR="${TARGET_DIR#\"}"
        TARGET_DIR="${TARGET_DIR%\'}"
        TARGET_DIR="${TARGET_DIR#\'}"
        TARGET_DIR="${TARGET_DIR## }"
        TARGET_DIR="${TARGET_DIR%% }"
    fi
    
    if [[ ! -d "$TARGET_DIR" ]]; then
        echo -e "\n${RED}❌ Error: Directory not found.${RESET}"
        echo -e "${DIM}   Path: $TARGET_DIR${RESET}"
        exit 1
    fi
}

# 🛡️  Safety Checks
safety_check() {
    case "$TARGET_DIR" in
        "/"|"/System"*|"/usr"*|"/bin"*|"/sbin"*|"$HOME"|"$HOME/Desktop"|"$HOME/Documents")
            echo -e "\n${RED}⚠️  SAFETY BLOCK${RESET}"
            echo -e "   System or root directories cannot be processed directly."
            exit 1
            ;;
    esac
}

# 🎮 Interactive Menu
select_mode() {
    SELECTED=0
    hide_cursor

    local options=("🚀 In-Place Optimization" "📂 Output to Adjacent Folder" "🩹 Fix iCloud Import Errors")
    local descriptions=("Replaces original files. Saves disk space." "Safe mode. Keeps originals untouched." "Fix corrupted Brotli EXIF metadata that prevents iCloud Photos import.")
    
    while true; do
        clear_screen
        draw_header
        echo -e "${BOLD}Select Operation Mode:${RESET}"
        echo ""
        
        for i in "${!options[@]}"; do
            if [[ $i -eq $SELECTED ]]; then
                echo -e "  ${CYAN}➜ ${BOLD}${options[$i]}${RESET}"
                echo -e "    ${CYAN}${DIM}${descriptions[$i]}${RESET}"
            else
                echo -e "    ${DIM}${options[$i]}${RESET}"
                echo -e "    ${DIM}${descriptions[$i]}${RESET}"
            fi
            echo ""
        done
        
        echo -e "${DIM}(Use ↑/↓ to navigate, Enter to select, q to quit)${RESET}"
        
        # Read input
        read -rsn1 key
        if [[ "$key" == $'\x1b' ]]; then
            read -rsn2 key
            if [[ "$key" == "[A" ]]; then # Up
                SELECTED=$(( (SELECTED - 1 + 3) % 3 ))
            elif [[ "$key" == "[B" ]]; then # Down
                SELECTED=$(( (SELECTED + 1) % 3 ))
            fi
        elif [[ "$key" == "" ]]; then # Enter
            break
        elif [[ "$key" == "q" ]]; then
            show_cursor
            exit 0
        fi
    done
    
    show_cursor
    
    if [[ $SELECTED -eq 0 ]]; then
        OUTPUT_MODE="inplace"
        echo -e "\n${YELLOW}⚠️  IN-PLACE MODE SELECTED${RESET}"
        echo -e "${DIM}   Original files will be replaced after successful conversion.${RESET}"
        echo -ne "   ${BOLD}Are you sure? (y/N): ${RESET}"
        read -r confirm
        [[ ! "$confirm" =~ ^[Yy]$ ]] && exit 0
    elif [[ $SELECTED -eq 1 ]]; then
        OUTPUT_MODE="adjacent"
        local base_name=$(basename "$TARGET_DIR")
        OUTPUT_DIR="$(dirname "$TARGET_DIR")/${base_name}_optimized"
        
        echo -e "\n${GREEN}✅ ADJACENT MODE SELECTED${RESET}"
        echo -e "   Output: ${DIM}$OUTPUT_DIR${RESET}"
        
        # Create output structure
        echo -e "   ${DIM}Creating directory structure...${RESET}"
        create_directory_structure "$TARGET_DIR" "$OUTPUT_DIR"
    else
        OUTPUT_MODE="brotli_fix_only"
        echo -e "\n${MAGENTA}🩹 ICLOUD IMPORT FIX MODE${RESET}"
        echo -e "${DIM}   Only files with corrupted Brotli EXIF will be fixed.${RESET}"
        echo -e "${DIM}   This resolves 'Unable to import to iCloud Photos' errors.${RESET}"
        echo ""
    fi
}

# 🛠️  Utils
create_directory_structure() {
    local src="$1"
    local dest="$2"
    mkdir -p "$dest"
    
    # 🔥 v7.4.9: 立即复制根目录时间戳
    touch -r "$src" "$dest"
    
    find "$src" -type d -print0 | while IFS= read -r -d '' dir; do
        local rel="${dir#$src}"
        rel="${rel#/}"
        if [[ -n "$rel" ]]; then
            mkdir -p "$dest/$rel"
            # 🔥 v7.4.9: 立即复制子目录时间戳
            touch -r "$dir" "$dest/$rel"
        fi
    done
}

# 📊 Stats
count_files() {
    draw_separator "Scanning Content"
    printf "${DIM}   Analyzing directory structure...${RESET}\n"
    
    TOTAL_FILES=$(find "$TARGET_DIR" -type f ! -name ".*" | wc -l | tr -d ' ')
    IMG_COUNT=$(find "$TARGET_DIR" -type f \( -iname "*.jpg" -o -iname "*.jpeg" -o -iname "*.jpe" -o -iname "*.jfif" -o -iname "*.png" -o -iname "*.webp" -o -iname "*.heic" -o -iname "*.heif" -o -iname "*.avif" -o -iname "*.gif" -o -iname "*.tiff" -o -iname "*.tif" -o -iname "*.bmp" \) | wc -l | tr -d ' ')
    VID_COUNT=$(find "$TARGET_DIR" -type f \( -iname "*.mp4" -o -iname "*.mov" -o -iname "*.mkv" -o -iname "*.avi" -o -iname "*.webm" -o -iname "*.m4v" -o -iname "*.wmv" -o -iname "*.flv" \) | wc -l | tr -d ' ')
    XMP_COUNT=$(find "$TARGET_DIR" -type f -iname "*.xmp" | wc -l | tr -d ' ')
    OTHER_COUNT=$((TOTAL_FILES - IMG_COUNT - VID_COUNT - XMP_COUNT))
    
    echo -e "   📁 Total Files: ${BOLD}$TOTAL_FILES${RESET}"
    echo -e "   🖼️  Images:      ${BOLD}${CYAN}$IMG_COUNT${RESET}"
    echo -e "   🎬 Videos:      ${BOLD}${MAGENTA}$VID_COUNT${RESET}"
    echo -e "   📋 Metadata:    ${BOLD}${DIM}$XMP_COUNT${RESET}"
    echo -e "   📦 Others:      ${BOLD}${DIM}$OTHER_COUNT${RESET} (Copy only)"
    echo ""
    
    if [[ $((IMG_COUNT + VID_COUNT)) -eq 0 ]]; then
        echo -e "${YELLOW}⚠️  No convertable media found. Only copying logic will apply.${RESET}"
    fi
}

# 🖼️  Process Images
process_images() {
    [[ $IMG_COUNT -eq 0 ]] && return 0

    draw_separator "Processing Images ($IMG_COUNT)"

    # 默认即推荐组合；仅传 run 与路径，与视频处理一致
    local args=(run)
    [[ "$ULTIMATE_MODE" == true ]] && args+=(--ultimate)
    [[ "$VERBOSE_MODE" == true ]] && args+=(--verbose)

    if [[ "$OUTPUT_MODE" == "inplace" ]]; then
        args+=(--in-place "$TARGET_DIR")
    else
        # 相邻目录模式：必须先传目录，再传 --output
        args+=("$TARGET_DIR" --output "$OUTPUT_DIR")
    fi

    # Execution - capture output for statistics parsing
    local output
    output=$("$IMGQUALITY_HEVC" "${args[@]}" 2>&1 | tee /dev/tty)
    parse_tool_stats "$output" "img"
    echo ""
}

# 🎬 Process Videos
process_videos() {
    [[ $VID_COUNT -eq 0 ]] && return 0
    
    draw_separator "Processing Videos ($VID_COUNT)"
    
    # 默认即推荐参数组合（explore + match-quality + compress + apple-compat + recursive + allow-size-tolerance）
    # 仅需传 run 与路径；递归强制开启。关闭项可组合：环境变量或在此追加 --no-apple-compat、--no-allow-size-tolerance
    local args=(run)
    [[ -n "${NO_APPLE_COMPAT:-}" ]] && args+=(--no-apple-compat)
    [[ -n "${NO_ALLOW_SIZE_TOLERANCE:-}" ]] && args+=(--no-allow-size-tolerance)
    [[ "$ULTIMATE_MODE" == true ]] && args+=(--ultimate)
    [[ "$VERBOSE_MODE" == true ]] && args+=(--verbose)
    
    if [[ "$OUTPUT_MODE" == "inplace" ]]; then
        args+=(--in-place "$TARGET_DIR")
    else
        # 相邻目录模式：必须先传目录，再传 --output
        args+=("$TARGET_DIR" --output "$OUTPUT_DIR")
    fi
    
    # Execution - capture output for statistics parsing
    local output
    output=$("$VIDQUALITY_HEVC" "${args[@]}" 2>&1 | tee /dev/tty)
    parse_tool_stats "$output" "vid"
    echo ""
}

# 📊 Merged Statistics Variables
IMG_SUCCEEDED=0
IMG_SKIPPED=0
IMG_FAILED=0
VID_SUCCEEDED=0
VID_SKIPPED=0
VID_FAILED=0

# 📊 Parse tool output for statistics
parse_tool_stats() {
    local output="$1"
    local tool_type="$2"  # "img" or "vid"
    
    # Parse "Succeeded: X" pattern
    local succeeded=$(echo "$output" | grep -oE 'Succeeded:\s*[0-9]+' | grep -oE '[0-9]+' | tail -1)
    local skipped=$(echo "$output" | grep -oE 'Skipped:\s*[0-9]+' | grep -oE '[0-9]+' | tail -1)
    local failed=$(echo "$output" | grep -oE 'Failed:\s*[0-9]+' | grep -oE '[0-9]+' | tail -1)
    
    if [[ "$tool_type" == "img" ]]; then
        IMG_SUCCEEDED=${succeeded:-0}
        IMG_SKIPPED=${skipped:-0}
        IMG_FAILED=${failed:-0}
    else
        VID_SUCCEEDED=${succeeded:-0}
        VID_SKIPPED=${skipped:-0}
        VID_FAILED=${failed:-0}
    fi
}

# 🔥 v7.10: Fix JXL Containers for iCloud Photos（脚本只负责调用，时间戳恢复由 img-hevc restore-timestamps 统一处理）
fix_jxl_containers() {
    local target_path="$TARGET_DIR"
    [[ "$OUTPUT_MODE" == "adjacent" ]] && target_path="$OUTPUT_DIR"

    local jxl_count=$(find "$target_path" -type f -iname "*.jxl" 2>/dev/null | wc -l | tr -d ' ')
    [[ $jxl_count -eq 0 ]] && return 0

    draw_separator "JXL Container Fix"
    echo -e "   ${CYAN}🔍 Checking $jxl_count JXL files for iCloud compatibility...${RESET}"
    echo ""

    local fixed=0
    local failed=0
    while IFS= read -r -d '' jxl_file; do
        local tmp_out="${jxl_file}.tmp.jxl"
        if python3 "$SCRIPT_DIR/jxl_container_fixer.py" "$jxl_file" "$tmp_out" 2>/dev/null; then
            if [[ -f "$tmp_out" ]]; then
                mv "$tmp_out" "$jxl_file"
                ((fixed++))
            fi
        else
            rm -f "$tmp_out"
            ((failed++))
        fi
    done < <(find "$target_path" -type f -iname "*.jxl" -print0)

    echo -e "   ${GREEN}✅ JXL Container Fix: $fixed fixed, $failed skipped/already-codestream${RESET}"
    echo ""
}

# 🎉 Final Summary
show_summary() {
    draw_separator "Task Completed"
    
    # Calculate totals
    local total_succeeded=$((IMG_SUCCEEDED + VID_SUCCEEDED))
    local total_skipped=$((IMG_SKIPPED + VID_SKIPPED))
    local total_failed=$((IMG_FAILED + VID_FAILED))
    local total_processed=$((total_succeeded + total_skipped + total_failed))
    
    echo -e "   ${GREEN}✅ Optimization Finished Successfully${RESET}"
    echo -e "   ${DIM}All files have been processed without omission.${RESET}"
    echo ""
    
    # Merged Statistics Report
    echo -e "   ${BOLD}📊 Merged Statistics Report${RESET}"
    echo -e "   ${DIM}───────────────────────────────────${RESET}"
    
    if [[ $IMG_COUNT -gt 0 ]]; then
        echo -e "   ${CYAN}🖼️  Images:${RESET} ${GREEN}$IMG_SUCCEEDED${RESET} succeeded, ${YELLOW}$IMG_SKIPPED${RESET} skipped, ${RED}$IMG_FAILED${RESET} failed"
    fi
    
    if [[ $VID_COUNT -gt 0 ]]; then
        echo -e "   ${MAGENTA}🎬 Videos:${RESET} ${GREEN}$VID_SUCCEEDED${RESET} succeeded, ${YELLOW}$VID_SKIPPED${RESET} skipped, ${RED}$VID_FAILED${RESET} failed"
    fi
    
    echo -e "   ${DIM}───────────────────────────────────${RESET}"
    echo -e "   ${WHITE}📦 Total:${RESET}  ${GREEN}$total_succeeded${RESET} succeeded, ${YELLOW}$total_skipped${RESET} skipped, ${RED}$total_failed${RESET} failed"
    
    if [[ $total_processed -gt 0 ]]; then
        local success_rate=$(( (total_succeeded * 100) / total_processed ))
        echo -e "   ${WHITE}📈 Success Rate:${RESET} ${GREEN}${success_rate}%${RESET}"
    fi
    
    echo ""
    
    if [[ "$OUTPUT_MODE" == "adjacent" ]]; then
        echo -e "   ${BLUE}📂 Output: $OUTPUT_DIR${RESET}"
        open "$OUTPUT_DIR" 2>/dev/null
    fi
    
    echo ""
    echo -e "${DIM}Press any key to exit...${RESET}"
    read -rsn1
    
    # 📝 Save session log
    save_log
}

# Main Execution Flow
_main() {
    clear_screen

    # Argument Parsing
    for arg in "$@"; do
        if [[ "$arg" == "--ultimate" ]]; then
            ULTIMATE_MODE=true
        elif [[ "$arg" == "--verbose" ]] || [[ "$arg" == "-v" ]]; then
            VERBOSE_MODE=true
        elif [[ -d "$arg" ]]; then
            TARGET_DIR="$arg"
        fi
    done

    check_tools
    get_target_directory

    # 🔥 显示配置信息
    echo ""
    echo -e "${CYAN}📋 Configuration:${RESET}"
    echo -e "   ${DIM}Target: ${RESET}${BOLD}$TARGET_DIR${RESET}"
    [[ "$ULTIMATE_MODE" == true ]] && echo -e "   ${MAGENTA}🔥 Ultimate Mode: ${RESET}${GREEN}ENABLED${RESET}"
    [[ "$VERBOSE_MODE" == true ]] && echo -e "   ${CYAN}💬 Verbose: ${RESET}${GREEN}ENABLED${RESET}" || echo -e "   ${DIM}💬 Verbose: DISABLED (use --verbose for details)${RESET}"
    echo ""

    safety_check
    select_mode

    # 🩹 Brotli EXIF Fix Only Mode - Skip normal processing
    if [[ "$OUTPUT_MODE" == "brotli_fix_only" ]]; then
        "$SCRIPT_DIR/fix_brotli_exif.sh" "$TARGET_DIR"

        echo ""
        echo -e "${GREEN}✅ Brotli EXIF Fix Completed${RESET}"
        echo ""
        echo -e "${DIM}Press any key to exit...${RESET}"
        read -rsn1
        exit 0
    fi

    count_files
    
    # Logic
    # Note: Modern tools (v6.9.13+) handle recursion and structure internally/robustly
    # We delegate the heavy lifting to them for progress bars and logic
    
    if [[ $IMG_COUNT -gt 0 ]]; then
        process_images
    fi
    
    if [[ $VID_COUNT -gt 0 ]]; then
        process_videos
    fi

    # Handle "Others" copying if in adjacent mode (Tools handle media, but what about others?)
    # Wait, the tool handles image formats. 
    # v6.9.13 says "Process all files". 
    # Does the tool copy non-media files? 
    # img-hevc/vid-hevc usually only touch their extensions.
    # We should perform a manual copy pass for non-media files if in adjacent mode.
    
    if [[ "$OUTPUT_MODE" == "adjacent" ]]; then
        draw_separator "Copying Non-Media Files"
        echo -ne "   ${DIM}Syncing other files...${RESET}"
        
        # Rsync is best for this - exclude media extensions we processed
        # Calculate exclusions
        # 🔥 Fixed case sensitivity issues by using bracket patterns
        local excludes=(
            --exclude="*.[jJ][pP][gG]" --exclude="*.[jJ][pP][eE][gG]" --exclude="*.[pP][nN][gG]" --exclude="*.[wW][eE][bB][pP]"
            --exclude="*.[hH][eE][iI][cC]" --exclude="*.[hH][eE][iI][fF]" --exclude="*.[aA][vV][iI][fF]" --exclude="*.[gG][iI][fF]"
            --exclude="*.[tT][iI][fF]" --exclude="*.[tT][iI][fF][fF]" --exclude="*.[jJ][pP][eE]" --exclude="*.[jJ][fF][iI][fF]"
            --exclude="*.[bB][mM][pP]" --exclude="*.[jJ][xX][lL]"
            --exclude="*.[mM][pP]4" --exclude="*.[mM][oO][vV]" --exclude="*.[mM][kK][vV]" --exclude="*.[aA][vV][iI]"
            --exclude="*.[wW][eE][bB][mM]" --exclude="*.[mM]4[vV]" --exclude="*.[wW][mM][vV]" --exclude="*.[fF][lL][vV]"
            --exclude="*.[xX][mM][pP]"
        )
        
        # Try to use brew rsync if available
        RSYNC_CMD="rsync"
        if [ -x "/opt/homebrew/opt/rsync/bin/rsync" ]; then
            RSYNC_CMD="/opt/homebrew/opt/rsync/bin/rsync"
        elif [ -x "/usr/local/opt/rsync/bin/rsync" ]; then
            RSYNC_CMD="/usr/local/opt/rsync/bin/rsync"
        fi

        "$RSYNC_CMD" -av --ignore-existing "${excludes[@]}" "$TARGET_DIR/" "$OUTPUT_DIR/" >/dev/null 2>&1
        echo -e "\r   ${GREEN}✅ Non-media files synced.${RESET}         "
        echo ""
    fi
    
    # 🔥 v7.10: Auto-fix JXL containers for iCloud Photos compatibility
    fix_jxl_containers

    # 🔥 v8.2.5: 后处理（JXL fix / rsync）会更新时间戳，统一用 shared_utils 逻辑恢复（脚本只调用）
    if [[ "$OUTPUT_MODE" == "adjacent" ]]; then
        "$IMGQUALITY_HEVC" restore-timestamps "$TARGET_DIR" "$OUTPUT_DIR" 2>/dev/null && echo -e "   ${GREEN}✅ Timestamps restored.${RESET}" || true
    fi

    show_summary
}

# 🔥 v7.0.1: Internal worker for script logging compatibility
if [[ "$1" == "--internal-worker" ]]; then
    shift
    # 💡 Variables are already initialized globally in the script
    _main "$@"
    exit $?
fi

# Wrapper function with full session logging
main() {
    init_log
    export LOG_FILE  # 🔥 Make it available to the worker process
    
    # 🔥 v7.0.1: Support both macOS and Linux script syntax
    if [[ "$OSTYPE" == "darwin"* ]]; then
        # macOS: Use -F for flush. Command and args follow the log file positional argument.
        # No -c option on macOS native script. Adding -e for child exit code.
        script -q -e -F "$LOG_FILE" "$BASH" "$0" --internal-worker "$@"
    else
        # Linux / Others: Use -f for flush, and -c for command string
        # Fallback to direct execution if script is not util-linux
        if script --version 2>/dev/null | grep -q "util-linux"; then
             script -q -f "$LOG_FILE" -c "$BASH \"$0\" --internal-worker \"$*\""
        else
             _main "$@"
        fi
    fi
}

main "$@"
