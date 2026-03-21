#!/usr/bin/env bash
# common.sh - Unified Path, Color, and Metadata Utilities
# Compatible with both Bash and Zsh
export LC_ALL=en_US.UTF-8
export LANG=en_US.UTF-8

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

append_path_if_exists() {
    local dir="$1"
    [[ -d "$dir" ]] || return 0
    case ":${PATH:-}:" in
        *":$dir:"*) ;;
        *) PATH="$dir${PATH:+:$PATH}" ;;
    esac
}

warn_shell() {
    printf '⚠️ [MFB Shell] %s\n' "$*" >&2
}

warn_shell_once() {
    local key="$1"
    shift
    local var_name="MFB_WARNED_${key}"
    if eval "[[ -n \${$var_name:-} ]]"; then
        return 0
    fi
    eval "$var_name=1"
    export "$var_name"
    warn_shell "$*"
}

normalize_cli_environment() {
    export LC_ALL="${LC_ALL:-en_US.UTF-8}"
    export LANG="${LANG:-en_US.UTF-8}"
    export TERM="${TERM:-xterm-256color}"
    export COLORTERM="${COLORTERM:-truecolor}"
    export SHELL="${SHELL:-/bin/zsh}"
    export HOME="${HOME:-/Users/$(id -un)}"
    export TMPDIR="${TMPDIR:-/tmp}"
    export CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"
    export RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}"

    append_path_if_exists "$CARGO_HOME/bin"
    append_path_if_exists "/opt/homebrew/bin"
    append_path_if_exists "/opt/homebrew/sbin"
    append_path_if_exists "/usr/local/bin"
    append_path_if_exists "/usr/local/sbin"
    append_path_if_exists "/usr/bin"
    append_path_if_exists "/bin"
    append_path_if_exists "/usr/sbin"
    append_path_if_exists "/sbin"
    export PATH
}

refresh_terminal_dimensions() {
    local tty_dev="/dev/tty"
    local rows=""
    local cols=""

    [[ -c "$tty_dev" ]] || return 0

    if read -r rows cols < <(stty size < "$tty_dev" 2>/dev/null); then
        :
    fi

    if [[ -z "$cols" ]] && command -v tput >/dev/null 2>&1; then
        if ! cols=$(tput cols 2>/dev/null); then
            warn_shell_once "TPUT_COLS" "tput could not read terminal columns; continuing with fallback detection."
            cols=""
        fi
        if ! rows=$(tput lines 2>/dev/null); then
            warn_shell_once "TPUT_LINES" "tput could not read terminal rows; continuing with fallback detection."
            rows=""
        fi
    fi

    [[ "$cols" =~ ^[0-9]+$ && "$cols" -gt 0 ]] && export COLUMNS="$cols"
    [[ "$rows" =~ ^[0-9]+$ && "$rows" -gt 0 ]] && export LINES="$rows"
}

ensure_wide_terminal_layout() {
    local min_cols="${1:-120}"
    local target_cols="${2:-140}"
    local target_rows="${3:-42}"

    refresh_terminal_dimensions
    if [[ "${COLUMNS:-0}" =~ ^[0-9]+$ ]] && [[ "${COLUMNS:-0}" -ge "$min_cols" ]]; then
        return 0
    fi

    if [[ -c /dev/tty ]]; then
        if ! printf '\033[8;%s;%st' "$target_rows" "$target_cols" > /dev/tty 2>/dev/null; then
            warn_shell_once "TTY_RESIZE_ESCAPE" "terminal did not accept ANSI resize escape; continuing with width fallback."
        fi
    fi

    case "${TERM_PROGRAM:-}" in
        Apple_Terminal)
            if ! osascript >/dev/null 2>&1 <<'EOF'
tell application "Terminal"
    if (count of windows) > 0 then
        set bounds of front window to {80, 60, 1720, 980}
        activate
    end if
end tell
EOF
            then
                warn_shell_once "APPLE_TERMINAL_RESIZE" "Apple Terminal window resize via AppleScript failed; continuing with width fallback."
            fi
            ;;
        iTerm.app)
            if ! osascript >/dev/null 2>&1 <<'EOF'
tell application "iTerm"
    if (count of windows) > 0 then
        set bounds of current window to {80, 60, 1720, 980}
        activate
    end if
end tell
EOF
            then
                warn_shell_once "ITERM_RESIZE" "iTerm window resize via AppleScript failed; continuing with width fallback."
            fi
            ;;
    esac

    sleep 0.2
    refresh_terminal_dimensions
    if [[ "${COLUMNS:-0}" =~ ^[0-9]+$ ]] && [[ "${COLUMNS:-0}" -lt "$min_cols" ]]; then
        export COLUMNS="$target_cols"
    fi
}

normalize_cli_environment
refresh_terminal_dimensions

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
                if ! touch -mt "$(date -r "$m" +%Y%m%d%H%M.%S)" "$d" 2>/dev/null; then
                    warn_shell_once "RESTORE_DIR_MTIME" "failed to restore one or more directory modification times."
                fi
                if [[ "$b" != "0" ]] && ! SetFile -d "$(date -r "$b" +%m/%d/%Y\ %H:%M:%S)" "$d" 2>/dev/null; then
                    warn_shell_once "RESTORE_DIR_BTIME" "failed to restore one or more directory creation times."
                fi
            fi
        done
    }
else
    # Fallback for Bash (silent placeholders)
    save_dir_timestamps() { :; }
    restore_dir_timestamps() { :; }
fi
