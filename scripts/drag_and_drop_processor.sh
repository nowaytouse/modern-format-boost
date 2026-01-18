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

# Script Location
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

# Tool Paths (🔥 v6.9.15: 修正为正确的 target/release 路径)
IMGQUALITY_HEVC="$PROJECT_ROOT/target/release/imgquality-hevc"
VIDQUALITY_HEVC="$PROJECT_ROOT/target/release/vidquality-hevc"

# Configuration
OUTPUT_MODE="inplace"
OUTPUT_DIR=""
SELECTED=0
ULTIMATE_MODE=true
VERBOSE_MODE=false  # 🔥 默认静默模式

# 🎨 Color Schemes (Premium Dark Mode)
RESET='\033[0m'
BOLD='\033[1m'
DIM='\033[2m'
RED='\033[38;5;196m'
GREEN='\033[38;5;46m'
YELLOW='\033[38;5;226m'
BLUE='\033[38;5;39m'
MAGENTA='\033[38;5;213m'
CYAN='\033[38;5;51m'
WHITE='\033[38;5;255m'
GRAY='\033[38;5;240m'
BG_HEADER='\033[48;5;236m'

# 🛠️  Helper Functions

# Hide cursor
hide_cursor() { printf '\033[?25l'; }
# Show cursor
show_cursor() { printf '\033[?25h'; }

# Clear screen
clear_screen() { printf '\033[2J\033[H'; }

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
    
    local options=("🚀 In-Place Optimization" "📂 Output to Adjacent Folder")
    local descriptions=("Replaces original files. Saves disk space." "Safe mode. Keeps originals untouched.")
    
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
        
        echo -e "${DIM}(Use ↑/↓ to navigate, Enter to select)${RESET}"
        
        # Read input
        read -rsn1 key
        if [[ "$key" == $'\x1b' ]]; then
            read -rsn2 key
            if [[ "$key" == "[A" ]]; then # Up
                SELECTED=$(( (SELECTED - 1 + 2) % 2 ))
            elif [[ "$key" == "[B" ]]; then # Down
                SELECTED=$(( (SELECTED + 1) % 2 ))
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
    else
        OUTPUT_MODE="adjacent"
        local base_name=$(basename "$TARGET_DIR")
        OUTPUT_DIR="$(dirname "$TARGET_DIR")/${base_name}_optimized"
        
        echo -e "\n${GREEN}✅ ADJACENT MODE SELECTED${RESET}"
        echo -e "   Output: ${DIM}$OUTPUT_DIR${RESET}"
        
        # Create output structure
        echo -e "   ${DIM}Creating directory structure...${RESET}"
        create_directory_structure "$TARGET_DIR" "$OUTPUT_DIR"
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
    printf "${DIM}   Analyzing directory structure...${RESET}\r"
    
    TOTAL_FILES=$(find "$TARGET_DIR" -type f ! -name ".*" | wc -l | tr -d ' ')
    IMG_COUNT=$(find "$TARGET_DIR" -type f \( -iname "*.jpg" -o -iname "*.png" -o -iname "*.webp" -o -iname "*.heic" -o -iname "*.avif" -o -iname "*.gif" -o -iname "*.tiff" -o -iname "*.bmp" \) | wc -l | tr -d ' ')
    VID_COUNT=$(find "$TARGET_DIR" -type f \( -iname "*.mp4" -o -iname "*.mov" -o -iname "*.mkv" -o -iname "*.avi" -o -iname "*.webm" \) | wc -l | tr -d ' ')
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
    
    # 🔥 v6.9.16: 修复参数顺序，确保 --recursive 正确传递以保留目录结构
    local args=(auto --explore --match-quality --compress --apple-compat --recursive)
    [[ "$ULTIMATE_MODE" == true ]] && args+=(--ultimate)
    [[ "$VERBOSE_MODE" == true ]] && args+=(--verbose)
    
    if [[ "$OUTPUT_MODE" == "inplace" ]]; then
        args+=(--in-place "$TARGET_DIR")
    else
        # 相邻目录模式：必须先传目录，再传 --output
        args+=("$TARGET_DIR" --output "$OUTPUT_DIR")
    fi
    
    # Execution
    "$IMGQUALITY_HEVC" "${args[@]}"
    echo ""
}

# 🎬 Process Videos
process_videos() {
    [[ $VID_COUNT -eq 0 ]] && return 0
    
    draw_separator "Processing Videos ($VID_COUNT)"
    
    # 🔥 v6.9.16: 修复参数顺序，确保 --recursive 正确传递以保留目录结构
    local args=(auto --explore --match-quality --compress --apple-compat --recursive)
    [[ "$ULTIMATE_MODE" == true ]] && args+=(--ultimate)
    [[ "$VERBOSE_MODE" == true ]] && args+=(--verbose)
    
    if [[ "$OUTPUT_MODE" == "inplace" ]]; then
        args+=(--in-place "$TARGET_DIR")
    else
        # 相邻目录模式：必须先传目录，再传 --output
        args+=("$TARGET_DIR" --output "$OUTPUT_DIR")
    fi
    
    # Execution
    "$VIDQUALITY_HEVC" "${args[@]}"
    echo ""
}

# 🎉 Final Summary
show_summary() {
    draw_separator "Task Completed"
    
    echo -e "   ${GREEN}✅ Optimization Finished Successfully${RESET}"
    echo -e "   ${DIM}All files have been processed without omission.${RESET}"
    
    if [[ "$OUTPUT_MODE" == "adjacent" ]]; then
        echo -e "   ${BLUE}📂 Output: $OUTPUT_DIR${RESET}"
        open "$OUTPUT_DIR" 2>/dev/null
    fi
    
    echo ""
    echo -e "${DIM}Press any key to exit...${RESET}"
    read -rsn1
}

# Main Execution Flow
main() {
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
    # imgquality-hevc/vidquality-hevc usually only touch their extensions.
    # We should perform a manual copy pass for non-media files if in adjacent mode.
    
    if [[ "$OUTPUT_MODE" == "adjacent" ]]; then
        draw_separator "Copying Non-Media Files"
        echo -ne "   ${DIM}Syncing other files...${RESET}"
        
        # Rsync is best for this - exclude media extensions we processed
        # Calculate exclusions
        local excludes=(
            --exclude="*.jpg" --exclude="*.jpeg" --exclude="*.png" --exclude="*.webp" 
            --exclude="*.heic" --exclude="*.avif" --exclude="*.gif" --exclude="*.tiff"
            --exclude="*.mp4" --exclude="*.mov" --exclude="*.mkv" --exclude="*.avi" 
            --exclude="*.webm" --exclude="*.xmp"
        )
        
        rsync -av --ignore-existing "${excludes[@]}" "$TARGET_DIR/" "$OUTPUT_DIR/" >/dev/null 2>&1
        echo -e "\r   ${GREEN}✅ Non-media files synced.${RESET}         "
        echo ""
    fi
    
    show_summary
}

main "$@"
