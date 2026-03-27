#!/usr/bin/env bash
# Smart Build System v7.5 - Selective build for Rust workspace projects
# Unified with common.sh (Path, Color, and Metadata Utilities)

set -Eeuo pipefail

export LC_ALL=en_US.UTF-8
export LANG=en_US.UTF-8

# ═══════════════════════════════════════════════════════════════
# 1. Path Setup & Unified Utilities (from common.sh)
# ═══════════════════════════════════════════════════════════════
if [[ -z "${SCRIPT_DIR:-}" ]]; then
	if [[ -n "${BASH_SOURCE[0]:-}" ]]; then
		SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
	elif [[ -n "${ZSH_VERSION:-}" ]]; then
		# zsh: use prompt expansion to get the sourced file path reliably
		# shellcheck disable=SC2296
		SCRIPT_DIR="$(cd "$(dirname "${(%):-%x}")" && pwd)"
	else
		# Fallback for other shells
		SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
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
	export "${var_name?}"
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

	if read -r rows cols < <(stty size <"$tty_dev" 2>/dev/null); then
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
		if ! printf '\033[8;%s;%st' "$target_rows" "$target_cols" >/dev/tty 2>/dev/null; then
			warn_shell_once "TTY_RESIZE_ESCAPE" "terminal did not accept ANSI resize escape; continuing with width fallback."
		fi
	fi

	case "${TERM_PROGRAM:-}" in
	Apple_Terminal)
		if ! osascript >/dev/null 2>&1 <<'EOF'; then
tell application "Terminal"
    if (count of windows) > 0 then
        set bounds of front window to {80, 60, 1720, 980}
        activate
    end if
end tell
EOF
			warn_shell_once "APPLE_TERMINAL_RESIZE" "Apple Terminal window resize via AppleScript failed; continuing with width fallback."
		fi
		;;
	iTerm.app)
		if ! osascript >/dev/null 2>&1 <<'EOF'; then
tell application "iTerm"
    if (count of windows) > 0 then
        set bounds of current window to {80, 60, 1720, 980}
        activate
    end if
end tell
EOF
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

# ═══════════════════════════════════════════════════════════════
# 2. Color Definitions (256-color compatible)
# ═══════════════════════════════════════════════════════════════
RESET='\033[0m'
NC='\033[0m'
BOLD='\033[1m'
DIM='\033[2m'
# shellcheck disable=SC2034
RED='\033[38;5;196m'
# shellcheck disable=SC2034
GREEN='\033[38;5;46m'
# shellcheck disable=SC2034
YELLOW='\033[38;5;226m'
# shellcheck disable=SC2034
BLUE='\033[38;5;39m'
# shellcheck disable=SC2034
MAGENTA='\033[38;5;162m'
# shellcheck disable=SC2034
CYAN='\033[38;5;51m'
# shellcheck disable=SC2034
ORANGE='\033[38;5;208m'
# shellcheck disable=SC2034
WHITE='\033[38;5;255m'
# shellcheck disable=SC2034
GRAY='\033[38;5;240m'
# shellcheck disable=SC2034
BG_HEADER='\033[48;5;236m'

# ═══════════════════════════════════════════════════════════════
# 3. Versioning & Branch Awareness
# ═══════════════════════════════════════════════════════════════
GET_BRANCH_TAG() {
	local branch
	if [[ -d "$PROJECT_ROOT/.git" ]]; then
		branch=$(git -C "$PROJECT_ROOT" symbolic-ref --short HEAD 2>/dev/null || git -C "$PROJECT_ROOT" rev-parse --short HEAD 2>/dev/null)
	fi

	case "${branch:-}" in
	nightly)
		echo -e " ${BOLD}${MAGENTA}[NIGHTLY]${RESET}"
		;;
	main)
		echo -e " ${BOLD}${CYAN}[MAIN]${RESET}"
		;;
	*)
		if [[ -n "$branch" ]]; then
			echo -e " ${DIM}[$branch]${RESET}"
		else
			echo ""
		fi
		;;
	esac
}

# ═══════════════════════════════════════════════════════════════
# 4. Zsh-Specific Advanced Metadata Functions
# ═══════════════════════════════════════════════════════════════
# These only activate if running in Zsh (e.g., repair_apple_photos.sh)
if [ -n "${ZSH_VERSION:-}" ]; then
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
		# shellcheck disable=SC2296
		local keys=("${(@k)dir_mtimes}")
		local d m b
		# Sort keys by length descending to restore child directories before parents
		# shellcheck disable=SC2296
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

# ═══════════════════════════════════════════════════════════════
# Smart Build System (from smart_build.sh)
# ═══════════════════════════════════════════════════════════════
cd "$PROJECT_ROOT"

# Format: "project_dir:binary_name"
ALL_PROJECTS=(
	"img_hevc:img-hevc"
	"vid_hevc:vid-hevc"
	"img_av1:img-av1"
	"vid_av1:vid-av1"
)

# Default build targets (HEVC tools)
DEFAULT_PROJECTS=("img_hevc" "vid_hevc")

# Helper: look up binary name by project directory
get_binary_name() {
	local project_dir="$1"
	for entry in "${ALL_PROJECTS[@]}"; do
		local dir="${entry%%:*}"
		local bin="${entry##*:}"
		if [[ "$dir" == "$project_dir" ]]; then
			echo "$bin"
			return 0
		fi
	done
	echo ""
	return 1
}

# ═══════════════════════════════════════════════════════════════
# CLI flags
# ═══════════════════════════════════════════════════════════════
FORCE_REBUILD=false
CLEAN_BUILD=false
VERBOSE=false
CLEAN_OLD_BINARIES=true
BUILD_ALL=false
DO_KONDO=false
SELECTED_PROJECTS=()

# Timestamp verification config
VERIFY_TIMESTAMPS=true
MAX_STALE_RETRIES=2     # Force full rebuild after this many verification failures

# ═══════════════════════════════════════════════════════════════
# Output helpers
# ═══════════════════════════════════════════════════════════════
print_header() {
	echo ""
	echo -e "${CYAN}${BOLD}🔧 Smart Build System v7.5${NC}"
	echo -e "${DIM}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
}

print_status() {
	local project="$1"
	local action="$2"
	local reason="$3"

	if [[ "$action" == "skip" ]]; then
		echo -e "${GREEN}✓${NC} ${BOLD}$project${NC} ${DIM}(up-to-date)${NC}"
	elif [[ "$action" == "rebuild" ]]; then
		echo -e "${YELLOW}⏳${NC} ${BOLD}$project${NC} ${DIM}($reason)${NC}"
	fi
}

print_success() {
	echo -e "${GREEN}✅${NC} ${BOLD}$1${NC} - compiled"
}

print_error() {
	echo ""
	echo -e "${RED}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
	echo -e "${RED}❌ COMPILATION FAILED: $1${NC}"
	echo -e "${RED}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
	echo ""
}

# ═══════════════════════════════════════════════════════════════
# Remove stale binaries outside target/
# ═══════════════════════════════════════════════════════════════
clean_old_binaries() {
	echo -e "${YELLOW}🧹 Cleaning old binaries...${NC}"

	local cleaned=0

	for entry in "${ALL_PROJECTS[@]}"; do
		local binary_name="${entry##*:}"
		while IFS= read -r -d '' old_binary; do
			echo -e "   ${RED}🗑️  Removing: ${DIM}$old_binary${NC}"
			rm -f "$old_binary"
			cleaned=$((cleaned + 1))
		done < <(find . -name "$binary_name" -type f -not -path "*/target/*" -print0 2>/dev/null)
	done

	if [ $cleaned -eq 0 ]; then
		echo -e "   ${GREEN}✓${NC} ${DIM}No old binaries found${NC}"
	else
		echo -e "   ${GREEN}✅ Cleaned $cleaned old binary file(s)${NC}"
	fi
	echo ""
}

# ═══════════════════════════════════════════════════════════════
# Deep cleanup via kondo
# ═══════════════════════════════════════════════════════════════
clean_with_kondo() {
	if ! command -v kondo >/dev/null 2>&1; then
		echo -e "${DIM}⚠️  kondo not found; skipping deep cleanup.${NC}"
		return 0
	fi

	echo -e "${YELLOW}🧹 Project Deep Cleanup (kondo)...${NC}"
	# Fix: removed -n (dry-run) flag so kondo actually deletes artifacts.
	# -I flags exclude Time Machine and user Library volumes.
	kondo -I /Volumes -I ~/Library .
	echo ""
}

# ═══════════════════════════════════════════════════════════════
# Timestamp helpers
# ═══════════════════════════════════════════════════════════════
get_newest_source_mtime() {
	local project_dir="$1"
	local newest=0

	# Project source files
	if [[ -d "$project_dir/src" ]]; then
		while IFS= read -r -d '' file; do
			local mtime
			mtime=$(stat -f %m "$file" 2>/dev/null || stat -c %Y "$file" 2>/dev/null || echo 0)
			[[ $mtime -gt $newest ]] && newest=$mtime
		done < <(find "$project_dir/src" -type f -name "*.rs" -print0 2>/dev/null)
	fi

	if [[ -f "$project_dir/Cargo.toml" ]]; then
		local mtime
		mtime=$(stat -f %m "$project_dir/Cargo.toml" 2>/dev/null || stat -c %Y "$project_dir/Cargo.toml" 2>/dev/null || echo 0)
		[[ $mtime -gt $newest ]] && newest=$mtime
	fi

	# Shared utility dependency
	if [[ -d "shared_utils/src" ]]; then
		while IFS= read -r -d '' file; do
			local mtime
			mtime=$(stat -f %m "$file" 2>/dev/null || stat -c %Y "$file" 2>/dev/null || echo 0)
			[[ $mtime -gt $newest ]] && newest=$mtime
		done < <(find "shared_utils/src" -type f -name "*.rs" -print0 2>/dev/null)
	fi

	# Also check shared_utils/Cargo.toml and workspace Cargo.lock
	for dep_file in "shared_utils/Cargo.toml" "Cargo.lock"; do
		if [[ -f "$dep_file" ]]; then
			local mtime
			mtime=$(stat -f %m "$dep_file" 2>/dev/null || stat -c %Y "$dep_file" 2>/dev/null || echo 0)
			[[ $mtime -gt $newest ]] && newest=$mtime
		fi
	done

	echo "$newest"
}

get_binary_mtime() {
	local binary_path="$1"
	[[ ! -f "$binary_path" ]] && echo "0" && return
	stat -f %m "$binary_path" 2>/dev/null || stat -c %Y "$binary_path" 2>/dev/null || echo 0
}

# ═══════════════════════════════════════════════════════════════
# Resolve binary path (unified workspace target directory)
# ═══════════════════════════════════════════════════════════════
get_binary_path() {
	local project_dir="$1"
	local binary_name="$2"

	if [[ -f "target/release/$binary_name" ]]; then
		echo "target/release/$binary_name"
	else
		echo ""
	fi
}

# ═══════════════════════════════════════════════════════════════
# Decide whether a project needs rebuilding
# ═══════════════════════════════════════════════════════════════
decide_build_action() {
	local project_dir="$1"
	local binary_name="$2"

	local binary_path
	binary_path=$(get_binary_path "$project_dir" "$binary_name")

	[[ "$FORCE_REBUILD" == "true" ]] && echo "rebuild:force" && return
	[[ -z "$binary_path" ]] && echo "rebuild:binary-missing" && return

	local source_mtime binary_mtime
	source_mtime=$(get_newest_source_mtime "$project_dir")
	binary_mtime=$(get_binary_mtime "$binary_path")

	[[ $source_mtime -gt $binary_mtime ]] && echo "rebuild:source-newer" && return

	echo "skip"
}

# ═══════════════════════════════════════════════════════════════
# Verify that the binary was actually updated after compilation
# ═══════════════════════════════════════════════════════════════
verify_binary_timestamp() {
	local binary_path="$1"
	local compile_start_time="$2"

	if [[ ! -f "$binary_path" ]]; then
		echo -e "${RED}⚠️  TIMESTAMP VERIFICATION FAILED: Binary not found${NC}"
		echo -e "${DIM}   Expected: $binary_path${NC}"
		return 1
	fi

	local binary_mtime
	binary_mtime=$(get_binary_mtime "$binary_path")

	# Binary mtime must be >= compilation start time
	if [[ $binary_mtime -lt $compile_start_time ]]; then
		echo -e "${RED}⚠️  TIMESTAMP VERIFICATION FAILED${NC}"
		echo -e "${DIM}   Binary mtime:  $(date -r "$binary_mtime"      '+%Y-%m-%d %H:%M:%S' 2>/dev/null || date -d @"$binary_mtime"      '+%Y-%m-%d %H:%M:%S' 2>/dev/null)${NC}"
		echo -e "${DIM}   Compile start: $(date -r "$compile_start_time" '+%Y-%m-%d %H:%M:%S' 2>/dev/null || date -d @"$compile_start_time" '+%Y-%m-%d %H:%M:%S' 2>/dev/null)${NC}"
		echo -e "${YELLOW}   ⚠️  Binary timestamp is older than compile time!${NC}"
		return 1
	fi

	return 0
}

# ═══════════════════════════════════════════════════════════════
# Compile a single project, with optional timestamp-verified retry
# ═══════════════════════════════════════════════════════════════
build_project() {
	local project_dir="$1"
	local binary_name="$2"
	local retry_count="${3:-0}"

	local compile_start_time
	compile_start_time=$(date +%s)

	if ! cargo build --release --manifest-path "$project_dir/Cargo.toml"; then
		print_error "$project_dir"
		return 1
	fi

	if [[ "$VERIFY_TIMESTAMPS" == "true" ]]; then
		local binary_path
		binary_path=$(get_binary_path "$project_dir" "$binary_name")

		if [[ -z "$binary_path" ]]; then
			echo -e "${RED}⚠️  TIMESTAMP VERIFICATION FAILED: Binary not found${NC}"
			echo -e "${DIM}   Project: $project_dir, Binary: $binary_name${NC}"
			return 1
		fi

		# Wait for filesystem to flush
		sleep 1

		if ! verify_binary_timestamp "$binary_path" "$compile_start_time"; then
			if [[ $retry_count -lt $MAX_STALE_RETRIES ]]; then
				echo -e "${YELLOW}🔄 Retry $((retry_count + 1))/$MAX_STALE_RETRIES: Rebuilding with clean...${NC}"
				# Partial clean of workspace root target (not per-project paths)
				rm -rf "target/release/deps" 2>/dev/null || true
				rm -rf "target/release/.fingerprint" 2>/dev/null || true
				# Fix: use if/return pattern so set -e doesn't swallow the failure
				if ! build_project "$project_dir" "$binary_name" $((retry_count + 1)); then
					return 1
				fi
				return 0
			else
				echo -e "${RED}❌ CRITICAL: Timestamp verification failed after $MAX_STALE_RETRIES retries${NC}"
				echo -e "${YELLOW}💡 Suggestion: Try 'cargo clean' or check file system issues${NC}"
				return 1
			fi
		fi
	fi

	return 0
}

# ═══════════════════════════════════════════════════════════════
# CLI argument parsing
# ═══════════════════════════════════════════════════════════════
parse_args() {
	while [[ $# -gt 0 ]]; do
		case "$1" in
		--kondo)
			DO_KONDO=true
			shift
			;;
		--force | -f)
			FORCE_REBUILD=true
			shift
			;;
		--clean | -c)
			CLEAN_BUILD=true
			shift
			;;
		--verbose | -v)
			VERBOSE=true
			shift
			;;
		--no-clean-old)
			CLEAN_OLD_BINARIES=false
			shift
			;;
		--all | -a)
			BUILD_ALL=true
			shift
			;;
		--hevc)
			SELECTED_PROJECTS+=("img_hevc" "vid_hevc")
			shift
			;;
		--av1)
			SELECTED_PROJECTS+=("img_av1" "vid_av1")
			shift
			;;
		--img)
			SELECTED_PROJECTS+=("img_hevc" "img_av1")
			shift
			;;
		--vid)
			SELECTED_PROJECTS+=("vid_hevc" "vid_av1")
			shift
			;;
		--no-verify-timestamps)
			VERIFY_TIMESTAMPS=false
			shift
			;;
		--help | -h)
			echo "Usage: $0 [OPTIONS]"
			echo ""
			echo "Options:"
			echo "  --force, -f       Force rebuild all selected projects"
			echo "  --clean, -c       Clean build artifacts before compiling"
			echo "  --verbose, -v     Show detailed output"
			echo "  --no-clean-old    Don't clean old binary files"
			echo "  --all, -a         Build all projects"
			echo "  --hevc            Build HEVC tools (default)"
			echo "  --av1             Build AV1 tools"
			echo "  --img             Build image tools"
			echo "  --vid             Build video tools"
			echo "  --kondo           Perform deep project cleanup using kondo"
			echo "  --no-verify-timestamps  Disable timestamp verification after build"
			echo "  --help, -h        Show this help"
			echo ""
			echo "Examples:"
			echo "  $0                # Build HEVC tools (default)"
			echo "  $0 --all          # Build all projects"
			echo "  $0 --hevc --force # Force rebuild HEVC tools"
			echo "  $0 --img --av1    # Build all image and AV1 tools"
			exit 0
			;;
		*)
			echo -e "${RED}Unknown option: $1${NC}"
			exit 1
			;;
		esac
	done
}

# ═══════════════════════════════════════════════════════════════
# Main
# ═══════════════════════════════════════════════════════════════
main() {
	parse_args "$@"
	print_header

	# Determine which projects to build
	local projects_to_build=()
	if [[ "$BUILD_ALL" == "true" ]]; then
		for entry in "${ALL_PROJECTS[@]}"; do
			projects_to_build+=("${entry%%:*}")
		done
	elif [[ ${#SELECTED_PROJECTS[@]} -gt 0 ]]; then
		projects_to_build=("${SELECTED_PROJECTS[@]}")
	else
		projects_to_build=("${DEFAULT_PROJECTS[@]}")
	fi

	# Fix: deduplicate project list to avoid double-building when flags overlap
	# (e.g. --img --av1 both add img_av1)
	local seen=()
	local deduped=()
	for proj in "${projects_to_build[@]}"; do
		local already=false
		for s in "${seen[@]}"; do
			[[ "$s" == "$proj" ]] && already=true && break
		done
		if [[ "$already" == "false" ]]; then
			seen+=("$proj")
			deduped+=("$proj")
		fi
	done
	projects_to_build=("${deduped[@]}")

	echo -e "${CYAN}📦 Building:${NC} ${BOLD}${projects_to_build[*]}${NC}"
	echo ""

	# Remove stale binaries outside target/
	if [[ "$CLEAN_OLD_BINARIES" == "true" ]]; then
		clean_old_binaries
	fi

	# Clean workspace root target/ artifacts (not per-project subdirectories)
	if [[ "$CLEAN_BUILD" == "true" ]]; then
		echo -e "${YELLOW}🧹 Cleaning build artifacts...${NC}"
		# Fix: workspace builds share a single root target/; clean that instead
		rm -rf "target/release/deps" 2>/dev/null || true
		rm -rf "target/release/.fingerprint" 2>/dev/null || true
		echo ""

		# Also run kondo deep clean when --clean is requested
		clean_with_kondo
	fi

	if [[ "$DO_KONDO" == "true" ]]; then
		clean_with_kondo
	fi

	local rebuilt=0
	local skipped=0
	local failed=0

	for proj_dir in "${projects_to_build[@]}"; do
		local binary_name
		binary_name=$(get_binary_name "$proj_dir")

		if [[ -z "$binary_name" ]]; then
			echo -e "${RED}❌ Unknown project: $proj_dir${NC}"
			failed=$((failed + 1))
			continue
		fi

		local decision
		decision=$(decide_build_action "$proj_dir" "$binary_name")
		local action="${decision%%:*}"
		local reason="${decision##*:}"

		if [[ "$action" == "skip" ]]; then
			print_status "$proj_dir" "skip" ""
			skipped=$((skipped + 1))
		else
			print_status "$proj_dir" "rebuild" "$reason"
			if build_project "$proj_dir" "$binary_name"; then
				print_success "$proj_dir"
				rebuilt=$((rebuilt + 1))
			else
				failed=$((failed + 1))
			fi
		fi
	done

	echo ""
	echo -e "${DIM}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"

	if [[ $failed -gt 0 ]]; then
		echo -e "${RED}❌ Build failed: $failed project(s)${NC}"
		exit 1
	fi

	if [[ $rebuilt -eq 0 ]]; then
		echo -e "${GREEN}✅ All binaries up-to-date (skipped $skipped)${NC}"
	else
		echo -e "${GREEN}✅ Built $rebuilt, skipped $skipped${NC}"
	fi

	# Show binary details when verbose or after a fresh build
	if [[ "$VERBOSE" == "true" ]] || [[ $rebuilt -gt 0 ]]; then
		echo ""
		echo -e "${DIM}Binary info:${NC}"
		for proj_dir in "${projects_to_build[@]}"; do
			local binary_name
			binary_name=$(get_binary_name "$proj_dir")
			[[ -z "$binary_name" ]] && continue
			local binary_path
			binary_path=$(get_binary_path "$proj_dir" "$binary_name")
			if [[ -n "$binary_path" ]] && [[ -f "$binary_path" ]]; then
				# Fix: declare size and mtime as local variables
				local size mtime
				size=$(du -h "$binary_path" | awk '{print $1}')
				mtime=$(stat -f "%Sm" -t "%Y-%m-%d %H:%M" "$binary_path" 2>/dev/null || stat -c "%y" "$binary_path" 2>/dev/null | cut -d. -f1)
				echo -e "  ${BOLD}$binary_name${NC}: $size, $mtime"
			fi
		done
	fi
}

main "$@"