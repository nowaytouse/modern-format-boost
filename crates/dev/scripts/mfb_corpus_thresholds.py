"""Training corpus maturity thresholds (Python SSOT).

Aligned with ``foundation::algorithm_runtime`` and ``constants.rs``:
strict floors default-on unless ``MODERN_FORMAT_DISABLE_STRICT_ALGORITHM_CORPUS=1``.
Env overrides only *raise* floors (never lower below code defaults).
"""

from __future__ import annotations

import os

ENV_DISABLE_STRICT_CORPUS = "MODERN_FORMAT_DISABLE_STRICT_ALGORITHM_CORPUS"
ENV_MIN_GIF_SAMPLES_TOTAL = "MODERN_FORMAT_MIN_GIF_SAMPLES_TOTAL"
ENV_MIN_GIF_SAMPLES_PER_CLASS = "MODERN_FORMAT_MIN_GIF_SAMPLES_PER_CLASS"
ENV_MIN_QUALITY_SAMPLES_TOTAL = "MODERN_FORMAT_MIN_QUALITY_SAMPLES_TOTAL"
ENV_MIN_QUALITY_SAMPLES_PER_CLASS = "MODERN_FORMAT_MIN_QUALITY_SAMPLES_PER_CLASS"

# Base (relaxed) floors — match ``constants.rs``.
MIN_GIF_SAMPLES_TOTAL = 50
MIN_GIF_SAMPLES_PER_CLASS = 15
MIN_QUALITY_SAMPLES_TOTAL = 40
MIN_QUALITY_SAMPLES_PER_CLASS = 15

# Strict floors (default when strict corpus is enabled).
MIN_GIF_SAMPLES_TOTAL_STRICT = 150
MIN_GIF_SAMPLES_PER_CLASS_STRICT = 30
MIN_QUALITY_SAMPLES_TOTAL_STRICT = 60
MIN_QUALITY_SAMPLES_PER_CLASS_STRICT = 25


def _truthy(name: str) -> bool:
    return os.getenv(name, "").strip().lower() in ("1", "true", "yes")


def strict_corpus_enabled() -> bool:
    return not _truthy(ENV_DISABLE_STRICT_CORPUS)


def _env_i64_at_least(name: str, floor: int) -> int:
    raw = os.getenv(name, "").strip()
    if not raw:
        return floor
    try:
        value = int(raw, 10)
    except ValueError:
        return floor
    return max(floor, value)


def min_loop_samples_total() -> int:
    floor = (
        MIN_GIF_SAMPLES_TOTAL_STRICT
        if strict_corpus_enabled()
        else MIN_GIF_SAMPLES_TOTAL
    )
    return _env_i64_at_least(ENV_MIN_GIF_SAMPLES_TOTAL, floor)


def min_loop_samples_per_class() -> int:
    floor = (
        MIN_GIF_SAMPLES_PER_CLASS_STRICT
        if strict_corpus_enabled()
        else MIN_GIF_SAMPLES_PER_CLASS
    )
    return _env_i64_at_least(ENV_MIN_GIF_SAMPLES_PER_CLASS, floor)


def min_quality_samples_total() -> int:
    floor = (
        MIN_QUALITY_SAMPLES_TOTAL_STRICT
        if strict_corpus_enabled()
        else MIN_QUALITY_SAMPLES_TOTAL
    )
    return _env_i64_at_least(ENV_MIN_QUALITY_SAMPLES_TOTAL, floor)


def min_quality_samples_per_class() -> int:
    floor = (
        MIN_QUALITY_SAMPLES_PER_CLASS_STRICT
        if strict_corpus_enabled()
        else MIN_QUALITY_SAMPLES_PER_CLASS
    )
    return _env_i64_at_least(ENV_MIN_QUALITY_SAMPLES_PER_CLASS, floor)


def loop_corpus_samples_shortfall(
    total: int, quality_class: int, video_class: int
) -> int:
    """Non-negative shortfall (same formula as Rust ``loop_corpus_samples_shortfall``)."""
    total = max(0, total)
    quality_class = max(0, quality_class)
    video_class = max(0, video_class)
    min_total = min_loop_samples_total()
    min_per = min_loop_samples_per_class()
    needed_total = max(0, min_total - total)
    needed_quality = max(0, min_per - quality_class)
    needed_video = max(0, min_per - video_class)
    return max(needed_total, needed_quality + needed_video)


def quality_corpus_samples_shortfall(high: int, low: int) -> int:
    """Non-negative shortfall (same formula as Rust ``quality_corpus_samples_shortfall``)."""
    high = max(0, high)
    low = max(0, low)
    total = high + low
    min_total = min_quality_samples_total()
    min_per = min_quality_samples_per_class()
    needed_total = max(0, min_total - total)
    needed_high = max(0, min_per - high)
    needed_low = max(0, min_per - low)
    return max(needed_total, needed_high + needed_low)


def loop_corpus_is_mature(total: int, quality_class: int, video_class: int) -> bool:
    return (
        total >= min_loop_samples_total()
        and quality_class >= min_loop_samples_per_class()
        and video_class >= min_loop_samples_per_class()
    )


def quality_corpus_is_mature(high: int, low: int) -> bool:
    return (
        high + low >= min_quality_samples_total()
        and high >= min_quality_samples_per_class()
        and low >= min_quality_samples_per_class()
    )
