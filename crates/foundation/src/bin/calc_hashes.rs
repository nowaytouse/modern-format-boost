//! Developer utility binary.
//!
//! This binary is not used by the main product, but some dependency-safety
//! tooling (e.g. `cargo-geiger`) expects a consistent set of bin targets
//! under `src/bin/`.

fn main() {
    if let Err(err) = foundation::entry_guard::assert_dev_tool_entry("calc_hashes") {
        eprintln!("calc_hashes entry guard: {err:#}");
        std::process::exit(foundation::constants::EXIT_CODE_ERROR);
    }
    foundation::progress_mode::emit_stderr(
        "calc_hashes is a developer-only utility and is currently a placeholder.",
    );
}
