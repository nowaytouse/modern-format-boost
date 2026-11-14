//! Training ingest progress on stderr (auditable, default-on; opt out via env).

use std::io::Write as _;
use std::time::Duration;

/// Whether [`crate::c_api::ingest_media_samples_batch`] emits `[INGEST-RUST]`
/// progress.
#[must_use]
pub fn ingest_progress_enabled() -> bool {
    crate::media_conversion_gate::delivery_env_enabled_unless_opt_out(
        crate::constants::ENV_MFB_TRAINING_INGEST_PROGRESS,
    )
}

/// Emit a progress line every N paths (always includes first and last when
/// enabled).
#[must_use]
pub const fn ingest_progress_step(total: usize) -> usize {
    if total <= 20 {
        1
    } else if total <= 200 {
        5
    } else if total <= 2000 {
        25
    } else {
        100
    }
}

/// Write a single auditable progress line to stderr (flushed immediately).
pub fn emit_ingest_progress_line(line: &str) {
    let _ = writeln!(std::io::stderr(), "{line}");
    let _ = std::io::stderr().flush();
}

/// Format seconds for `[INGEST-RUST]` lines (one decimal when >= 10s).
#[must_use]
pub fn format_elapsed_secs(elapsed: Duration) -> String {
    let secs = elapsed.as_secs_f64();
    if secs >= 10.0 {
        format!("{secs:.1}s")
    } else {
        format!("{secs:.2}s")
    }
}

/// Path token for logs: basename only, capped length.
#[must_use]
pub fn path_basename_for_log(path: &std::path::Path) -> String {
    let name = crate::media_conversion_gate::delivery_path_basename_for_log_or_unknown(path);
    if name.len() <= 80 {
        name
    } else {
        format!("{}…", &name[..77])
    }
}

/// Whether this index should emit a progress tick (1-based index).
#[must_use]
pub const fn should_emit_ingest_progress_tick(
    index_one_based: usize,
    total: usize,
    step: usize,
) -> bool {
    index_one_based == 1 || index_one_based == total || index_one_based.is_multiple_of(step)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ingest_progress_step_scales_with_batch() {
        assert_eq!(ingest_progress_step(1), 1);
        assert_eq!(ingest_progress_step(15), 1);
        assert_eq!(ingest_progress_step(50), 5);
        assert_eq!(ingest_progress_step(500), 25);
        assert_eq!(ingest_progress_step(5000), 100);
    }

    #[test]
    fn progress_tick_covers_ends() {
        let step = ingest_progress_step(100);
        assert!(should_emit_ingest_progress_tick(1, 100, step));
        assert!(should_emit_ingest_progress_tick(100, 100, step));
    }
}
