# SOURCE: MEDIA_CONVERSION_DELIVERY_SEAL.md

# Media conversion delivery layer seal

This document records the **sealed** state of runtime conversion delivery (`img` / `vid` +
`foundation` delivery substrate). It complements
[`MEDIA_CONVERSION_LAYER_CONTRACT.md`](MEDIA_CONVERSION_LAYER_CONTRACT.md) (invariants **M1–M206**;
registry test `media_conversion_contract_m1_m206_design_complete`) and is distinct from
[`ALGORITHM_LAYER_CONTRACT.md`](ALGORITHM_LAYER_CONTRACT.md) (KNN / loop intent inference) and
[`training_tier_audit.rs`](../crates/foundation/src/training_tier_audit.rs) (static training
tiers).

**See also:** [Discipline layer seal (Rust+Py+sh 100%)](MEDIA_CONVERSION_DISCIPLINE_SEAL.md) · [README layer contracts](../README.md#-layer-contracts--training) · [CHANGELOG 0.11.4](CHANGELOG.md)

## Completion note (honest scope)

| Layer                                        | Status                                                                                                                                                                                                                             |
| -------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **Delivery contract (M1–M206)**              | **Design-complete** — `media_conversion_contract_m1_m206_design_complete` (M1–M158 core + M159 corpus row + M160–M206 SSOT and unwrap/map_or/or_else closures).                                                                    |
| **Discipline (Rust+Py+sh)**                  | **100% in CI** — `media_conversion_discipline_layer_closure_m158` + poison/cwd/temp scans in `media_conversion_discipline_poison_logging_m167` (see [`MEDIA_CONVERSION_DISCIPLINE_SEAL.md`](MEDIA_CONVERSION_DISCIPLINE_SEAL.md)). |
| **Emitter / numeric forgery seal (M39–M43)** | **Sealed** in CI: single `log_anomaly!` site in gate; production numeric-forgery scan + heatmap baseline (`media_conversion_delivery_layer_sealed`).                                                                               |
| **Runtime correctness on your corpus**       | **Not 100%** — requires per-file `/tmp` validation; fallbacks remain **visible** when data is genuinely missing (DB percentiles, unmeasured SSIM, etc.).                                                                           |
| **Algorithm / training tier**                | **Out of scope** — separate contracts; delivery hardening does not seal KNN/HDBSCAN training.                                                                                                                                      |

**“100%”** here means **100% of the written contract matrix (M1–M206)** plus **100% discipline scans (M158/M167)**. It does **not** mean zero runtime fallbacks on every file. **Runtime corpus proof** on your library remains **~45–55%** (see `MEDIA_CONVERSION_HARDENING_AUDIT.md` §3).

## What is sealed (M1–M67 core + M114–M124 extensions + M160–M206 closures)

| Area                             | Guarantee                                                                                                                                                                                     |
| -------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Static / animated routing        | Fail-closed static conversion; no silent “success” copies for non-static sources (M1–M3, M8)                                                                                                  |
| Explore / size / entropy         | Strict gates; relax only via `MODERN_FORMAT_DISABLE_STRICT_MEDIA_CONVERSION=1` (M4–M7, M8)                                                                                                    |
| Fallback telemetry               | User-visible and log anomalies use `[delivery fallback:…]` via `delivery_fallback_audit` (M9–M27)                                                                                             |
| Emitter visibility               | `delivery_path_audit` / `delivery_batch_audit` / `delivery_fallback_audit` are **`pub(crate)`**; production calls `delivery_strict_*` or domain wrappers only (M100–M101)                     |
| Contract registry                | **M1–M206** milestone rows; `media_conversion_contract_m1_m206_design_complete` verifies referenced dev tests                                                                                 |
| Animated container preflight     | WebP/GIF/APNG header probe before primary ffprobe; path-scoped promote recovery (M123–M125)                                                                                                   |
| FFprobe native frame override    | GIF/APNG/WebP animated `nb_frames` from native parsers when ffprobe under-reports (M126)                                                                                                      |
| Post-ffprobe detection repair    | 0×0 canvas + under-reported `frame_count` repaired from bitstream headers in `detect_video_impl` (M127)                                                                                       |
| Analysis cache policy            | Documented hit validation chain (M152) + file_size gates (M149–M151) + I/O matrix (M148) (M128–M152)                                                                                          |
| Batch path-tree roots            | Cache validate/scan uses `canonicalize_for_tool_input` (no silent `canonicalize().unwrap_or_else`) (M103)                                                                                     |
| Quality CRF content_type         | Missing `content_type` audits via `quality_content_type_missing_audit` (M104)                                                                                                                 |
| Path safety canonicalize         | `safety.rs` / `path_validator.rs` use `canonicalize_for_tool_input` (M105)                                                                                                                    |
| Production canonicalize seal     | Workspace scan: no silent `canonicalize().unwrap_or_else` in img/vid/foundation production (M106)                                                                                             |
| Safety cwd normalization         | Relative safety checks use `delivery_safety_relative_base_or_root` (audited `/` when cwd unavailable; not run-log `.`) (M107)                                                                 |
| GPU accel numeric SSOT           | Quality score / encode improvement / compression potential / ceiling CRF via gate helpers (M108)                                                                                              |
| Loop intent signal SSOT          | Bytes/frame, audible audio, fps kinetic weights via intent batch audits (M109)                                                                                                                |
| Loop threshold duration SSOT     | `LoopThresholds::from_profile` routes percentile fallback chains via `loop_duration_or_fallback` (M110)                                                                                       |
| Loop inference defaults SSOT     | Pixels, duration-z, keywords, frame-count labels, and parent depth route through gate intent audits (M111)                                                                                    |
| Loop diagnostic label SSOT       | Probability/duration/neighbor/layer-tag formatters route through gate intent audits (M112)                                                                                                    |
| Progress/error empty-string SSOT | Explore SSIM segments and FFmpeg exit-code suffixes route through gate audits (M113)                                                                                                          |
| `log_anomaly!` routing           | **No** raw `log_anomaly!` in `crates/img/src` or `crates/vid/src`; exactly **one** `log_anomaly!` call site in `media_conversion_gate.rs` (`delivery_fallback_audit`)                         |
| Unwrap-or / JSON / env           | Production delivery paths use `media_conversion_gate` helpers (M28–M38); inference snapshots use `json_*_or_null` helpers                                                                     |
| Numeric forgery scan             | Production `img` / `vid` / `foundation` must not use silent `unwrap_or(0)` / `map_or(0, …)` except entries in the dev-test allowlist and audited code inside `media_conversion_gate.rs` (M39) |

## Explicitly out of scope

These are **not** delivery-layer violations:

- **Algorithm layer** (`algorithm_runtime`, KNN training bins, HDBSCAN fusion env gates documented in the algorithm contract)
- **`algorithm_audit.rs`** (static string / pattern scans for algorithm seals)
- **`numeric_cast.rs`** strict cast helpers (intentional error paths, not user-visible delivery defaults)
- **Unit tests** (`#[cfg(test)]`, `mod tests`, `proptest!`)
- **Structural defaults** (`LoopReferenceProfile::default()`, `partial_cmp` → `Ordering::Equal` for NaN, `ssim_mapping` ordered insert `position().unwrap_or(len)`)
- **`PoisonError::into_inner`** and mutex recovery paths audited elsewhere

## Verification (single command block)

```bash
export PATH="$HOME/.rustup/toolchains/nightly-aarch64-apple-darwin/bin:$PATH"

cargo clippy -p foundation -p img -p vid --lib -- -D warnings

cargo test -p dev --test test_real_silent_fallbacks media_conversion -- --test-threads=1
cargo test -p dev --test test_real_silent_fallbacks media_conversion_delivery -- --test-threads=1
cargo test -p dev --test test_real_silent_fallbacks production_code_has_no_numeric -- --test-threads=1

# Optional human-readable heatmap (per-file unwrap_or density)
python3 crates/dev/scripts/media_conversion_delivery_heatmap.py --report

# Deep audit (allowlist vitality, scanner blind spots) — see MEDIA_CONVERSION_HARDENING_AUDIT.md
python3 crates/dev/scripts/media_conversion_delivery_heatmap.py --deep
```

## Heatmap baseline (M39)

File: [`crates/dev/src/fixtures/media_conversion_delivery_heatmap_baseline.json`](../crates/dev/src/fixtures/media_conversion_delivery_heatmap_baseline.json)

- `numeric_forgery_offenders`: count of production lines matching the numeric-forgery pattern set (must stay **0** unless the allowlist or gate changes are audited)
- `gate_log_anomaly_count`: must stay **1** while only `delivery_fallback_audit` emits `log_anomaly!`

After an intentional change, update the baseline and document the reason in the PR.

## Python orchestration

[`crates/dev/scripts/media_scope.py`](../crates/dev/scripts/media_scope.py) mirrors M1–M39 for drag-and-drop / verify routing and references the same `[delivery fallback:…]` log tag in Rust traces.

---

