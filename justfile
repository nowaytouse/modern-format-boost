# Modern Format Boost — command SSOT for local autofix and CI quick gates.
#
# Local:  just fix     — fmt + clippy --fix (push after reviewing diff)
# CI:     just check    — read-only fmt + clippy (same as check_all clippy step)
# CI:     just fix-gate — compatibility alias for read-only check gate
# Heavy:  cargo run --locked -p dev --bin check_all -- --ci

default:
    @just --list

# Format only (read-only).
fmt-check:
    cargo fmt --all -- --check

# Format only (write).
fmt-fix:
    cargo fmt --all

# Clippy ultra-strict (read-only; delegates to Rust clippy_strict bin).
clippy-check:
    cargo run --locked -p dev --bin clippy_strict

# Clippy ultra-strict autofix (slow on full workspace).
clippy-fix:
    cargo run --locked -p dev --bin clippy_strict -- --fix

# Local one-shot: format + clippy autofix.
fix: fmt-fix clippy-fix

# Pre-push / lightweight CI: fmt + clippy check-only.
check: fmt-check clippy-check

# CI gate: read-only fmt + clippy; fail when committed sources are not clean.
fix-gate:
    @cargo run --locked -p dev --bin just_fix_gate
