#!/usr/bin/env bash
# Smart Build System v7.5 - Intelligent Selective Build
#
# 🔥 v7.5 New Features:
# - ✅ Post-build timestamp verification (Ensures binary was truly updated)
# - ✅ Automatic force rebuild on multiple verification failures
# - ✅ Loud error reporting (Compilation errors MUST notify user)
# 🔥 v7.4.1 Fixes:
# - ✅ Compatibility with macOS bash 3.x (Removed associative arrays)
# 🔥 v7.4 Features:
# - ✅ Selective build (Build only required projects)
# - ✅ Intelligent cleanup of obsolete binaries
# - ✅ Intelligent timestamp comparison
# - ✅ Force rebuild option
# - ✅ Accurate path handling
# (Merged common.sh dependencies)

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
cd "$PROJECT_ROOT"

# ═══════════════════════════════════════════════════════════════
# Color Definitions
# ═══════════════════════════════════════════════════════════════
if [[ -t 1 ]]; then
	RED='\033[38;5;196m'
	GREEN='\033[38;5;46m'
	YELLOW='\033[38;5;226m'
	CYAN='\033[38;5;51m'
	BOLD='\033[1m'
	DIM='\033[2m'
	NC='\033[0m'
else
	RED=''
	GREEN=''
	YELLOW=''
	CYAN=''
	BOLD=''
	DIM=''
	NC=''
fi

# ═══════════════════════════════════════════════════════════════
# Project Configuration - bash 3.x compatible
# ═══════════════════════════════════════════════════════════════
# Format: "project_dir:binary_name"
ALL_PROJECTS=(
	"img_hevc:img-hevc"
	"vid_hevc:vid-hevc"
	"img_av1:img-av1"
	"vid_av1:vid-av1"
)

# Default projects to build (HEVC tools)
DEFAULT_PROJECTS=("img_hevc" "vid_hevc")

# Helper function: Get binary name from project directory
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

# CLI Arguments
FORCE_REBUILD=false
CLEAN_BUILD=false
VERBOSE=false
CLEAN_OLD_BINARIES=true
BUILD_ALL=false
SELECTED_PROJECTS=()

# 🔥 v7.5: Timestamp Verification Config
VERIFY_TIMESTAMPS=true
MAX_STALE_RETRIES=2 # Allow up to 2 timestamp verification failures, force rebuild on the 3rd

# ═══════════════════════════════════════════════════════════════
# Output Functions
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
# 🔥 v7.4: Intelligent obsolete binary cleanup
# ═══════════════════════════════════════════════════════════════
clean_old_binaries() {
	echo -e "${YELLOW}🧹 Cleaning old binaries...${NC}"

	local cleaned=0

	# Find and remove all old binaries (not in target/release)
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
# 🔥 v8.3: Kondo Deep Cleanup
# ═══════════════════════════════════════════════════════════════
clean_with_kondo() {
	if ! command -v kondo >/dev/null 2>&1; then
		echo -e "${DIM}⚠️  kondo not found; skipping deep cleanup.${NC}"
		return 0
	fi

	echo -e "${YELLOW}🧹 Project Deep Cleanup (kondo)...${NC}"
	# Use safe parameters: Clean current project only, exclude Time Machine volumes and libraries
	kondo -n -I /Volumes -I ~/Library .
	echo ""
}

# ═══════════════════════════════════════════════════════════════
# Timestamp Functions
# ═══════════════════════════════════════════════════════════════
get_newest_source_mtime() {
	local project_dir="$1"
	local newest=0

	# Project source code
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

	# shared_utils dependencies
	if [[ -d "shared_utils/src" ]]; then
		while IFS= read -r -d '' file; do
			local mtime
			mtime=$(stat -f %m "$file" 2>/dev/null || stat -c %Y "$file" 2>/dev/null || echo 0)
			[[ $mtime -gt $newest ]] && newest=$mtime
		done < <(find "shared_utils/src" -type f -name "*.rs" -print0 2>/dev/null)
	fi

	# 🔥 v8.2.4: Also check shared_utils/Cargo.toml and workspace Cargo.lock
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
# Build Decision Logic
# ═══════════════════════════════════════════════════════════════
decide_build_action() {
	local project_dir="$1"
	local binary_name="$2"

	# 🔥 v7.5: Use get_binary_path to locate the correct executable
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
# 🔥 v7.5: Timestamp Verification Functions
# ═══════════════════════════════════════════════════════════════
get_binary_path() {
	local project_dir="$1"
	local binary_name="$2"

	# 🔥 v8.3: Unified workspace target directory
	if [[ -f "target/release/$binary_name" ]]; then
		echo "target/release/$binary_name"
	else
		echo ""
	fi
}

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

	# Binary modification time should be >= compile start time
	if [[ $binary_mtime -lt $compile_start_time ]]; then
		echo -e "${RED}⚠️  TIMESTAMP VERIFICATION FAILED${NC}"
		echo -e "${DIM}   Binary mtime: $(date -r "$binary_mtime" '+%Y-%m-%d %H:%M:%S' 2>/dev/null || date -d @"$binary_mtime" '+%Y-%m-%d %H:%M:%S' 2>/dev/null)${NC}"
		echo -e "${DIM}   Compile start: $(date -r "$compile_start_time" '+%Y-%m-%d %H:%M:%S' 2>/dev/null || date -d @"$compile_start_time" '+%Y-%m-%d %H:%M:%S' 2>/dev/null)${NC}"
		echo -e "${YELLOW}   ⚠️  Binary timestamp is older than compile time!${NC}"
		return 1
	fi

	return 0
}

# ═══════════════════════════════════════════════════════════════
# Build Functions
# ═══════════════════════════════════════════════════════════════
build_project() {
	local project_dir="$1"
	local binary_name="$2"
	local retry_count="${3:-0}"

	# Record compilation start time
	local compile_start_time
	compile_start_time=$(date +%s)

	# 🔥 Fix: Handled cargo output and return codes correctly
	if ! cargo build --release --manifest-path "$project_dir/Cargo.toml"; then
		print_error "$project_dir"
		return 1
	fi

	# 🔥 v7.5: Post-build timestamp verification
	if [[ "$VERIFY_TIMESTAMPS" == "true" ]]; then
		local binary_path
		binary_path=$(get_binary_path "$project_dir" "$binary_name")

		if [[ -z "$binary_path" ]]; then
			echo -e "${RED}⚠️  TIMESTAMP VERIFICATION FAILED: Binary not found${NC}"
			echo -e "${DIM}   Project: $project_dir, Binary: $binary_name${NC}"
			return 1
		fi

		# Wait for 1 second to ensure filesystem synchronization
		sleep 1

		if ! verify_binary_timestamp "$binary_path" "$compile_start_time"; then
			# Timestamp verification failed
			if [[ $retry_count -lt $MAX_STALE_RETRIES ]]; then
				echo -e "${YELLOW}🔄 Retry $((retry_count + 1))/$MAX_STALE_RETRIES: Rebuilding with clean...${NC}"
				# Cleanup and retry
				# 🔥 v8.3: Only clean root target
				rm -rf "target/release/deps" 2>/dev/null || true
				rm -rf "target/release/.fingerprint" 2>/dev/null || true
				build_project "$project_dir" "$binary_name" $((retry_count + 1))
				return $?
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
# CLI Parameter Parsing
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
			echo "  $0 --img --av1    # Build AV1 image tools"
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
# Main Function
# ═══════════════════════════════════════════════════════════════
main() {
	parse_args "$@"
	print_header

	# Determine projects to build
	local projects_to_build=()
	if [[ "$BUILD_ALL" == "true" ]]; then
		# Build all projects - Extract project directory names
		for entry in "${ALL_PROJECTS[@]}"; do
			projects_to_build+=("${entry%%:*}")
		done
	elif [[ ${#SELECTED_PROJECTS[@]} -gt 0 ]]; then
		projects_to_build=("${SELECTED_PROJECTS[@]}")
	else
		projects_to_build=("${DEFAULT_PROJECTS[@]}")
	fi

	echo -e "${CYAN}📦 Building:${NC} ${BOLD}${projects_to_build[*]}${NC}"
	echo ""

	# Cleanup old binaries
	if [[ "$CLEAN_OLD_BINARIES" == "true" ]]; then
		clean_old_binaries
	fi

	# Cleanup build artifacts
	if [[ "$CLEAN_BUILD" == "true" ]]; then
		echo -e "${YELLOW}🧹 Cleaning build artifacts...${NC}"
		for proj_dir in "${projects_to_build[@]}"; do
			rm -rf "$proj_dir/target/release/deps" 2>/dev/null || true
			rm -rf "$proj_dir/target/release/.fingerprint" 2>/dev/null || true
		done
		rm -rf "shared_utils/target/release/deps" 2>/dev/null || true
		echo ""

		# 🔥 v8.3: Auto-trigger kondo cleanup
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

	# Show binary information
	if [[ "$VERBOSE" == "true" ]] || [[ $rebuilt -gt 0 ]]; then
		echo ""
		echo -e "${DIM}Binary info:${NC}"
		for proj_dir in "${projects_to_build[@]}"; do
			local binary_name
			binary_name=$(get_binary_name "$proj_dir")
			if [[ -z "$binary_name" ]]; then
				continue
			fi
			local binary_path
			binary_path=$(get_binary_path "$proj_dir" "$binary_name")
			if [[ -n "$binary_path" ]] && [[ -f "$binary_path" ]]; then
				size=$(du -h "$binary_path" | awk '{print $1}')
				mtime=$(stat -f "%Sm" -t "%Y-%m-%d %H:%M" "$binary_path" 2>/dev/null || stat -c "%y" "$binary_path" 2>/dev/null | cut -d. -f1)
				echo -e "  ${BOLD}$binary_name${NC}: $size, $mtime"
			fi
		done
	fi
}

main "$@"
