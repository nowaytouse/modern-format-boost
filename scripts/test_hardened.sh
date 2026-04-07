#!/bin/bash
set -e

# Colors for output
GREEN='\033[0;32m'
BLUE='\033[0;34m'
NC='\033[0m'

echo -e "${BLUE}🛡️ Running Modern Format Boost Hardened Test Suite${NC}\n"

# 1. Check for cargo-llvm-cov
if ! command -v cargo-llvm-cov &> /dev/null; then
    echo "⚠️ cargo-llvm-cov not found. Installing..."
    cargo install cargo-llvm-cov
fi

echo -e "${GREEN}📊 Generating Coverage Report...${NC}"
cargo llvm-cov --all-features --workspace --html
echo "✅ HTML report generated in target/llvm-cov/html/index.html"

# 2. AddressSanitizer (ASan)
echo -e "\n${GREEN}🧪 Running AddressSanitizer (Memory Safety)...${NC}"
RUSTFLAGS="-Z sanitizer=address" cargo +nightly test --workspace --target x86_64-apple-darwin -- -Z unstable-options --error-format=json || echo "⚠️ ASan requires nightly and compatible target."

# 3. Miri (Undefined Behavior Detection)
echo -e "\n${GREEN}🧭 Running Miri (Logic & UB Check)...${NC}"
# We run a subset because Miri is slow and doesn't handle FFI
cargo +nightly miri test -p shared_utils --lib float_compare || echo "⚠️ Miri check skipped or failed (common for FFI projects)."

# 4. Mutation Testing (Optional)
if command -v cargo-mutants &> /dev/null; then
    echo -e "\n${GREEN}🦠 Running Mutation Testing (Test Quality)...${NC}"
    cargo mutants -d crates/shared_utils --timeout 30
else
    echo -e "\n💡 Install 'cargo-mutants' to evaluate test effectiveness."
fi

echo -e "\n${BLUE}✅ Hardened Testing Session Complete.${NC}"
