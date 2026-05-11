//! Developer utility binary.
//!
//! This binary is not used by the main product, but some dependency-safety
//! tooling (e.g. `cargo-geiger`) expects a consistent set of bin targets
//! under `src/bin/`.

fn main() {
    shared_utils::progress_mode::emit_stderr(
        "calc_hashes is a developer-only utility and is currently a placeholder.",
    );
}
