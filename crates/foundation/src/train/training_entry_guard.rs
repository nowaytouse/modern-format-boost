//! Training ingest / tier-probe entry guards (delegates to
//! [`crate::entry_guard`]).

use crate::entry_guard::{
    self, CliEntryGuard, INVOKER_DIRECT, INVOKER_INTERNAL_REEXEC, INVOKER_RUN_TRAINING,
    INVOKER_TEST_HARNESS, INVOKER_TRAINING_PIPELINE,
};
use anyhow::Result;

const INGEST_INVOKERS: &[&str] = &[
    INVOKER_RUN_TRAINING,
    INVOKER_TRAINING_PIPELINE,
    INVOKER_TEST_HARNESS,
    INVOKER_DIRECT,
    INVOKER_INTERNAL_REEXEC,
    "run_training.py",
];

fn assert_tier_probe_invoker(api_name: &'static str) -> Result<()> {
    let invoker = entry_guard::resolved_invoker();
    if INGEST_INVOKERS.contains(&invoker.as_str()) {
        return Ok(());
    }
    if invoker.is_empty() {
        anyhow::bail!(
            "refusing {api_name} without MFB_INVOKER or MFB_TRAINING_INVOKER (allowed: {}). Use: \
             cargo run --locked -p dev --bin run_training -- --execute",
            INGEST_INVOKERS.join(", ")
        );
    }
    anyhow::bail!(
        "refusing {api_name}: unknown invoker {invoker:?} (allowed: {})",
        INGEST_INVOKERS.join(", ")
    );
}

/// Guard for `train_quality` binary (`main`).
///
/// # Errors
///
/// Returns an error when the entry guard checks in [`CliEntryGuard::assert`]
/// fail.
pub fn assert_train_quality_entry() -> Result<()> {
    CliEntryGuard {
        expected_bin: "train_quality",
        allowed_invokers: INGEST_INVOKERS,
        production_hint: "Use: cargo run --locked -p dev --bin run_training -- --execute",
        require_invoker: true,
    }
    .assert()
}

/// Guard for `train_knn` binary (`main`).
///
/// # Errors
///
/// Returns an error when the entry guard checks in [`CliEntryGuard::assert`]
/// fail.
pub fn assert_train_knn_entry() -> Result<()> {
    CliEntryGuard {
        expected_bin: "train_knn",
        allowed_invokers: INGEST_INVOKERS,
        production_hint: "Use: cargo run --locked -p dev --bin run_training -- --execute",
        require_invoker: true,
    }
    .assert()
}

/// Guard for training probe C-APIs used during collection.
///
/// # Errors
///
/// Returns an error when a shell wrapper is detected or the delegated invoker
/// is missing / invalid. Compatibility callers may host this C-API in Python,
/// so `argv[0]` is not required to match the exported symbol name.
pub fn assert_tier_probe_c_api_entry(api_name: &'static str) -> Result<()> {
    if entry_guard::shell_wrapper_in_ancestry(6).is_some() {
        anyhow::bail!(
            "refusing {api_name} via shell-wrapped process; invoke via cargo run --locked -p dev \
             --bin run_training -- --execute"
        );
    }
    assert_tier_probe_invoker(api_name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    struct EnvRestore {
        key: &'static str,
        value: Option<String>,
    }

    impl EnvRestore {
        fn set(key: &'static str, value: &'static str) -> Self {
            let previous = saved_env_value(key);
            unsafe {
                std::env::set_var(key, value);
            }
            Self {
                key,
                value: previous,
            }
        }

        fn remove(key: &'static str) -> Self {
            let previous = saved_env_value(key);
            unsafe {
                std::env::remove_var(key);
            }
            Self {
                key,
                value: previous,
            }
        }
    }

    fn saved_env_value(key: &str) -> Option<String> {
        match std::env::var(key) {
            Ok(value) => Some(value),
            Err(std::env::VarError::NotPresent) => None,
            Err(e) => {
                crate::media_conversion_gate::delivery_db_batch_audit(
                    "training_entry_env",
                    format!("failed to read env {key} before override: {e}; restore will remove"),
                );
                None
            }
        }
    }

    impl Drop for EnvRestore {
        fn drop(&mut self) {
            if let Some(value) = self.value.as_deref() {
                unsafe {
                    std::env::set_var(self.key, value);
                }
            } else {
                unsafe {
                    std::env::remove_var(self.key);
                }
            }
        }
    }

    #[test]
    #[serial]
    fn tier_probe_invoker_accepts_run_training() {
        let _invoker = EnvRestore::set(crate::entry_guard::MFB_INVOKER_ENV, "run_training.py");
        let _training = EnvRestore::remove(crate::entry_guard::TRAINING_INVOKER_ENV);
        assert!(assert_tier_probe_invoker("mfb_probe_static_still_image").is_ok());
    }

    #[test]
    #[serial]
    fn tier_probe_invoker_rejects_unknown_invoker() {
        let _invoker = EnvRestore::set(crate::entry_guard::MFB_INVOKER_ENV, "python_api.py");
        let _training = EnvRestore::remove(crate::entry_guard::TRAINING_INVOKER_ENV);
        let err = assert_tier_probe_invoker("mfb_probe_loop_intent")
            .expect_err("unknown invoker must fail");
        assert!(err.to_string().contains("unknown invoker"));
        assert!(err.to_string().contains("mfb_probe_loop_intent"));
    }
}
