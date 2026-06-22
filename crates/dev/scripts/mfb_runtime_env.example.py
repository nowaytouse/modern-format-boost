# Modern Format Boost — runtime algorithm gates example (Python/JSON format).
# Copy this dictionary's keys to your environment variables or local_env.json.
# Defaults are tightened: unit-probability seal always on; structural seal, Layer6 KNN,
# loop inference_log, quality DB lookup/fusion, HDBSCAN fusion, and KNN disagreement guard default ON.
# Loop feature_stats fail-closed unless LOOP_FEATURE_STATS_FAIL_OPEN=1 (dev only).

ENV_EXAMPLES = {
    # ── Loop intent (seal + Layer6 KNN + HDBSCAN fusion default ON) ─────────────
    # "MODERN_FORMAT_DISABLE_LOOP_INTENT_LAYER6_KNN": "1",
    # "MODERN_FORMAT_DISABLE_LOOP_INTENT_ALGORITHM_SEAL": "1",
    # "MODERN_FORMAT_DISABLE_LOOP_HDBSCAN_FUSION": "1",   # pure HNSW when catalog missing
    # "MODERN_FORMAT_LOOP_HNSW_MIN_WEIGHTED_NEIGHBORS": "2",
    # "MODERN_FORMAT_LOOP_FEATURE_STATS_FAIL_OPEN": "1",   # dev/bootstrap only (default fail-closed)
    # "MODERN_FORMAT_DISABLE_LOOP_FEATURE_STATS_FAIL_OPEN": "1",  # lock fail-closed even if FAIL_OPEN=1
    # "MODERN_FORMAT_DISABLE_LOOP_INTENT_INFERENCE_LOG": "1",
    # "MODERN_FORMAT_DISABLE_LOOP_INTENT_INFERENCE_AUDIT_ONLY": "1",  # write runtime verdict to loop inference_log column
    # "MODERN_FORMAT_DISABLE_QUALITY_INFERENCE_AUDIT_ONLY": "1",    # write runtime verdict to quality inference_log columns
    # ── Static / scenario quality (seal + DB lookup/fusion default ON) ───────────
    # "MODERN_FORMAT_DISABLE_QUALITY_ALGORITHM_SEAL": "1",
    # "MODERN_FORMAT_DISABLE_STATIC_QUALITY_DB_LOOKUP": "1",
    # "MODERN_FORMAT_DISABLE_STATIC_QUALITY_DB_FUSION": "1",
    # "MODERN_FORMAT_DISABLE_SCENARIO_QUALITY_DB_FUSION": "1",
    # "MODERN_FORMAT_DISABLE_SCENARIO_QUALITY_DB_LOOKUP": "1",
    # "MODERN_FORMAT_DISABLE_QUALITY_KNN_DISAGREE_GUARD": "1",
    # "MODERN_FORMAT_FORCE_QUALITY_KNN": "1",   # forces static lookup even when lookup disabled
    # ── Exploration / encode (seal + confidence gate default ON) ─────────────────
    # "MODERN_FORMAT_DISABLE_EXPLORATION_ALGORITHM_SEAL": "1",
    # "MODERN_FORMAT_DISABLE_EXPLORATION_CONFIDENCE_GATE": "1",
    # "MODERN_FORMAT_DISABLE_EXPLORATION_SSIM_PRESENCE_GATE": "1",
    # "MODERN_FORMAT_DISABLE_EXPLORATION_SSIM_THRESHOLD_GATE": "1",
    # "MODERN_FORMAT_DISABLE_EXPLORATION_SIZE_TARGET_GATE": "1",
    # ── Inference logging (quality stacks; heuristic branches default ON) ─────────
    # "MODERN_FORMAT_DISABLE_QUALITY_INFERENCE_HEURISTIC_LOGS": "1",
    # ── Corpus maturity (strict 150/30 loop, 60/25 quality — default ON) ─────────
    # "MODERN_FORMAT_DISABLE_STRICT_ALGORITHM_CORPUS": "1",   # relax to 50/15 base floors
    # "MODERN_FORMAT_STRICT_ALGORITHM_CORPUS": "1",           # legacy redundant with default
    # "MODERN_FORMAT_MIN_GIF_SAMPLES_TOTAL": "200",
    # "MODERN_FORMAT_MIN_QUALITY_SAMPLES_TOTAL": "100",
    # ── Global kill-switches (usually leave unset) ───────────────────────────────
    # "MODERN_FORMAT_DISABLE_DB_FEEDBACK": "1",
    # "MODERN_FORMAT_DISABLE_IMAGE_QUALITY_DB": "1",
}
