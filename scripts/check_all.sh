#!/bin/bash
# Deep Code Quality Scanning Script for Modern Format Boost
# Based on debug/logs/check suggestions

set -e

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
BOLD='\033[1m'
NC='\033[0m' # No Color

echo -e "${BOLD}${CYAN}🚀 Starting Triple-Deep Code Quality Scan...${NC}"
echo -e "${BLUE}======================================================${NC}"

# Check for required components/tools
check_tool() {
    if ! command -v "$1" &> /dev/null; then
        echo -e "${YELLOW}⚠️  Tool '$1' not found. Skipping. (Try: cargo install $1)${NC}"
        return 1
    fi
    return 0
}

# 1. Format Check
echo -e "\n${BOLD}[1/11] Checking code formatting...${NC}"
if cargo fmt --version &> /dev/null; then
    cargo fmt --check && echo -e "${GREEN}✅ Formatting is correct.${NC}" || echo -e "${RED}❌ Formatting issues found! Run 'cargo fmt' to fix.${NC}"
else
    echo -e "${YELLOW}⚠️  rustfmt not found. Skipping.${NC}"
fi

# 2. Deep Clippy Audit
echo -e "\n${BOLD}[2/11] Running Deep Clippy Audit (Pedantic & Nursery)...${NC}"
cargo clippy --workspace --all-targets --all-features -- \
    -W clippy::pedantic \
    -W clippy::nursery \
    -D warnings \
    && echo -e "${GREEN}✅ Clippy audit passed.${NC}" || echo -e "${RED}❌ Clippy found issues!${NC}"

# 3. Security Audit
echo -e "\n${BOLD}[3/11] Running Security Audit (CVEs)...${NC}"
if check_tool cargo-audit; then
    cargo audit && echo -e "${GREEN}✅ No known vulnerabilities.${NC}" || echo -e "${RED}❌ Vulnerabilities found!${NC}"
fi

# 4. Dependency & License Deny
echo -e "\n${BOLD}[4/11] Running Dependency & License Checks...${NC}"
if check_tool cargo-deny; then
    cargo deny check && echo -e "${GREEN}✅ Dependency and license checks passed.${NC}" || echo -e "${RED}❌ Deny checks failed!${NC}"
fi

# 5. Unused Dependency Check (Machete - Fast)
echo -e "\n${BOLD}[5/11] Checking for unused dependencies (Machete)...${NC}"
if check_tool cargo-machete; then
    cargo machete && echo -e "${GREEN}✅ No obvious unused dependencies.${NC}" || echo -e "${YELLOW}⚠️  Check machete output above.${NC}"
fi

# 6. Unused Dependency Check (Udeps - Deep)
echo -e "\n${BOLD}[6/11] Checking for unused dependencies (Udeps - Deep)...${NC}"
if check_tool cargo-udeps; then
    # udeps often needs nightly
    cargo +nightly udeps --all-targets && echo -e "${GREEN}✅ Udeps check passed.${NC}" || echo -e "${YELLOW}⚠️  Udeps flagged potential issues.${NC}"
fi

# 7. Unsafe Code Analysis (Geiger)
echo -e "\n${BOLD}[7/11] Analyzing Unsafe Code Usage...${NC}"
if check_tool cargo-geiger; then
    cargo geiger --output-format ascii 2>/dev/null | tail -20 && echo -e "${GREEN}✅ Geiger analysis complete.${NC}" || echo -e "${YELLOW}⚠️  Geiger analysis failed.${NC}"
fi

# 8. Binary Size Analysis (Bloat)
echo -e "\n${BOLD}[8/11] Analyzing Binary Size (Bloat)...${NC}"
if check_tool cargo-bloat; then
    cargo bloat --release -n 20 && echo -e "${GREEN}✅ Size analysis complete.${NC}" || echo -e "${YELLOW}⚠️  Size analysis failed.${NC}"
fi

# 9. Feature Combination Check (Hack)
echo -e "\n${BOLD}[9/11] Testing feature flag combinations...${NC}"
if check_tool cargo-hack; then
    cargo hack check --each-feature --no-dev-deps && echo -e "${GREEN}✅ All feature combinations compile.${NC}" || echo -e "${RED}❌ Feature mismatch found!${NC}"
fi

# 10. Memory Protection (Miri - Optional)
echo -e "\n${BOLD}[10/11] Checking for Undefined Behavior (Miri)...${NC}"
if rustup toolchain list | grep -q nightly; then
    echo -e "${BLUE}ℹ️  Running Miri on core logic...${NC}"
    cargo +nightly miri test --package shared_utils --lib ssim --ignore-leaks 2>/dev/null && echo -e "${GREEN}✅ Miri checks passed (core logic).${NC}" || echo -e "${YELLOW}⚠️  Miri flagged issues or skipped (use --verbose to debug).${NC}"
else
    echo -e "${YELLOW}⚠️  Nightly toolchain not found. Skipping Miri.${NC}"
fi

# 11. Optimized Testing (Nextest)
echo -e "\n${BOLD}[11/11] Running tests (Nextest)...${NC}"
if check_tool cargo-nextest; then
    cargo nextest run --workspace --all-features && echo -e "${GREEN}✅ All tests passed.${NC}" || echo -e "${RED}❌ Tests failed!${NC}"
else
    echo -e "${BLUE}ℹ️  Nextest not found, falling back to standard cargo test...${NC}"
    cargo test --workspace --all-features && echo -e "${GREEN}✅ All tests passed.${NC}" || echo -e "${RED}❌ Tests failed!${NC}"
fi

echo -e "\n${BLUE}======================================================${NC}"
echo -e "${BOLD}${GREEN}✨ COMPLETE Quality Scan Finished!${NC}"
echo -e "${CYAN}Note: Project Integrity Check was REMOVED as requested.${NC}"
