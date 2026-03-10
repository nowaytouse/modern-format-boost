#!/usr/bin/env bash
# Modern Format Boost - Drag & Drop Processor v7.0
# 
# Usage: Drag folder onto this script or double-click to select

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=common.sh
source "$SCRIPT_DIR/common.sh"

# Tool Paths
IMGQUALITY_HEVC="$PROJECT_ROOT/target/release/img-hevc"
VIDQUALITY_HEVC="$PROJECT_ROOT/target/release/vid-hevc"

# Configuration
OUTPUT_MODE="inplace"
OUTPUT_DIR=""
SELECTED=0
ULTIMATE_MODE=true
VERBOSE_MODE=false

# 🛠️  Helper Functions

hide_cursor() { printf '\033[?25l'; }
show_cursor() { printf '\033[?25h'; }
clear_screen() { printf '\033[2J\033[H'; }

SPINNER_PID=""
ELAPSED_START=0

# Format seconds matching Rust format_duration_compact() style (without ms).
# Bash only has second precision, so ms is omitted.
# Short: 45s | Medium: 05m00s | Long: 01h  00m00s
# Day+: 01D   01h  00m00s | Week+: 01W   01D   01h  00m00s
_fmt_elapsed() {
    local t=$1
    [[ "$t" -lt 0 ]] && t=0
    local Y M W D h m s
    Y=$(( t / (365*86400) )); t=$(( t % (365*86400) ))
    M=$(( t / (30*86400) ));  t=$(( t % (30*86400) ))
    W=$(( t / (7*86400) ));   t=$(( t % (7*86400) ))
    D=$(( t / 86400 ));       t=$(( t % 86400 ))
    h=$(( t / 3600 ));        t=$(( t % 3600 ))
    m=$(( t / 60 ));          s=$(( t % 60 ))

    if [[ $Y -gt 0 ]]; then
        printf '%02dY   %02dM   %02dW   %02dD   %02dh  %02dm%02ds' "$Y" "$M" "$W" "$D" "$h" "$m" "$s"
    elif [[ $M -gt 0 ]]; then
        printf '%02dM   %02dW   %02dD   %02dh  %02dm%02ds' "$M" "$W" "$D" "$h" "$m" "$s"
    elif [[ $W -gt 0 ]]; then
        printf '%02dW   %02dD   %02dh  %02dm%02ds' "$W" "$D" "$h" "$m" "$s"
    elif [[ $D -gt 0 ]]; then
        printf '%02dD   %02dh  %02dm%02ds' "$D" "$h" "$m" "$s"
    elif [[ $h -gt 0 ]]; then
        printf '%02dh  %02dm%02ds' "$h" "$m" "$s"
    elif [[ $m -gt 0 ]]; then
        printf '%02dm%02ds' "$m" "$s"
    else
        printf '%02ds' "$s"
    fi
}

# Write elapsed time to terminal TITLE BAR (OSC escape), not content area.
# This completely avoids collision with binary output in the content area.
# Pad with trailing spaces to overwrite any previous longer title content.
_tty_title() { [[ -c /dev/tty ]] && printf '\033]0;⏱ %s                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                \007' "$1" > /dev/tty 2>/dev/null; }
start_elapsed_spinner() {
    ELAPSED_START=$(date +%s)
    [[ -n "$SPINNER_PID" ]] && return
    (
        local start=$ELAPSED_START
        while true; do
            now=$(date +%s)
            elapsed_sec=$(( now - start ))
            _tty_title "$(_fmt_elapsed "$elapsed_sec")"
            sleep 0.15
        done
    ) 2>/dev/null &
    SPINNER_PID=$!
    disown "$SPINNER_PID" 2>/dev/null || true
}
stop_elapsed_spinner() {
    if [[ -n "$SPINNER_PID" ]]; then
        ( kill "$SPINNER_PID" 2>/dev/null; wait "$SPINNER_PID" 2>/dev/null ) 2>/dev/null || true
        SPINNER_PID=""
    fi
    [[ "$ELAPSED_START" -eq 0 ]] && return
    local end
    end=$(date +%s)
    local elapsed_sec=$(( end - ELAPSED_START ))
    [[ "$elapsed_sec" -lt 0 ]] && elapsed_sec=0
    local elapsed_str
    elapsed_str=$(_fmt_elapsed "$elapsed_sec")

    echo "   Total time: $elapsed_str"
    # Clear title bar
    [[ -c /dev/tty ]] && printf '\033]0;\007' > /dev/tty 2>/dev/null
}

# ── Ctrl+C confirmation guard ─────────────────────────────────────────────────
# If the user presses Ctrl+C after 4.5 min of processing, ask for confirmation.
# No input within 8 seconds or pressing 'n' resumes processing.
# Pressing 'y' performs a clean exit (stops spinner, restores cursor).
_CTRLC_CONFIRM_ACTIVE=false

_handle_sigint() {
    # If already in the confirmation prompt, ignore re-entrant signals
    [[ "$_CTRLC_CONFIRM_ACTIVE" == true ]] && return

    local elapsed=0
    [[ "$ELAPSED_START" -gt 0 ]] && elapsed=$(( $(date +%s) - ELAPSED_START ))

    # Under 4.5 minutes: exit immediately without confirmation
    if [[ "$elapsed" -lt 270 ]]; then
        echo ""
        show_cursor
        stop_elapsed_spinner
        echo -e "\n${YELLOW}⚠️  Interrupted by user.${RESET}"
        exit 130
    fi

    # 4.5+ minutes: ask for confirmation (read from /dev/tty, 8s timeout)
    _CTRLC_CONFIRM_ACTIVE=true
    local elapsed_str
    elapsed_str=$(_fmt_elapsed "$elapsed")
    printf '\n' > /dev/tty
    printf "${YELLOW}⚠️  Ctrl+C detected after %s of processing.${RESET}\n" "$elapsed_str" > /dev/tty
    printf "${BOLD}   Confirm exit? [y/N] (auto-resume in 8s): ${RESET}" > /dev/tty

    local answer=""
    if read -r -t 8 -n 1 answer < /dev/tty 2>/dev/null; then
        printf '\n' > /dev/tty
        if [[ "$answer" == "y" || "$answer" == "Y" ]]; then
            show_cursor
            stop_elapsed_spinner
            echo -e "\n${YELLOW}⚠️  Interrupted by user after $elapsed_str.${RESET}"
            exit 130
        fi
    else
        printf '\n' > /dev/tty
    fi

    printf "${GREEN}▶  Resuming...${RESET}\n" > /dev/tty
    _CTRLC_CONFIRM_ACTIVE=false
}

trap '_handle_sigint' INT


LOG_DIR="$PROJECT_ROOT/logs"
LOG_FILE=""
VERBOSE_LOG_FILE=""
SESSION_START_TIME=""

init_log() {
    SESSION_START_TIME=$(date +"%Y-%m-%d_%H-%M-%S")
    mkdir -p "$LOG_DIR"
    LOG_FILE="$LOG_DIR/drag_drop_${SESSION_START_TIME}.log"
    VERBOSE_LOG_FILE="$LOG_DIR/verbose_${SESSION_START_TIME}.log"
}

merge_run_logs() {
    [[ -z "$LOG_FILE" ]] && return
    [[ -z "$SESSION_START_TIME" ]] && return

    # Only merge if running via app
    if [[ -n "$FROM_APP" ]]; then
        local merged_log="$LOG_DIR/merged_${SESSION_START_TIME}.log"

        # Find the most recent img and vid logs (they may have slightly different timestamps)
        local img_log=$(ls -t "$LOG_DIR"/img_hevc_run_*.log 2>/dev/null | head -1)
        local vid_log=$(ls -t "$LOG_DIR"/vid_hevc_run_*.log 2>/dev/null | head -1)

        {
            echo "========================================"
            echo "📋 MERGED LOG - Modern Format Boost"
            echo "========================================"
            echo "Session: $SESSION_START_TIME"
            echo ""

            if [[ -f "$LOG_FILE" ]]; then
                echo "========================================"
                echo "🔧 Drag & Drop Script Log"
                echo "========================================"
                cat "$LOG_FILE"
                echo ""
            fi

            if [[ -n "$img_log" && -f "$img_log" ]]; then
                echo "========================================"
                echo "🖼️  Image Processing Log"
                echo "========================================"
                cat "$img_log"
                echo ""
            fi

            if [[ -n "$vid_log" && -f "$vid_log" ]]; then
                echo "========================================"
                echo "🎬 Video Processing Log"
                echo "========================================"
                cat "$vid_log"
                echo ""
            fi

            echo "========================================"
            echo "✅ Log Merge Complete"
            echo "========================================"
        } > "$merged_log"

        # Remove individual logs after merge
        [[ -f "$LOG_FILE" ]] && rm -f "$LOG_FILE"
        [[ -n "$img_log" && -f "$img_log" ]] && rm -f "$img_log"
        [[ -n "$vid_log" && -f "$vid_log" ]] && rm -f "$vid_log"
    fi
}

save_log() {
    [[ -z "$LOG_FILE" ]] && return
    [[ ! -f "$LOG_FILE" ]] && return

    if [[ -n "$VERBOSE_LOG_FILE" && -f "$VERBOSE_LOG_FILE" ]]; then
        {
            echo ""
            echo "========================================"
            echo "📋 Full internal tool log"
            echo "========================================"
            cat "$VERBOSE_LOG_FILE"
            echo ""
        } >> "$LOG_FILE"
    fi

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
    
    echo -e "   ${DIM}📝 Session log:  $LOG_FILE${RESET}"
}

draw_header() {
    local width=70
    local title="🚀 MODERN FORMAT BOOST v7.0"
    local padding=$(( (width - ${#title}) / 2 ))
    
    echo ""
    echo -e "${BLUE}╭$(printf '─%.0s' {1..70})╮${RESET}"
    printf "${BLUE}│${RESET}${BG_HEADER}%*s${BOLD}${WHITE}%s${RESET}${BG_HEADER}%*s${RESET}${BLUE}│${RESET}\n" $padding "" "$title" $padding ""
    echo -e "${BLUE}│$(printf '─%.0s' {1..70})│${RESET}"
    echo -e "${BLUE}│${RESET}  ${DIM}PREMIUM MEDIA OPTIMIZER${RESET}               ${BLUE}│${RESET}"
    echo -e "${BLUE}│${RESET}  ${GREEN}●${RESET} ${DIM}No Data Loss${RESET}   ${GREEN}●${RESET} ${DIM}Smart Conversion${RESET}   ${GREEN}●${RESET} ${DIM}Auto-Repair${RESET}               ${BLUE}│${RESET}"
    echo -e "${BLUE}╰$(printf '─%.0s' {1..70})╯${RESET}"
    echo ""
}

draw_separator() {
    local title="$1"
    echo -e "${DIM}── ${BOLD}${WHITE}${title}${RESET} ${DIM}$(printf '─%.0s' {1..50})${RESET}"
    echo ""
}

check_tools() {
    "$SCRIPT_DIR/smart_build.sh" || {
        echo -e "${RED}❌ Build failed. Please check the logs.${RESET}"
        drain_stdin
        read -rsp "Press any key to exit..." -n1
        exit 1
    }
}

get_target_directory() {
    if [[ -z "$TARGET_DIR" ]]; then
        draw_header
        echo -e "${CYAN}📂 Waiting for input...${RESET}"
        echo -e "${DIM}   Please drag and drop a folder here, then press Enter.${RESET}"
        echo -ne "   ${BOLD}> ${RESET}"
        drain_stdin
        read -r TARGET_DIR
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

safety_check() {
    case "$TARGET_DIR" in
        "/"|"/System"*|"/usr"*|"/bin"*|"/sbin"*|"$HOME"|"$HOME/Desktop"|"$HOME/Documents")
            echo -e "\n${RED}⚠️  SAFETY BLOCK${RESET}"
            echo -e "   System or root directories cannot be processed directly."
            exit 1
            ;;
    esac
}

drain_stdin() {
    while read -rsn1 -t 0.01 _ 2>/dev/null; do :; done
}

select_mode() {
    drain_stdin
    SELECTED=0
    hide_cursor

    local options=("📂 Output to Adjacent Folder" "🚀 In-Place Optimization" "🩹 Fix iCloud Import Errors")
    local descriptions=("Safe mode. Keeps originals untouched." "Replaces original files. Saves disk space." "Fix corrupted Brotli EXIF metadata that prevents iCloud Photos import.")
    
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
        
        read -rsn1 key
        if [[ "$key" == $'\x1b' ]]; then
            read -rsn2 key
            if [[ "$key" == "[A" ]]; then
                SELECTED=$(( (SELECTED - 1 + 3) % 3 ))
            elif [[ "$key" == "[B" ]]; then
                SELECTED=$(( (SELECTED + 1) % 3 ))
            fi
        elif [[ "$key" == "" ]]; then
            break
        elif [[ "$key" == "q" ]]; then
            show_cursor
            exit 0
        fi
    done
    
    show_cursor
    
    if [[ $SELECTED -eq 0 ]]; then
        OUTPUT_MODE="adjacent"
        local base_name=$(basename "$TARGET_DIR")
        OUTPUT_DIR="$(dirname "$TARGET_DIR")/${base_name}_optimized"
        
        echo -e "\n${GREEN}✅ ADJACENT MODE SELECTED${RESET}"
        echo -e "   Output: ${DIM}$OUTPUT_DIR${RESET}"
        echo -e "   ${DIM}Creating directory structure...${RESET}"
        create_directory_structure "$TARGET_DIR" "$OUTPUT_DIR"
    elif [[ $SELECTED -eq 1 ]]; then
        OUTPUT_MODE="inplace"
        echo -e "\n${YELLOW}⚠️  IN-PLACE MODE SELECTED${RESET}"
        echo -e "${DIM}   Original files will be replaced after successful conversion.${RESET}"
        echo -ne "   ${BOLD}Are you sure? (y/N): ${RESET}"
        drain_stdin
        read -r confirm
        [[ ! "$confirm" =~ ^[Yy]$ ]] && exit 0
    else
        OUTPUT_MODE="brotli_fix_only"
        echo -e "\n${MAGENTA}🩹 ICLOUD IMPORT FIX MODE${RESET}"
        echo -e "${DIM}   Only files with corrupted Brotli EXIF will be fixed.${RESET}"
        echo -e "${DIM}   This resolves 'Unable to import to iCloud Photos' errors.${RESET}"
        echo ""
    fi
}

create_directory_structure() {
    local src="$1"
    local dest="$2"
    mkdir -p "$dest"
    touch -r "$src" "$dest"
    
    find "$src" -type d -print0 | while IFS= read -r -d '' dir; do
        local rel="${dir#$src}"
        rel="${rel#/}"
        if [[ -n "$rel" ]]; then
            mkdir -p "$dest/$rel"
            touch -r "$dir" "$dest/$rel"
        fi
    done
}

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
}

process_images() {
    [[ $IMG_COUNT -eq 0 ]] && return 0
    draw_separator "Processing Images ($IMG_COUNT)"
    local args=(run --recursive)
    [[ "$ULTIMATE_MODE" == true ]] && args+=(--ultimate)
    [[ "$VERBOSE_MODE" == true ]] && args+=(--verbose)

    if [[ "$OUTPUT_MODE" == "inplace" ]]; then
        args+=(--in-place "$TARGET_DIR")
    else
        args+=("$TARGET_DIR" --output "$OUTPUT_DIR")
    fi

    # Spinner runs in terminal title bar — no collision with binary output.
    local output
    if [[ -n "$LOG_FILE" ]]; then
        output=$("$IMGQUALITY_HEVC" "${args[@]}" 2>&1 | tee /dev/stderr | tee -a "$LOG_FILE")
    else
        output=$("$IMGQUALITY_HEVC" "${args[@]}" 2>&1 | tee /dev/stderr)
    fi
    parse_tool_stats "$output" "img"
    echo ""
}

process_videos() {
    [[ $VID_COUNT -eq 0 ]] && return 0
    draw_separator "Processing Videos ($VID_COUNT)"
    local args=(run --recursive)
    [[ "$ULTIMATE_MODE" == true ]] && args+=(--ultimate)
    [[ "$VERBOSE_MODE" == true ]] && args+=(--verbose)

    if [[ "$OUTPUT_MODE" == "inplace" ]]; then
        args+=(--in-place "$TARGET_DIR")
    else
        args+=("$TARGET_DIR" --output "$OUTPUT_DIR")
    fi

    local output
    if [[ -n "$LOG_FILE" ]]; then
        output=$("$VIDQUALITY_HEVC" "${args[@]}" 2>&1 | tee /dev/stderr | tee -a "$LOG_FILE")
    else
        output=$("$VIDQUALITY_HEVC" "${args[@]}" 2>&1 | tee /dev/stderr)
    fi
    parse_tool_stats "$output" "vid"
    echo ""
}

IMG_SUCCEEDED=0
IMG_SKIPPED=0
IMG_FAILED=0
VID_SUCCEEDED=0
VID_SKIPPED=0
VID_FAILED=0

parse_tool_stats() {
    local output="$1"
    local tool_type="$2"
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

show_summary() {
    draw_separator "Task Completed"
    local total_succeeded=$((IMG_SUCCEEDED + VID_SUCCEEDED))
    local total_skipped=$((IMG_SKIPPED + VID_SKIPPED))
    local total_failed=$((IMG_FAILED + VID_FAILED))
    local total_processed=$((total_succeeded + total_skipped + total_failed))
    
    echo -e "   ${GREEN}✅ Optimization Finished Successfully${RESET}"
    echo ""
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
    drain_stdin
    read -rsn1
    save_log
    merge_run_logs
}

_main() {
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
    echo ""
    echo -e "${CYAN}📋 Configuration:${RESET}"
    echo -e "   ${DIM}Target: ${RESET}${BOLD}$TARGET_DIR${RESET}"
    [[ "$ULTIMATE_MODE" == true ]] && echo -e "   ${MAGENTA}🔥 Ultimate Mode: ${RESET}${GREEN}ENABLED${RESET}"
    [[ "$VERBOSE_MODE" == true ]] && echo -e "   ${CYAN}💬 Verbose: ${RESET}${GREEN}ENABLED${RESET}"
    echo ""

    safety_check
    select_mode

    if [[ "$OUTPUT_MODE" == "brotli_fix_only" ]]; then
        "$SCRIPT_DIR/repair_apple_photos.sh" "$TARGET_DIR"
        echo ""
        echo -e "${GREEN}✅ Brotli EXIF Fix Completed${RESET}"
        echo ""
        echo -e "${DIM}Press any key to exit...${RESET}"
        read -rsn1
        exit 0
    fi

    count_files
    if [[ $IMG_COUNT -gt 0 || $VID_COUNT -gt 0 ]]; then
        start_elapsed_spinner
    fi
    if [[ $IMG_COUNT -gt 0 ]]; then
        process_images
    fi
    if [[ $VID_COUNT -gt 0 ]]; then
        process_videos
    fi
    if [[ $IMG_COUNT -gt 0 || $VID_COUNT -gt 0 ]]; then
        stop_elapsed_spinner
    fi
    
    if [[ "$OUTPUT_MODE" == "adjacent" ]]; then
        draw_separator "Syncing Non-Media Files"
        local excludes=(
            --exclude="*.[jJ][pP][gG]" --exclude="*.[jJ][pP][eE][gG]" --exclude="*.[pP][nN][gG]" --exclude="*.[wW][eE][bB][pP]"
            --exclude="*.[hH][eE][iI][cC]" --exclude="*.[hH][eE][iI][fF]" --exclude="*.[aA][vV][iI][fF]" --exclude="*.[gG][iI][fF]"
            --exclude="*.[tT][iI][fF]" --exclude="*.[tT][iI][fF][fF]" --exclude="*.[jJ][pP][eE]" --exclude="*.[jJ][fF][iI][fF]"
            --exclude="*.[bB][mM][pP]" --exclude="*.[jJ][xX][lL]"
            --exclude="*.[mM][pP]4" --exclude="*.[mM][oO][vV]" --exclude="*.[mM][kK][vV]" --exclude="*.[aA][vV][iI]"
            --exclude="*.[wW][eE][bB][mM]" --exclude="*.[mM]4[vV]" --exclude="*.[wW][mM][vV]" --exclude="*.[fF][lL][vV]"
            --exclude="*.[xX][mM][pP]"
        )
        RSYNC_CMD="rsync"
        if [ -x "/opt/homebrew/opt/rsync/bin/rsync" ]; then RSYNC_CMD="/opt/homebrew/opt/rsync/bin/rsync"; fi
        "$RSYNC_CMD" -av --ignore-existing "${excludes[@]}" "$TARGET_DIR/" "$OUTPUT_DIR/" >/dev/null 2>&1
        echo -e "   ${GREEN}✅ Non-media files synced.${RESET}"
        "$IMGQUALITY_HEVC" restore-timestamps "$TARGET_DIR" "$OUTPUT_DIR" 2>/dev/null && echo -e "   ${GREEN}✅ Timestamps restored.${RESET}" || true
    fi

    show_summary
}

if [[ "$1" == "--internal-worker" ]]; then
    shift
    _main "$@"
    exit $?
fi

main() {
    init_log
    export LOG_FILE
    export VERBOSE_LOG_FILE
    ( "$BASH" "$0" --internal-worker "$@" ) 2>&1 | tee "$LOG_FILE"
    exit "${PIPESTATUS[0]:-$?}"
}

main "$@"
