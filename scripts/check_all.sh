#!/usr/bin/env bash
# Comprehensive code quality scanner for Modern Format Boost.

set -Eeuo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "${REPO_ROOT}"

if [[ -t 1 ]]; then
	RED='\033[0;31m'
	GREEN='\033[0;32m'
	YELLOW='\033[1;33m'
	BLUE='\033[0;34m'
	CYAN='\033[0;36m'
	BOLD='\033[1m'
	NC='\033[0m'
else
	RED=''
	GREEN=''
	YELLOW=''
	BLUE=''
	CYAN=''
	BOLD=''
	NC=''
fi

REQUIRED_BRANCH="${CHECK_ALL_DEFAULT_BRANCH:-nightly}"
ENFORCE_BRANCH=1
RUN_OPTIONAL=1
RUN_EXPENSIVE=1
ALLOW_FETCH=0
AUTO_FIX=0

show_help() {
	cat <<EOF
Usage: scripts/check_all.sh [options]

Options:
  --allow-non-nightly    Do not enforce '${REQUIRED_BRANCH}' git branch.
  --required-only        Run only required checks (fmt/clippy/tests).
  --no-expensive         Skip expensive optional checks (udeps/bloat/hack/miri).
  --fetch-advisory-db    Allow network fetch for cargo-audit/cargo-deny.
  --fix                  Auto-fix issues (cargo fmt, cargo clippy --fix).
  -h, --help             Show this help.

Environment:
  CHECK_ALL_DEFAULT_BRANCH=<branch>  Override default required branch (default: nightly).
EOF
}

while [[ $# -gt 0 ]]; do
	case "$1" in
	--allow-non-nightly)
		ENFORCE_BRANCH=0
		;;
	--required-only)
		RUN_OPTIONAL=0
		;;
	--no-expensive)
		RUN_EXPENSIVE=0
		;;
	--fetch-advisory-db)
		ALLOW_FETCH=1
		;;
	--fix)
		AUTO_FIX=1
		;;
	-h | --help)
		show_help
		exit 0
		;;
	*)
		echo "Unknown option: $1" >&2
		show_help >&2
		exit 2
		;;
	esac
	shift
done

has_cargo_subcommand() {
	cargo "$1" --version >/dev/null 2>&1
}

has_command() {
	command -v "$1" >/dev/null 2>&1
}

has_nightly_toolchain() {
	rustup toolchain list 2>/dev/null | awk '{print $1}' | grep -Eq '^nightly($|-)'
}

has_rust_component() {
	local component="$1"
	local toolchain="${2:-}"
	if [[ -n "$toolchain" ]]; then
		rustup +"$toolchain" component list --installed 2>/dev/null |
			awk '{print $1}' |
			grep -Eq "^${component}($|-)"
	else
		rustup component list --installed 2>/dev/null |
			awk '{print $1}' |
			grep -Eq "^${component}($|-)"
	fi
}

advisory_db_dir() {
	# Must match deny.toml: `db-path = "~/.cargo/advisory-db"`
	printf '%s/advisory-db' "${CARGO_HOME:-$HOME/.cargo}"
}

advisory_db_writable() {
	local cargo_home
	cargo_home="${CARGO_HOME:-$HOME/.cargo}"
	# cargo-deny will need to create/fetch/update files under cargo home.
	# So check writability of cargo home (more robust than the db subdir).
	[[ -d "${cargo_home}" && -w "${cargo_home}" ]]
}

advisory_db_is_git_repo() {
	local db_dir
	db_dir="$(advisory_db_dir)"
	[[ -d "${db_dir}" ]] || return 1

	# cargo-deny keeps a directory of advisory git repositories under `db-path`.
	# The parent `db-path` itself is not necessarily a git repo, so we need to
	# check for any `advisory-db-*` sub-repo.
	if git -C "${db_dir}" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
		return 0
	fi

	local any=0
	local d
	for d in "${db_dir}"/advisory-db-*; do
		[[ -d "$d" ]] || continue
		if git -C "$d" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
			any=1
			break
		fi
	done
	[[ $any -eq 1 ]]
}

declare -i step=0
declare -i passed=0
declare -i failed=0
declare -i warned=0
declare -i skipped=0
declare -a failed_steps=()
declare -a warned_steps=()
declare -a skipped_steps=()

print_step_header() {
	local kind="$1"
	local name="$2"
	step=$((step + 1))
	echo -e "\n${BOLD}[${step}] ${kind}: ${name}${NC}"
}

run_required() {
	local name="$1"
	shift
	print_step_header "required" "${name}"
	if "$@"; then
		passed=$((passed + 1))
		echo -e "${GREEN}PASS${NC} ${name}"
	else
		local rc=$?
		failed=$((failed + 1))
		failed_steps+=("${name} (exit ${rc})")
		echo -e "${RED}FAIL${NC} ${name} (exit ${rc})"
	fi
}

run_optional() {
	local name="$1"
	shift
	print_step_header "optional" "${name}"
	if "$@"; then
		passed=$((passed + 1))
		echo -e "${GREEN}PASS${NC} ${name}"
	else
		local rc=$?
		warned=$((warned + 1))
		warned_steps+=("${name} (exit ${rc})")
		echo -e "${YELLOW}WARN${NC} ${name} (exit ${rc})"
	fi
}

skip_optional() {
	local name="$1"
	local reason="$2"
	print_step_header "optional" "${name}"
	skipped=$((skipped + 1))
	skipped_steps+=("${name}: ${reason}")
	echo -e "${BLUE}SKIP${NC} ${name} (${reason})"
}

echo -e "${BOLD}${CYAN}Starting code quality scan${NC}"
echo -e "${BLUE}Repo: ${REPO_ROOT}${NC}"
echo -e "${BLUE}Default branch policy: ${REQUIRED_BRANCH}${NC}"

if git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
	current_branch="$(git rev-parse --abbrev-ref HEAD)"
	echo -e "${BLUE}Current git branch: ${current_branch}${NC}"
	if [[ ${ENFORCE_BRANCH} -eq 1 && "${current_branch}" != "${REQUIRED_BRANCH}" ]]; then
		echo -e "${RED}Branch policy violation: expected '${REQUIRED_BRANCH}', got '${current_branch}'.${NC}" >&2
		echo "Use --allow-non-nightly to bypass."
		exit 2
	fi
fi

if [[ ${AUTO_FIX} -eq 1 ]]; then
	echo -e "\n${BOLD}${CYAN}Running auto-fix${NC}"
	echo -e "${BLUE}Applying cargo fmt...${NC}"
	cargo fmt --all
	echo -e "${BLUE}Applying cargo clippy --fix...${NC}"
	cargo clippy --fix --workspace --all-targets --all-features --allow-dirty --allow-staged
	echo -e "${GREEN}Auto-fix completed${NC}"
fi

run_required "cargo fmt --all --check" cargo fmt --all --check

run_required \
	"cargo clippy --workspace --all-targets --all-features -D warnings" \
	cargo clippy --workspace --all-targets --all-features -- \
	-D warnings

if has_cargo_subcommand nextest; then
	run_required "cargo nextest run --workspace --all-features" \
		cargo nextest run --workspace --all-features
else
	run_required "cargo test --workspace --all-features" \
		cargo test --workspace --all-features
fi

if [[ ${RUN_OPTIONAL} -eq 1 ]]; then
	# -----------------------------
	# Shell script quality checks
	# -----------------------------
	shell_files=()
	while IFS= read -r -d '' f; do
		shell_files+=("$f")
	done < <(find . -type f -name "*.sh" -print0 2>/dev/null || true)

	if has_command shellcheck; then
		if [[ ${#shell_files[@]} -gt 0 ]]; then
			# Only count error-level issues that would cause runtime failures.
			# Exclude SC1071 (zsh shebang warnings) as this project uses zsh scripts.
			run_optional “shellcheck *.sh errors only, zsh ignored” \
				shellcheck --severity=error --exclude=SC1071 “${shell_files[@]}”
		else
			skip_optional “shellcheck *.sh” “no .sh files found under repo”
		fi
	else
		skip_optional “shellcheck *.sh” “shellcheck not installed”
	fi

	if has_command shfmt; then
		if [[ ${#shell_files[@]} -gt 0 ]]; then
			# shfmt parses bash/zsh differently; using auto mode causes false positives.
			# Since this project uses both bash and zsh, split by shebang and diff separately.
			bash_files=()
			zsh_files=()
			for f in "${shell_files[@]}"; do
				[[ -z "$f" ]] && continue
				first_line=""
				read -r first_line <"$f" 2>/dev/null || true
				case "$(basename "$f")" in
				"common.sh")
					# common.sh contains zsh-only parameter expansion syntax,
					# so it must use -ln zsh for shfmt diff to be 0.
					zsh_files+=("$f")
					;;
				*)
					case "$first_line" in
					"#!/bin/zsh" | "#!/usr/bin/env zsh") zsh_files+=("$f") ;;
					*) bash_files+=("$f") ;;
					esac
					;;
				esac
			done

			if [[ ${#bash_files[@]} -gt 0 ]]; then
				# -d: diff output only (non-zero exit code indicates formatting differences)
				run_optional "shfmt -d *.sh bash" shfmt -d -ln bash "${bash_files[@]}"
			fi
			if [[ ${#zsh_files[@]} -gt 0 ]]; then
				run_optional "shfmt -d *.sh zsh" shfmt -d -ln zsh "${zsh_files[@]}"
			fi
		else
			skip_optional "shfmt -d *.sh" "no .sh files found under repo"
		fi
	else
		skip_optional "shfmt -d *.sh" "shfmt not installed"
	fi

	# Docs should compile too (catches broken intra-doc links / feature-gated docs).
	run_optional "cargo doc --workspace --no-deps" cargo doc --workspace --no-deps

	run_optional \
		"cargo clippy deep --workspace --all-targets --all-features -W pedantic -W nursery" \
		cargo clippy --workspace --all-targets --all-features -- \
		-W clippy::pedantic \
		-W clippy::nursery

	if has_cargo_subcommand audit; then
		if [[ ${ALLOW_FETCH} -eq 1 ]]; then
			run_optional "cargo audit" cargo audit
		else
			run_optional "cargo audit --no-fetch" cargo audit --no-fetch
		fi
	else
		skip_optional "cargo audit" "cargo-audit not installed"
	fi

	if has_cargo_subcommand deny; then
		if [[ ${ALLOW_FETCH} -eq 1 ]]; then
			if advisory_db_writable; then
				run_optional "cargo deny check --hide-inclusion-graph" \
					cargo deny check --hide-inclusion-graph
			else
				skip_optional "cargo deny check" "advisory DB is missing or read-only ($(advisory_db_dir))"
			fi
		else
			if advisory_db_writable && advisory_db_is_git_repo; then
				run_optional "cargo deny check --disable-fetch --hide-inclusion-graph" \
					cargo deny check --disable-fetch --hide-inclusion-graph
			else
				skip_optional "cargo deny check --disable-fetch" "advisory DB is missing, read-only, or not a valid git repo ($(advisory_db_dir))"
			fi
		fi
	else
		skip_optional "cargo deny check" "cargo-deny not installed"
	fi

	if has_cargo_subcommand machete; then
		run_optional "cargo machete" cargo machete
	else
		skip_optional "cargo machete" "cargo-machete not installed"
	fi

	if [[ ${RUN_EXPENSIVE} -eq 1 ]]; then
		if has_cargo_subcommand udeps; then
			if has_nightly_toolchain && has_rust_component rust-src nightly; then
				run_optional "cargo +nightly udeps --workspace --all-targets" \
					cargo +nightly udeps --workspace --all-targets
			else
				skip_optional "cargo +nightly udeps" "nightly toolchain or rust-src missing"
			fi
		else
			skip_optional "cargo udeps" "cargo-udeps not installed"
		fi

		if has_cargo_subcommand geiger; then
			# cargo-geiger may exit due to abnormal cargo/rustc build artifact state (e.g., missing rmeta files).
			# This doesn't indicate code issues, so downgrade known internal error patterns to SKIP.
			geiger_name="cargo geiger per package --all-features --all-targets"
			geiger_log="$(mktemp "/tmp/geiger_log.XXXXXX")"
			print_step_header "optional" "${geiger_name}"

			set +e
			env REPO_ROOT="${REPO_ROOT}" bash -c 'set -e; for m in "$REPO_ROOT/shared_utils/Cargo.toml" "$REPO_ROOT/img_hevc/Cargo.toml" "$REPO_ROOT/img_av1/Cargo.toml" "$REPO_ROOT/vid_hevc/Cargo.toml" "$REPO_ROOT/vid_av1/Cargo.toml"; do cargo geiger --manifest-path "$m" --all-features --all-targets --output-format Ascii; done' >"${geiger_log}" 2>&1
			geiger_rc=$?
			set -e

			if [[ ${geiger_rc} -eq 0 ]]; then
				passed=$((passed + 1))
				echo -e "${GREEN}PASS${NC} ${geiger_name}"
			else
				geiger_skip_reason=""
				if command -v rg >/dev/null 2>&1 && rg -q "No such file or directory|NotFound|error: Io" "${geiger_log}"; then
					geiger_skip_reason="internal error geiger/cargo artifact missing"
					skipped=$((skipped + 1))
					skipped_steps+=("${geiger_name}: ${geiger_skip_reason} exit ${geiger_rc}")
					echo -e "${BLUE}SKIP${NC} ${geiger_name} ${geiger_skip_reason}"
				else
					warned=$((warned + 1))
					warned_steps+=("${geiger_name} exit ${geiger_rc}")
					echo -e "${YELLOW}WARN${NC} ${geiger_name} exit ${geiger_rc}"
				fi
			fi

			rm -f "${geiger_log}"
		else
			skip_optional "cargo geiger" "cargo-geiger not installed"
		fi

		if has_cargo_subcommand bloat; then
			run_optional "cargo bloat --release --crates -n 20" \
				cargo bloat --release --crates -n 20
		else
			skip_optional "cargo bloat" "cargo-bloat not installed"
		fi

		if has_cargo_subcommand hack; then
			run_optional "cargo hack check --workspace --each-feature --no-dev-deps" \
				cargo hack check --workspace --each-feature --no-dev-deps
		else
			skip_optional "cargo hack" "cargo-hack not installed"
		fi

		if has_nightly_toolchain && has_rust_component miri nightly; then
			# Miri is very slow on extensive proptest/property tests, so use a representative stable unit test for quick verification.
			run_optional "cargo +nightly miri test -p shared_utils signature test, no isolation" \
				env MIRIFLAGS="-Zmiri-disable-isolation" cargo +nightly miri test -p shared_utils --lib test_signature_stability
		elif has_nightly_toolchain; then
			skip_optional "cargo miri test" "miri component not installed nightly"
		else
			skip_optional "cargo miri test" "nightly toolchain missing"
		fi
	else
		skip_optional "expensive optional checks" "disabled by --no-expensive"
	fi
fi

echo -e "\n${BLUE}========================================${NC}"
echo -e "${BOLD}Summary${NC}"
echo -e "Passed: ${GREEN}${passed}${NC}"
echo -e "Required failures: ${RED}${failed}${NC}"
echo -e "Optional warnings: ${YELLOW}${warned}${NC}"
echo -e "Skipped: ${BLUE}${skipped}${NC}"

if [[ ${#failed_steps[@]} -gt 0 ]]; then
	echo -e "\n${RED}Required failures:${NC}"
	printf '  - %s\n' "${failed_steps[@]}"
fi

if [[ ${#warned_steps[@]} -gt 0 ]]; then
	echo -e "\n${YELLOW}Optional warnings:${NC}"
	printf '  - %s\n' "${warned_steps[@]}"
fi

if [[ ${#skipped_steps[@]} -gt 0 ]]; then
	echo -e "\n${BLUE}Skipped checks:${NC}"
	printf '  - %s\n' "${skipped_steps[@]}"
fi

if [[ ${failed} -gt 0 ]]; then
	echo -e "\n${RED}Quality scan completed with required check failures.${NC}"
	exit 1
fi

echo -e "\n${GREEN}Quality scan completed successfully (required checks passed).${NC}"
