#!/usr/bin/env bash
# Modern Format Boost - Cache Cleaner v1.0
#
# Cleans analysis and quality caches to free up space.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=common.sh
source "$SCRIPT_DIR/common.sh"

CACHE_DIR="$PROJECT_ROOT/.cache"
DB_FILE="$CACHE_DIR/image_analysis_v2.db"
LOG_DIR="$PROJECT_ROOT/logs"
MFB_PROGRESS_DIR="$HOME/.mfb_progress"

clear_screen() { printf '\033[2J\033[H'; }

draw_header() {
    echo -e "${BLUE}╭$(printf '─%.0s' {1..60})╮${RESET}"
    echo -e "${BLUE}│${RESET}  ${BOLD}${RED}🔥 DATA PURGE UTILITY v1.0${RESET}                            ${BLUE}│${RESET}"
    echo -e "${BLUE}╰$(printf '─%.0s' {1..60})╯${RESET}"
    echo -e "   ${RED}⚠️  WARNING: Critical processing data will be permanently deleted.${RESET}"
    echo ""
}

show_stats() {
    echo -e "${BOLD}Current Cache Status:${RESET}"
    if [[ -d "$CACHE_DIR" ]]; then
        local size
        size=$(du -sh "$CACHE_DIR" | cut -f1)
        echo -e "   📂 Directory: ${DIM}$CACHE_DIR${RESET}"
        echo -e "   📦 Total Size: ${BOLD}${GREEN}$size${RESET}"
        
        if [[ -f "$DB_FILE" ]]; then
            local db_size
            db_size=$(du -sh "$DB_FILE" | cut -f1)
            echo -e "   🗄️  Database:  ${DIM}image_analysis_v2.db${RESET} (${db_size})"
        fi
    else
        echo -e "   ${YELLOW}Empty: No cache directory found.${RESET}"
    fi
    
    log_size=$(du -sh "$LOG_DIR" 2>/dev/null | cut -f1 || echo "0B")
    echo -e "   📝 Logs:      ${DIM}$log_size${RESET}"
    
    if [[ -d "$MFB_PROGRESS_DIR" ]]; then
        local prog_size
        prog_size=$(du -sh "$MFB_PROGRESS_DIR" 2>/dev/null | cut -f1 || echo "0B")
        echo -e "   🔄 Progress:  ${DIM}$prog_size${RESET}"
    fi
    echo ""
}

_main() {
    clear_screen
    draw_header
    show_stats

    echo -e "${RED}🔥 Purging all analysis data, logs and progress trackers...${RESET}"
    echo ""

    # Vacuum database if sqlite3 is available
    if command -v sqlite3 >/dev/null 2>&1 && [[ -f "$DB_FILE" ]]; then
        echo -e "${DIM}   Vacuuming database...${RESET}"
        sqlite3 "$DB_FILE" "VACUUM;" 2>/dev/null
        echo -e "   ${GREEN}✅ Database vacuumed${RESET}"
    fi

    # Purge cache directory
    if [[ -d "$CACHE_DIR" ]]; then
        echo -e "${DIM}   Removing cache directory...${RESET}"
        rm -rf "$CACHE_DIR"
        echo -e "   ${GREEN}✅ Cache purged${RESET}"
    fi

    # Clear logs (with safety check)
    if [[ -d "$LOG_DIR" && "$LOG_DIR" != "/" ]]; then
        echo -e "${DIM}   Clearing logs...${RESET}"
        rm -f "$LOG_DIR"/*.log
        echo -e "   ${GREEN}✅ Logs cleared${RESET}"
    fi

    # Purge MFB progress directory
    if [[ -d "$MFB_PROGRESS_DIR" ]]; then
        echo -e "${DIM}   Removing MFB progress directory...${RESET}"
        rm -rf "$MFB_PROGRESS_DIR"
        echo -e "   ${GREEN}✅ MFB progress purged${RESET}"
    fi

    echo ""
    echo -e "${GREEN}✅ Cleanup Complete${RESET}"
    echo ""
    echo -e "${DIM}Press any key to return to menu...${RESET}"
    read -rn1
    # Return to caller (select_mode in drag_and_drop_processor.sh)
    return 0
}

_main
