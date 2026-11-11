

## MFB native AppKit design verification

For native macOS Swift/AppKit GUI changes, use `$mfb-appkit-design-qa` when applicable. Prefer Apple's native UI and accessibility behavior. Do not substitute browser-based tests for AppKit verification.

## Private code index and QA

A private index may be available at `${XDG_STATE_HOME:-~/.local/state}/project-indexes/<checkout-basename>/latest.json`. Verify its full Git HEAD and timestamp before use; rebuild from current committed source when stale. Keep index output private and uncommitted. Use synthetic fixtures for privacy and QA; exclude vaults, logs, and real dumps.

## macOS App builds

Use `cargo run --locked --release -p dev --bin smart_build -- --gui` for an incremental refresh of the repository's native App. Keep bundle assembly, signing, verification, and publication in Smart Build. A direct Cargo build verifies Rust compilation only.
