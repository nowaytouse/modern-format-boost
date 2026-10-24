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

## Local checkpoint persistence

The local SQLite `checkpoint` namespace uses a logical schema-3 header plus one
hash-verified row per normalized source identity. Completing or invalidating one
source changes only that row in a transaction; memory and processed-path state
advance only after commit. WAL, FULL synchronization, owned checkpoint locks and
source-content validation are unchanged.

A compatible schema-2 checkpoint, including a valid empty checkpoint, is migrated
after source validation in one transaction. Failure rolls back to the prior
state. No checkpoint is created merely because an absent scope was inspected.
Malformed payloads, wrong row identities, unsupported schemas and orphaned rows
are retained and rejected, not treated as empty state. An older binary cannot
read schema-3 checkpoint rows and must not be used for their resume.

Startup validation and compaction still traverse the current checkpoint once;
each entry still opens a database connection. This contract removes whole-state
rewrites from per-file updates, not all database I/O.

Focused regressions live in `checkpoint::tests`, including unchanged-row byte,
digest and timestamp checks, atomic migration rollback and empty-state migration.

## Verification

```bash
cargo test -p foundation --lib --locked checkpoint::tests:: -- --test-threads=1
cargo test -p dev --test test_real_silent_fallbacks database -- --test-threads=1
cargo test -p dev --test test_real_silent_fallbacks algorithm_inference_snapshot -- --test-threads=1
```

---
