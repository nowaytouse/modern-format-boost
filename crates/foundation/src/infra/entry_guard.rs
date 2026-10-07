//! Project-wide CLI / C-API entry guards (fail-closed).
//!
//! - Rejects shell `*.sh` wrapper chains that invoke MFB Python or ingest
//!   binaries.
//! - Validates `MFB_INVOKER` / `MFB_TRAINING_INVOKER` for delegated Rust/Python
//!   tools.
//! - See `docs/dev/config/CONFIG_CONSUMERS.md` for JSON config ownership.

use anyhow::{Result, bail};
use std::process::Command;
use std::sync::OnceLock;

/// Primary invoker token (Python script or Rust binary name).
pub const MFB_INVOKER_ENV: &str = "MFB_INVOKER";
/// Training-specific alias (Rust ingest children); read after `MFB_INVOKER` if
/// unset.
pub const TRAINING_INVOKER_ENV: &str = "MFB_TRAINING_INVOKER";
pub const RUST_DIRECT_OK_ENV: &str = "MFB_RUST_DIRECT_OK";

pub const INVOKER_DIRECT: &str = "direct";
pub const INVOKER_TEST_HARNESS: &str = "test-harness";
pub const INVOKER_INTERNAL_REEXEC: &str = "internal-reexec";
pub const INVOKER_RUN_TRAINING: &str = "run_training";
pub const INVOKER_TRAINING_PIPELINE: &str = "training_pipeline";
pub const INVOKER_DATABASE_MANAGER: &str = "database_manager";
pub const INVOKER_CHECK_ALL: &str = "check_all";

const SHELL_NAMES: &[&str] = &["bash", "sh", "zsh", "dash", "fish", "ksh"];

const PIPELINE_TOOL_INVOKERS: &[&str] = &[
    INVOKER_RUN_TRAINING,
    INVOKER_TRAINING_PIPELINE,
    INVOKER_DATABASE_MANAGER,
    INVOKER_DIRECT,
    INVOKER_TEST_HARNESS,
    INVOKER_CHECK_ALL,
    "run_training.py",
    "training_pipeline.py",
    "loop_intent_clustering.py",
    "quality_regression_model.py",
];

const PRODUCT_INVOKERS: &[&str] = &[
    INVOKER_DIRECT,
    INVOKER_TEST_HARNESS,
    INVOKER_DATABASE_MANAGER,
    INVOKER_TRAINING_PIPELINE,
    INVOKER_RUN_TRAINING,
    "run_training.py",
    "drag_and_drop_processor",
];
const DEV_SCRIPTS_MARKER: &str = "crates/dev/scripts";

/// Guard options for a Rust CLI binary.
pub struct CliEntryGuard<'a> {
    pub expected_bin: &'a str,
    pub allowed_invokers: &'a [&'a str],
    pub production_hint: &'a str,
    /// When true, empty invoker is rejected unless [`RUST_DIRECT_OK_ENV`] +
    /// `direct`.
    pub require_invoker: bool,
}

impl CliEntryGuard<'_> {
    /// # Errors
    ///
    /// Returns an error when `argv[0]` is not the expected binary, a shell
    /// wrapper is detected in the process ancestry, or the invoker
    /// environment is missing or invalid.
    pub fn assert(self) -> Result<()> {
        assert_canonical_argv0(self.expected_bin)?;
        // Cargo integration tests launch child binaries through a temporary
        // shell script.  The explicit test-harness token is already part of
        // every CLI allow-list, so honor it here instead of mistaking Cargo's
        // runner for an untrusted production wrapper.
        if resolved_invoker() != INVOKER_TEST_HARNESS
            && let Some(wrapper) = shell_wrapper_in_ancestry(6)
        {
            bail!(
                "refusing shell-wrapped invocation of {} (ancestor: {wrapper:?}). {}",
                self.expected_bin,
                self.production_hint
            );
        }
        assert_invoker(
            self.expected_bin,
            self.allowed_invokers,
            self.require_invoker,
        )
    }
}

/// User-facing `img` / `vid` CLIs: block shell wrappers; invoker optional.
///
/// # Errors
///
/// Returns an error when the entry guard checks in [`CliEntryGuard::assert`]
/// fail.
pub fn assert_product_cli_entry(expected_bin: &str) -> Result<()> {
    CliEntryGuard {
        expected_bin,
        allowed_invokers: PRODUCT_INVOKERS,
        production_hint: "invoke directly: cargo run -p img -- … or target/release/img",
        require_invoker: false,
    }
    .assert()
}

/// DB / training maintenance binaries (`refresh_stats`, `recompute_stats`, …).
///
/// # Errors
///
/// Returns an error when the entry guard checks in [`CliEntryGuard::assert`]
/// fail.
pub fn assert_pipeline_tool_entry(expected_bin: &str) -> Result<()> {
    CliEntryGuard {
        expected_bin,
        allowed_invokers: PIPELINE_TOOL_INVOKERS,
        production_hint: "invoke via `cargo run --locked -p dev --bin training_pipeline -- ...`",
        require_invoker: true,
    }
    .assert()
}

/// Dev-only tooling (`index_gallery`, …): block shell wrappers only.
///
/// # Errors
///
/// Returns an error when the entry guard checks in [`CliEntryGuard::assert`]
/// fail.
pub fn assert_dev_tool_entry(expected_bin: &str) -> Result<()> {
    CliEntryGuard {
        expected_bin,
        allowed_invokers: &[INVOKER_DIRECT, INVOKER_TEST_HARNESS, INVOKER_CHECK_ALL],
        production_hint: "invoke directly: cargo run -p dev --bin index_gallery -- …",
        require_invoker: false,
    }
    .assert()
}

#[must_use]
pub fn resolved_invoker() -> String {
    for key in [MFB_INVOKER_ENV, TRAINING_INVOKER_ENV] {
        match std::env::var(key) {
            Ok(value) => {
                let trimmed = value.trim().to_string();
                if !trimmed.is_empty() {
                    return trimmed;
                }
            }
            Err(std::env::VarError::NotPresent) => {}
            Err(e) => {
                crate::media_conversion_gate::delivery_runtime_batch_audit(
                    "entry_guard_env",
                    format!("failed to read invoker env {key}: {e}"),
                );
            }
        }
    }
    String::new()
}

fn assert_canonical_argv0(expected_bin: &str) -> Result<()> {
    let Some(arg0) = std::env::args().next() else {
        bail!("refusing {expected_bin} with empty argv[0]");
    };
    let base = crate::media_conversion_gate::delivery_argv0_basename_or_full(&arg0);
    let expected_exe = format!("{expected_bin}.exe");
    if base != expected_bin && base != expected_exe {
        bail!("refusing non-canonical argv[0] for {expected_bin}: got {base:?}");
    }
    Ok(())
}

fn assert_invoker(context: &str, allowed_invokers: &[&str], require_invoker: bool) -> Result<()> {
    let invoker = resolved_invoker();
    if invoker.is_empty() {
        if !require_invoker || direct_cargo_invocation_allowed() {
            return Ok(());
        }
        let allowed_list = allowed_invokers.join(", ");
        bail!(
            "refusing {context} without {MFB_INVOKER_ENV} or {TRAINING_INVOKER_ENV} (allowed: \
             {allowed_list}). {}",
            production_hint_for(context)
        );
    }
    if allowed_invokers.contains(&invoker.as_str()) {
        return Ok(());
    }
    bail!(
        "refusing {context}: unknown invoker {invoker:?} (allowed: {})",
        allowed_invokers.join(", ")
    );
}

fn production_hint_for(context: &str) -> &'static str {
    match context {
        "train_quality" | "train_knn" => {
            "Production: cargo run --locked -p dev --bin run_training -- --execute"
        }
        "refresh_stats" | "recompute_stats" | "repair_loop_probe" => {
            "Production: cargo run --locked -p dev --bin training_pipeline -- <command>"
        }
        _ => "See docs/dev/config/ENTRY_GUARD_REGISTRY.md",
    }
}

fn direct_cargo_invocation_allowed() -> bool {
    let allowed = match std::env::var(RUST_DIRECT_OK_ENV) {
        Ok(value) => matches!(value.trim(), "1" | "true" | "yes"),
        Err(std::env::VarError::NotPresent) => false,
        Err(e) => {
            crate::media_conversion_gate::delivery_runtime_batch_audit(
                "entry_guard_env",
                format!("failed to read {RUST_DIRECT_OK_ENV}: {e}"),
            );
            false
        }
    };
    allowed && resolved_invoker() == INVOKER_DIRECT
}

/// Cached result of the ancestry check.  Process ancestry never changes after
/// fork, so we scan exactly once and reuse the result on every subsequent call.
/// This avoids spawning `ps` on every probed file, which was the main training
/// throughput bottleneck.
static SHELL_WRAPPER_CACHE: OnceLock<Option<String>> = OnceLock::new();

#[cfg(unix)]
fn process_args_and_ppid(pid: i32) -> Option<(String, i32)> {
    // Single `ps` invocation returns both fields in one call.
    // Output format: "<parent_pid> <args…>" — split on first space.
    let output = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "ppid= args="])
        .output();
    let output = match output {
        Ok(output) => output,
        Err(e) => {
            crate::media_conversion_gate::delivery_runtime_batch_audit(
                "entry_guard_process",
                format!("failed to run ps for pid {pid}: {e}"),
            );
            return None;
        }
    };
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    let (parent_str, args) = trimmed.split_once(' ')?;
    let parent: i32 = match parent_str.trim().parse() {
        Ok(parent) => parent,
        Err(e) => {
            crate::media_conversion_gate::delivery_runtime_batch_audit(
                "entry_guard_process",
                format!("failed to parse parent pid '{parent_str}' for pid {pid}: {e}"),
            );
            return None;
        }
    };
    let args = args.trim().to_string();
    if args.is_empty() {
        return None;
    }
    Some((args, parent))
}

#[cfg(not(unix))]
fn process_args_and_ppid(_pid: i32) -> Option<(String, i32)> {
    None
}

/// Scan process ancestry for a shell-wrapper command, caching the result.
///
/// Because ancestry is immutable for the lifetime of a process this is
/// computed at most once — subsequent calls return the cached value in O(1)
/// without spawning any child processes.
#[must_use]
pub fn shell_wrapper_in_ancestry(max_depth: usize) -> Option<String> {
    SHELL_WRAPPER_CACHE
        .get_or_init(|| scan_shell_wrapper_in_ancestry(max_depth))
        .clone()
}

fn scan_shell_wrapper_in_ancestry(max_depth: usize) -> Option<String> {
    #[cfg(unix)]
    {
        let mut pid = unsafe { libc::getppid() };
        for _ in 0..max_depth {
            if pid <= 1 {
                break;
            }
            let Some((args, parent)) = process_args_and_ppid(pid) else {
                break;
            };
            if is_shell_wrapper_command(&args) {
                return Some(args);
            }
            pid = parent;
        }
    }
    None
}

#[must_use]
pub fn is_shell_wrapper_command(args: &str) -> bool {
    let lower = args.to_ascii_lowercase();
    if is_lean_ctx_shell_snapshot_command(&lower) {
        return false;
    }
    if lower.contains(".sh") {
        for marker in [
            "run_training",
            "train_quality",
            "train_knn",
            "training_pipeline",
            "database_manager",
            "backfill_directory",
            "quality_regression",
            "loop_intent",
            "drag_and_drop",
            "recompute_stats",
            "repair_loop_probe",
            "db_diagnostics",
            DEV_SCRIPTS_MARKER,
        ] {
            if lower.contains(marker) {
                return true;
            }
        }
    }
    for shell in SHELL_NAMES {
        let needle = format!("{shell} ");
        if let Some(idx) = lower.find(&needle) {
            let rest = &lower[idx + needle.len()..];
            if let Some(first_arg) = rest.split_whitespace().next()
                && first_arg.contains(".sh")
            {
                // Exact allow rule for the single macOS App launch script
                if first_arg == "/tmp/mfb_app_launch.sh"
                    || first_arg == "/private/tmp/mfb_app_launch.sh"
                    || is_lean_ctx_shell_snapshot_command(first_arg)
                {
                    continue;
                }
                return true;
            }
        }
    }
    false
}

fn is_lean_ctx_shell_snapshot_command(args: &str) -> bool {
    args.contains("lean-ctx") && args.contains("/shell_snapshots/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_shell_wrappers() {
        assert!(is_shell_wrapper_command("bash /tmp/run_training_2k.sh"));
        assert!(is_shell_wrapper_command(
            "sh ./wrap.sh python3 crates/dev/scripts/run_training.py"
        ));
        assert!(!is_shell_wrapper_command(
            "python3 crates/dev/scripts/run_training.py --dry-run"
        ));
    }

    #[test]
    fn ignores_lean_ctx_shell_snapshot_wrappers() {
        assert!(!is_shell_wrapper_command(
            "sh /var/folders/xx/lean-ctx/shell_snapshots/run_training_snapshot.sh"
        ));
        assert!(!is_shell_wrapper_command(
            "/opt/homebrew/bin/lean-ctx -c python3 crates/dev/scripts/run_training.py --four-lane"
        ));
    }
}
