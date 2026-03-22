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

show_help() {
    cat <<EOF
Usage: scripts/check_all.sh [options]

Options:
  --allow-non-nightly    Do not enforce '${REQUIRED_BRANCH}' git branch.
  --required-only        Run only required checks (fmt/clippy/tests).
  --no-expensive         Skip expensive optional checks (udeps/bloat/hack/miri).
  --fetch-advisory-db    Allow network fetch for cargo-audit/cargo-deny.
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
        -h|--help)
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

has_nightly_toolchain() {
    rustup toolchain list 2>/dev/null | awk '{print $1}' | grep -Eq '^nightly($|-)'
}

has_rust_component() {
    rustup component list --installed 2>/dev/null | awk '{print $1}' | grep -Eq "^$1($|-)"
}

advisory_db_dir() {
    printf '%s/advisory-dbs' "${CARGO_HOME:-$HOME/.cargo}"
}

advisory_db_writable() {
    local db_dir
    db_dir="$(advisory_db_dir)"
    [[ -d "${db_dir}" && -w "${db_dir}" ]]
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
    run_optional \
        "cargo clippy (deep) --workspace --all-targets --all-features -W pedantic -W nursery" \
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
            if advisory_db_writable; then
                run_optional "cargo deny check --disable-fetch --hide-inclusion-graph" \
                    cargo deny check --disable-fetch --hide-inclusion-graph
            else
                skip_optional "cargo deny check --disable-fetch" "advisory DB is missing or read-only ($(advisory_db_dir))"
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
            if has_nightly_toolchain && has_rust_component rust-src; then
                run_optional "cargo +nightly udeps --workspace --all-targets" \
                    cargo +nightly udeps --workspace --all-targets
            else
                skip_optional "cargo +nightly udeps" "nightly toolchain or rust-src missing"
            fi
        else
            skip_optional "cargo udeps" "cargo-udeps not installed"
        fi

        if has_cargo_subcommand geiger; then
            run_optional "cargo geiger --all-features --all-targets --output-format Ascii" \
                cargo geiger --all-features --all-targets --output-format Ascii
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

        if has_cargo_subcommand miri; then
            if has_nightly_toolchain; then
                run_optional "cargo +nightly miri test -p shared_utils --lib" \
                    cargo +nightly miri test -p shared_utils --lib
            else
                skip_optional "cargo miri test" "nightly toolchain missing"
            fi
        else
            skip_optional "cargo miri test" "miri component not installed"
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
