# SOURCE: MEDIA_CONVERSION_DISCIPLINE_SEAL.md

# Media conversion discipline layer seal (Rust + Python + entry)

This document seals **engineering discipline** for the delivery/training codebase: numeric-forgery scan,
extended default patterns (M68–M69), Python entry guards, and training tier SSOT. It is **not** runtime
corpus success on user files (see [`MEDIA_CONVERSION_DELIVERY_SEAL.md`](MEDIA_CONVERSION_DELIVERY_SEAL.md)).

## What “100% discipline” means

| Check                                                                                                             | Enforced by                                                                       |
| ----------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------- |
| Zero unallowlisted `unwrap_or(0)` / `map_or(0,…)` / `map_or(100,…)` in production Rust (`img`/`vid`/`foundation`) | `production_code_has_no_numeric_forgery_fallbacks`                                |
| M68 extended patterns (`unwrap_or_default`, `map_or(true,…)`, etc.)                                               | `media_conversion_extended_defaults_m68`                                          |
| M69 substrate patterns (FFI/JXL/loop/DB JPEG)                                                                     | `media_conversion_substrate_defaults_m69`                                         |
| Numeric defaults only in `media_conversion_gate.rs` (ALLOWLIST empty)                                             | same scans + M43                                                                  |
| Python production scripts call `guard_main`                                                                       | `python_production_scripts_declare_guard_main` + `PRODUCTION_GUARDED_SCRIPTS`     |
| Training ingest fail-closed                                                                                       | `training_pipeline_execute_paths_are_fail_closed`                                 |
| Tier rules Rust↔JSON lock                                                                                         | `training_tier_ambiguous_policy_defaults_to_exclude` + M157                       |
| **Single closure gate**                                                                                           | `media_conversion_discipline_layer_closure_m158` runs all scans above in one test |

When `media_conversion_discipline_layer_closure_m158` passes in CI, the **Rust + Py + sh discipline slice is 100%**
for documented invariants. Remaining ~45–55% gap is **runtime corpus E2E**, not discipline.

## Training corpus (M159)

Static ingest tier rules are tightened in `training_tier_audit.rs` + `training_rules.json`:

- **high** and **low** both use **ANY** (one rule sufficient).
- Social-high: short side ≥1080 with entropy ≥5.5 (dead zone does not block dimension-qualified highs).
- Low: max side ≤512 with entropy ≤5.5.
- `run_training.py` emits `[INFO|WARN] training_corpus_tier_coverage` after each static tier rescan.

Dev test: `media_conversion_training_corpus_tier_m159`.

## Out of scope (by design)

- PostgreSQL trigger/schema internals
- “Zero `[delivery fallback:…]` on every file”
- Full-library batch transcode success

---
