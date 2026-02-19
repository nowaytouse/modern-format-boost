#!/opt/homebrew/bin/bash
# Modern Format Boost - JXL Backup Cleanup
# Remove .container.backup files after verification

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Colors
RESET='\033[0m'
BOLD='\033[1m'
RED='\033[38;5;196m'
GREEN='\033[38;5;46m'
YELLOW='\033[38;5;226m'
CYAN='\033[38;5;51m'
DIM='\033[2m'

main() {
    local target_dir="${1:-.}"
    
    echo ""
    echo -e "${CYAN}🗑️  JXL Backup Cleanup${RESET}"
    echo -e "${DIM}Part of Modern Format Boost${RESET}"
    echo ""
    
    if [[ ! -d "$target_dir" ]]; then
        echo -e "${RED}✗ Directory not found: $target_dir${RESET}"
        exit 1
    fi
    
    # Find backup files
    local backup_files=($(find "$target_dir" -type f -name "*.container.backup" 2>/dev/null))
    local count=${#backup_files[@]}
    
    if [[ $count -eq 0 ]]; then
        echo -e "${DIM}No backup files found in: $target_dir${RESET}"
        exit 0
    fi
    
    echo -e "${YELLOW}Found $count backup file(s):${RESET}"
    echo ""
    
    # List files with sizes
    local total_size=0
    for file in "${backup_files[@]}"; do
        local size=$(stat -f%z "$file" 2>/dev/null || stat -c%s "$file" 2>/dev/null)
        local size_mb=$(echo "scale=2; $size / 1024 / 1024" | bc)
        total_size=$((total_size + size))
        echo -e "  ${DIM}$(basename "$file") (${size_mb} MB)${RESET}"
    done
    
    local total_mb=$(echo "scale=2; $total_size / 1024 / 1024" | bc)
    echo ""
    echo -e "${BOLD}Total: $count files, ${total_mb} MB${RESET}"
    echo ""
    
    # Confirmation
    echo -e "${YELLOW}⚠️  This will permanently delete backup files${RESET}"
    echo -e "${DIM}   Make sure converted files work correctly before proceeding${RESET}"
    echo ""
    echo -ne "${BOLD}Delete all backups? (y/N): ${RESET}"
    read -r confirm
    
    if [[ ! "$confirm" =~ ^[Yy]$ ]]; then
        echo -e "${DIM}Cancelled${RESET}"
        exit 0
    fi
    
    # Delete backups
    echo ""
    echo -e "${CYAN}Deleting backups...${RESET}"
    
    local deleted=0
    for file in "${backup_files[@]}"; do
        if rm "$file" 2>/dev/null; then
            ((deleted++))
            echo -e "  ${GREEN}✓${RESET} ${DIM}$(basename "$file")${RESET}"
        else
            echo -e "  ${RED}✗${RESET} ${DIM}$(basename "$file")${RESET}"
        fi
    done
    
    echo ""
    echo -e "${GREEN}✓ Deleted $deleted/$count backup files${RESET}"
    echo -e "${DIM}  Freed ${total_mb} MB${RESET}"
}

main "$@"
