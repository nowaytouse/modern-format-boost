# SOURCE: DATABASE_LAYER_CONTRACT.md

# Database layer contract

PostgreSQL schema, multi-scenario ingest, loop-intent vectors, and quality DB paths in
`database.rs`, `multi_scenario_db.rs`, `image_quality_db.rs`, and `database_vector.rs`.
Delivery audits: [`MEDIA_CONVERSION_LAYER_CONTRACT.md`](MEDIA_CONVERSION_LAYER_CONTRACT.md) (M21, M34, M37, M38).
Algorithm inference truth: [`ALGORITHM_LAYER_CONTRACT.md`](ALGORITHM_LAYER_CONTRACT.md) (I4, I6, I10).

## Core invariants

| ID  | Invariant                                                                                     | Enforcement                                                     |
| --- | --------------------------------------------------------------------------------------------- | --------------------------------------------------------------- |
| D1  | Schema/table validation errors use `ui_user_facing_error` (no raw `bail!("❌ …")`)            | `multi_scenario_schema_errors_use_gate_d1`                      |
| D2  | DB refresh / audit stderr uses `ui_icon_pick`, `symbols::pick` (legacy), or `ui_stderr::line` | `database_audit_logs_use_symbol_pick`, `ui_icon_pick`, U12, M64 |
| D3  | Loop ingest duration labels use gate helpers                                                  | `ui_duration_secs_label_or_unknown`, U12                        |
| D4  | `inference_log` snapshot overlays use `json_*_or_null` / `json_finite_f64_or_null`            | I10, `algorithm_inference_snapshot_audit_overlay_i10`           |
| D5  | KNN / maturity logs prefix emoji via `symbols::pick` (no raw format literals)                 | `database_audit_logs_use_symbol_pick`                           |

## Verification

```bash
cargo test -p dev --test test_real_silent_fallbacks database -- --test-threads=1
cargo test -p dev --test test_real_silent_fallbacks algorithm_inference_snapshot -- --test-threads=1
```

---
