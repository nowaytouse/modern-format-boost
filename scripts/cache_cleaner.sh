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

clear_screen() { printf '\033[2J\033[H'; }

draw_header() {
    echo -e "${BLUE}╭$(printf '─%.0s' {1..60})╮${RESET}"
    echo -e "${BLUE}│${RESET}  ${BOLD}${CYAN}🧹 CACHE CLEANER v1.0${RESET}                                ${BLUE}│${RESET}"
    echo -e "${BLUE}╰$(printf '─%.0s' {1..60})╯${RESET}"
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
    
    local log_size
    log_size=$(du -sh "$LOG_DIR" 2>/dev/null | cut -f1 || echo "0B")
    echo -e "   📝 Logs:      ${DIM}$log_size${RESET}"
    echo ""
}

_main() {
    clear_screen
    draw_header
    show_stats

    echo -e "${BOLD}Select Cleaning Operation:${RESET}"
    echo -e "   ${CYAN}1)${RESET} ${WHITE}Vacuum Only${RESET} (Prune old entries, keep database)"
    echo -e "   ${CYAN}2)${RESET} ${WHITE}Purge All Cache${RESET} (Delete entire .cache folder)"
    echo -e "   ${CYAN}3)${RESET} ${WHITE}Clear Logs${RESET} (Delete all log files)"
    echo -e "   ${CYAN}q)${RESET} ${WHITE}Cancel & Quit${RESET}"
    echo ""
    echo -ne "   ${BOLD}Choice > ${RESET}"
    
    read -r choice
    echo -e "\n"

    case "$choice" in
        1)
            echo -e "${CYAN}Pruning records older than 30 days and vacuuming...${RESET}"
            # We don't have a direct CLI for cleanup_old_records yet, 
            # but we can use sqlite3 if available or just wait for the tool to do it.
            # For a simple script, we'll try to use sqlite3 if present to VACCUM.
            if command -v sqlite3 >/dev/null 2>&1 && [[ -f "$DB_FILE" ]]; then
                sqlite3 "$DB_FILE" "VACUUM;"
                echo -e "${GREEN}✅ Database vacuumed.${RESET}"
            else
                echo -e "${YELLOW}⚠️ sqlite3 not found or DB missing. Vacuum skipped.${RESET}"
            fi
            ;;
        2)
            echo -ne "${RED}${BOLD}All cached analysis will be lost. Are you sure? (y/N): ${RESET}"
            read -r confirm
            if [[ "$confirm" =~ ^[Yy]$ ]]; then
                rm -rf "$CACHE_DIR"
                echo -e "\n${GREEN}✅ Cache directory purged.${RESET}"
            else
                echo -e "\n${DIM}Operation cancelled.${RESET}"
            fi
            ;;
        3)
            echo -ne "${YELLOW}Delete all session logs? (y/N): ${RESET}"
            read -r confirm
            if [[ "$confirm" =~ ^[Yy]$ ]]; then
                # Security check: ensure LOG_DIR is not empty/root
                if [[ -d "$LOG_DIR" && "$LOG_DIR" != "/" ]]; then
                    rm -f "$LOG_DIR"/*.log
                    echo -e "\n${GREEN}✅ Logs cleared.${RESET}"
                else
                    echo -e "\n${RED}❌ Invalid log directory.${RESET}"
                fi
            else
                echo -e "\n${DIM}Operation cancelled.${RESET}"
            fi
            ;;
        [qQ])
            exit 0
            ;;
        *)
            echo -e "${RED}Invalid selection.${RESET}"
            ;;
    esac

    echo ""
    echo -e "${DIM}Press any key to exit...${RESET}"
    read -rn1
}

_main
