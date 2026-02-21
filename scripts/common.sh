#!/usr/bin/env bash
# common.sh - Unified Path, Color, and Metadata Utilities
# Compatible with both Bash and Zsh

# 1. Path Setup
if [[ -z "${SCRIPT_DIR:-}" ]]; then
    if [[ -n "${BASH_SOURCE[0]:-}" ]]; then
        SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
    else
        # zsh: ${(%):-%x} is the current script path
        SCRIPT_DIR="$(cd "$(dirname "${(%):-%x}")" && pwd)"
    fi
fi
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

# 2. Color Definitions (256-color compatible)
RESET='\033[0m'
NC='\033[0m'
BOLD='\033[1m'
DIM='\033[2m'
RED='\033[38;5;196m'
GREEN='\033[38;5;46m'
YELLOW='\033[38;5;226m'
BLUE='\033[38;5;39m'
CYAN='\033[38;5;51m'
MAGENTA='\033[38;5;213m'
WHITE='\033[38;5;255m'
GRAY='\033[38;5;240m'
BG_HEADER='\033[48;5;236m'

# 3. Zsh-Specific Advanced Metadata Functions
# These only activate if running in Zsh (e.g., repair_apple_photos.sh)
if [ -n "$ZSH_VERSION" ]; then
    typeset -gA dir_mtimes
    typeset -gA dir_btimes

    save_dir_timestamps() {
        local target_dir="${1:?}"
        echo -e "${DIM}🗂️  Saving directory timestamps...${NC}"
        dir_mtimes=()
        dir_btimes=()
        while IFS= read -r d; do
            local abs_d
            abs_d=$(realpath "$d")
            dir_mtimes["$abs_d"]=$(stat -f%m "$abs_d")
            dir_btimes["$abs_d"]=$(stat -f%B "$abs_d" 2>/dev/null || echo "0")
        done < <(find "$target_dir" -type d 2>/dev/null)
    }

    restore_dir_timestamps() {
        echo -e "${DIM}🗂️  Restoring directory timestamps...${NC}"
        # Use Zsh key expansion (@k) safely
        local keys=("${(@k)dir_mtimes}")
        local d m b
        # Sort keys by length descending to restore child directories before parents
        for d in ${(f)"$(printf '%s\n' "${keys[@]}" | awk '{ print length, $0 }' | sort -rn | cut -d' ' -f2-)"}; do
            [[ -z "$d" ]] && continue
            m="${dir_mtimes[$d]}"
            b="${dir_btimes[$d]}"
            if [[ -d "$d" ]]; then
                touch -mt "$(date -r "$m" +%Y%m%d%H%M.%S)" "$d" 2>/dev/null || true
                [[ "$b" != "0" ]] && SetFile -d "$(date -r "$b" +%m/%d/%Y\ %H:%M:%S)" "$d" 2>/dev/null || true
            fi
        done
    }
else
    # Fallback for Bash (silent placeholders)
    save_dir_timestamps() { :; }
    restore_dir_timestamps() { :; }
fi
