//! Corpus maturity thresholds (Rust SSOT).
//!
//! Mirrors `crates/dev/scripts/mfb_corpus_thresholds.py` and
//! `foundation::algorithm_runtime` / `constants.rs`.

pub const ENV_DISABLE_STRICT_CORPUS: &str = "MODERN_FORMAT_DISABLE_STRICT_ALGORITHM_CORPUS";
pub const ENV_MIN_GIF_SAMPLES_TOTAL: &str = "MODERN_FORMAT_MIN_GIF_SAMPLES_TOTAL";
pub const ENV_MIN_GIF_SAMPLES_PER_CLASS: &str = "MODERN_FORMAT_MIN_GIF_SAMPLES_PER_CLASS";
pub const ENV_MIN_QUALITY_SAMPLES_TOTAL: &str = "MODERN_FORMAT_MIN_QUALITY_SAMPLES_TOTAL";
pub const ENV_MIN_QUALITY_SAMPLES_PER_CLASS: &str = "MODERN_FORMAT_MIN_QUALITY_SAMPLES_PER_CLASS";

pub const MIN_GIF_SAMPLES_TOTAL: u64 = 50;
pub const MIN_GIF_SAMPLES_PER_CLASS: u64 = 15;
pub const MIN_QUALITY_SAMPLES_TOTAL: u64 = 40;
pub const MIN_QUALITY_SAMPLES_PER_CLASS: u64 = 15;

pub const MIN_GIF_SAMPLES_TOTAL_STRICT: u64 = 150;
pub const MIN_GIF_SAMPLES_PER_CLASS_STRICT: u64 = 30;
pub const MIN_QUALITY_SAMPLES_TOTAL_STRICT: u64 = 60;
pub const MIN_QUALITY_SAMPLES_PER_CLASS_STRICT: u64 = 25;

fn truthy_env(name: &str) -> bool {
    matches!(
        std::env::var(name)
            .unwrap_or_default()
            .trim()
            .to_lowercase()
            .as_str(),
        "1" | "true" | "yes"
    )
}

#[must_use]
pub fn strict_corpus_enabled() -> bool {
    !truthy_env(ENV_DISABLE_STRICT_CORPUS)
}

fn env_u64_at_least(name: &str, floor: u64) -> u64 {
    match std::env::var(name) {
        Ok(raw) => {
            let raw = raw.trim();
            if raw.is_empty() {
                return floor;
            }
            match raw.parse::<u64>() {
                Ok(v) => v.max(floor),
                Err(_) => floor,
            }
        }
        Err(_) => floor,
    }
}

#[must_use]
pub fn min_loop_samples_total() -> u64 {
    let floor = if strict_corpus_enabled() {
        MIN_GIF_SAMPLES_TOTAL_STRICT
    } else {
        MIN_GIF_SAMPLES_TOTAL
    };
    env_u64_at_least(ENV_MIN_GIF_SAMPLES_TOTAL, floor)
}

#[must_use]
pub fn min_loop_samples_per_class() -> u64 {
    let floor = if strict_corpus_enabled() {
        MIN_GIF_SAMPLES_PER_CLASS_STRICT
    } else {
        MIN_GIF_SAMPLES_PER_CLASS
    };
    env_u64_at_least(ENV_MIN_GIF_SAMPLES_PER_CLASS, floor)
}

#[must_use]
pub fn min_quality_samples_total() -> u64 {
    let floor = if strict_corpus_enabled() {
        MIN_QUALITY_SAMPLES_TOTAL_STRICT
    } else {
        MIN_QUALITY_SAMPLES_TOTAL
    };
    env_u64_at_least(ENV_MIN_QUALITY_SAMPLES_TOTAL, floor)
}

#[must_use]
pub fn min_quality_samples_per_class() -> u64 {
    let floor = if strict_corpus_enabled() {
        MIN_QUALITY_SAMPLES_PER_CLASS_STRICT
    } else {
        MIN_QUALITY_SAMPLES_PER_CLASS
    };
    env_u64_at_least(ENV_MIN_QUALITY_SAMPLES_PER_CLASS, floor)
}

#[must_use]
pub fn loop_corpus_samples_shortfall(total: u64, quality_class: u64, video_class: u64) -> u64 {
    let total = total;
    let quality_class = quality_class;
    let video_class = video_class;
    let min_total = min_loop_samples_total();
    let min_per = min_loop_samples_per_class();
    let needed_total = min_total.saturating_sub(total);
    let needed_quality = min_per.saturating_sub(quality_class);
    let needed_video = min_per.saturating_sub(video_class);
    needed_total.max(needed_quality + needed_video)
}

#[must_use]
pub fn quality_corpus_samples_shortfall(high: u64, low: u64) -> u64 {
    let high = high;
    let low = low;
    let total = high + low;
    let min_total = min_quality_samples_total();
    let min_per = min_quality_samples_per_class();
    let needed_total = min_total.saturating_sub(total);
    let needed_high = min_per.saturating_sub(high);
    let needed_low = min_per.saturating_sub(low);
    needed_total.max(needed_high + needed_low)
}

#[must_use]
pub fn loop_corpus_is_mature(total: u64, quality_class: u64, video_class: u64) -> bool {
    total >= min_loop_samples_total()
        && quality_class >= min_loop_samples_per_class()
        && video_class >= min_loop_samples_per_class()
}

#[must_use]
pub fn quality_corpus_is_mature(high: u64, low: u64) -> bool {
    high + low >= min_quality_samples_total()
        && high >= min_quality_samples_per_class()
        && low >= min_quality_samples_per_class()
}
