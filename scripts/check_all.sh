#!/bin/bash
set -e
cargo fmt --check
cargo clippy -- -W clippy::pedantic -W clippy::nursery -D warnings
cargo audit
cargo deny check
cargo machete
cargo udeps
cargo geiger 2>/dev/null | tail -20
cargo nextest run

# Rustup 
#rustup component add clippy miri rust-src rustfmt rust-analyzer && \

# Cargo 
#cargo install cargo-audit cargo-deny cargo-geiger cargo-udeps cargo-bloat \
#  cargo-semver-checks cargo-expand cargo-hack cargo-machete cargo-flamegraph \
#  cargo-nextest cargo-mutants