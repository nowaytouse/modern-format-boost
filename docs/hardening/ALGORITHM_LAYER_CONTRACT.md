# SOURCE: ALGORITHM_LAYER_CONTRACT.md

# Algorithm layer contract (100% acceptance criteria)

This document is the **single source of truth** for whether the algorithm layer is "contract complete."
It does **not** require zero operational fallbacks; it requires **honest telemetry**, **default-on gates**
with `MODERN_FORMAT_DISABLE_*` kill-switches, and **CI-enforced** invariants.

**See also:** [README layer contracts](../README.md#-layer-contracts--training) · [CHANGELOG 0.11.4](CHANGELOG.md) · [MULTI_SCENARIO_IMPLEMENTATION_GUIDE](MULTI_SCENARIO_IMPLEMENTATION_GUIDE.md)

## Core invariants

| ID  | Invariant                                                                                                                                                                                                            | Enforcement                                                                                                                |
| --- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------- |
| I1  | Runtime gates read `MODERN_FORMAT_DISABLE_*` only (via [`algorithm_runtime.rs`](../crates/foundation/src/algorithm_runtime.rs))                                                                                      | `algorithm_runtime_rejects_legacy_enable_env_keys`, `algorithm_modules_reject_legacy_enable_env_reads`                     |
| I2  | Unit probabilities are finite and in \[0,1\]; non-finite rejected                                                                                                                                                    | [`algorithm_seal.rs`](../crates/foundation/src/algorithm_seal.rs) + `algorithm_modules_reject_forbidden_numeric_fallbacks` |
| I3  | Video exploration delivery uses `ExploreResult::pipeline_acceptable`                                                                                                                                                 | `vid_explore_delivery_paths_do_not_use_quality_or_size_or_gate`, vid unit tests                                            |
| I4  | Loop `inference_log` default audit-only: column `final_verdict = TelemetryOnly`; runtime truth in `signal_snapshot`                                                                                                  | `loop_inference_runtime_verdict_from_snapshot`, SQL `loop_inference_log_effective`                                         |
| I5  | Layer 7 policy exits store **NULL** `tree_probability` / `final_probability` (no borrowed tree posterior)                                                                                                            | `inference_used_layer7_policy`, `loop_intent_layer7_inference_log_does_not_borrow_tree_posterior`                          |
| I6  | HDBSCAN fusion default-on: missing/invalid catalog → `HdbscanCatalogUnavailable` (not silent pure HNSW)                                                                                                              | `database.rs` tests, production alert                                                                                      |
| I7  | Static + scenario quality heuristic branches label `resolution_branch`; heuristic scores must not populate `knn_score`                                                                                               | `heuristic_branches_do_not_populate_knn_score_column`, `scenario_heuristic_branches_do_not_populate_knn_score_columns`     |
| I8  | Python training/scripts use `DISABLE_*` only, not `MODERN_FORMAT_ENABLE_*`                                                                                                                                           | `training_scripts_do_not_use_legacy_modern_format_enable_env`                                                              |
| I9  | Cross-crate numeric forgery scan (`foundation`, `img`, `vid`)                                                                                                                                                        | `production_code_has_no_numeric_forgery_fallbacks`                                                                         |
| I10 | Loop `inference_log` snapshot overlays (`audit_only`, `runtime_final_probability`, etc.) use `json_*_or_null` / `json_finite_f64_or_null` helpers (no inline `map_or(Value::Null, \|p\| json!(p))` on probabilities) | `json_finite_f64_or_null`; dev test `algorithm_inference_snapshot_audit_overlay_i10`                                       |

## Allowlisted fallbacks (documented, not fake probabilities)

| Path                               | Branch / signal                                      | Kill-switch / opt-in                               | Telemetry rule                                                    |
| ---------------------------------- | ---------------------------------------------------- | -------------------------------------------------- | ----------------------------------------------------------------- |
| Loop corpus immature               | `corpus_immature` HNSW branch                        | `DISABLE_STRICT_ALGORITHM_CORPUS=1` relaxes floors | No KNN posterior; tree-only or skip                               |
| Loop DB unavailable                | `layer7_fallback`, `Layer 0: DB unavailable`         | `DISABLE_DB_FEEDBACK=1`                            | Layer 7 → NULL posteriors in `inference_log`                      |
| Loop feature_stats parse           | bootstrap defaults                                   | `LOOP_FEATURE_STATS_FAIL_OPEN=1` (opt-in)          | Logged; fail-closed by default                                    |
| Loop Layer 7                       | `layer7_fallback`                                    | N/A (policy)                                       | NULL `tree_probability` / `final_probability`                     |
| HDBSCAN catalog missing            | `hdbscan_catalog_unavailable`                        | `DISABLE_LOOP_HDBSCAN_FUSION=1`                    | Reject fusion; optional pure HNSW                                 |
| Static quality immature            | `corpus_immature_heuristic`                          | `DISABLE_STRICT_ALGORITHM_CORPUS=1`                | `final_verdict=immature`; no `knn_score` from BPP                 |
| Scenario (animated/video) immature | `corpus_immature_heuristic`                          | same                                               | NULL `knn_*` columns; scores in `inference_snapshot`              |
| Static quality DB off              | `db_disabled_heuristic` / `db_unavailable_heuristic` | `DISABLE_IMAGE_QUALITY_DB=1`                       | Heuristic scores in `heuristic_score` / `bpp_fallback_score` only |
| Video explore apple_compat         | SSIM ≥ `ACCEPTABLE_MIN_SSIM`                         | `APPLE_COMPAT` flag                                | Explicit exception to `pipeline_acceptable`                       |
| macOS branding                     | `ENV_ENABLE_BRANDING`                                | Opt-in `=1`                                        | Not an algorithm inference gate                                   |

## Analytics (SQL)

Do not `GROUP BY` placeholder `final_verdict` / `resolution_branch` on audit-only rows.

| View                                             | Key columns                                                                                                            |
| ------------------------------------------------ | ---------------------------------------------------------------------------------------------------------------------- |
| `loop_inference_log_effective`                   | `effective_final_verdict`, `effective_final_probability`, `is_layer7_policy_exit`, `tree_probability_is_authoritative` |
| `image_quality_inference_log_effective`          | `effective_final_verdict`, `effective_resolution_branch`                                                               |
| `animated_image_quality_inference_log_effective` | same                                                                                                                   |
| `video_quality_inference_log_effective`          | same                                                                                                                   |

Defined in [`migrations/003_inference_runtime_verdict_views.sql`](../migrations/003_inference_runtime_verdict_views.sql) and [`migrations/004_loop_inference_posterior_views.sql`](../migrations/004_loop_inference_posterior_views.sql), applied by [`multi_scenario_db.rs`](../crates/foundation/src/multi_scenario_db.rs).

## Compliance matrix (tests)

| Invariant          | Test(s)                                                                                                                             |
| ------------------ | ----------------------------------------------------------------------------------------------------------------------------------- |
| I1                 | `algorithm_runtime_rejects_legacy_enable_env_keys`, `algorithm_modules_reject_legacy_enable_env_reads`                              |
| I2                 | `algorithm_modules_reject_forbidden_numeric_fallbacks`, `production_code_has_no_numeric_forgery_fallbacks`                          |
| I3                 | `vid_explore_delivery_paths_do_not_use_quality_or_size_or_gate`, `cache_exact_hint_uses_pipeline_acceptable_not_quality_or_size_or` |
| I4                 | `loop_inference_runtime_verdict_from_snapshot` (database tests), views in schema init                                               |
| I5                 | `inference_used_layer7_policy_detects_fallback_tracking`, `loop_intent_layer7_inference_log_does_not_borrow_tree_posterior`         |
| I6                 | `database` HDBSCAN catalog tests                                                                                                    |
| I7                 | `heuristic_branches_do_not_populate_knn_score_column`, `scenario_heuristic_branches_do_not_populate_knn_score_columns`              |
| I8                 | `training_scripts_do_not_use_legacy_modern_format_enable_env`                                                                       |
| I9                 | `production_code_has_no_numeric_forgery_fallbacks`                                                                                  |
| I10                | `algorithm_inference_snapshot_audit_overlay_i10`                                                                                    |
| Module coverage    | `algorithm_audit_modules_cover_runtime_callers`                                                                                     |
| Contract doc       | `algorithm_contract_doc_exists_and_lists_allowlist`                                                                                 |
| Env test isolation | `env_mutation_test_modules_declare_serial_isolation`                                                                                |

## Verification commands

```bash
cargo test -p foundation --lib -- --test-threads=1
cargo test -p dev --test test_real_silent_fallbacks -- --test-threads=1
cargo test -p vid cache_exact_hint --lib
```

**Contract 100%** = all commands pass and every row in the compliance matrix has a passing test.

## Out of scope (by design)

- Forcing zero heuristics when corpus is immature (production would stall).
- Default-off `quality_inference_log_heuristic_fallbacks` (behavior change).
- AST-level whole-repo audit (substring + caller coverage is sufficient for this contract).

---

