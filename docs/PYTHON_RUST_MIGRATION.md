# Python to Rust Migration Contract

Production and CI orchestration are Rust-first. Python remains only where its
ecosystem is part of the implementation or where a compatibility bridge is
still covered by tests.

## Completion status (2026-07-22)

Operational migration is complete for production and CI entry points:

- Repository documentation, workflows, production hints, and release automation
  select the Rust binaries.
- The canonical Rust `run_training` binary owns orchestration and loads the
  shared `training_rules.json` directly.
- A repository-wide caller audit found no production or CI invocation of a
  migrated Python entry point.

Physical removal is intentionally separate from operational completion. Python
files in the retained categories below remain only while their ecosystem,
compatibility, or test role is still required.

## Canonical commands

| Task                          | Source-tree command                                          | Release command                          |
| ----------------------------- | ------------------------------------------------------------ | ---------------------------------------- |
| Training orchestration        | `cargo run --locked -p dev --bin run_training -- <options>`  | `target/release/run_training <options>`  |
| Full repository checks        | `cargo run --locked -p dev --bin check_all -- <options>`     | `target/release/check_all <options>`     |
| Dependency installation       | `cargo run --locked -p dev --bin install_deps -- <options>`  | `target/release/install_deps <options>`  |
| iCloud Photos import          | `cargo run --locked -p dev --bin icloud_import -- <options>` | `target/release/icloud_import <options>` |
| Incremental application build | `cargo run --locked -p dev --bin smart_build -- <options>`   | `target/release/smart_build <options>`   |

The Rust training orchestrator may invoke Python ML workers when a model step
depends on Python-native libraries. That delegation does not make the legacy
Python orchestrator the public entry point.

## Migrated operational entry points

The following Python script names have same-purpose Rust binaries under
`crates/dev/src/bin/`. Callers must use the Rust binary for production and CI.

| Rust binary                               | Non-canonical Python reference               |
| ----------------------------------------- | -------------------------------------------- |
| `backfill_directory_scores`               | `backfill_directory_scores.py`               |
| `cache_cleaner`                           | `cache_cleaner.py`                           |
| `check_all`                               | `check_all.py`                               |
| `collect_optimized`                       | `collect_optimized.py`                       |
| `create_live_photo`                       | `create_live_photo.py`                       |
| `database_manager`                        | `database_manager.py`                        |
| `generate_test_media`                     | `generate_test_media.py`                     |
| `icloud_import`                           | `icloud_import.py`                           |
| `install_deps`                            | `install_deps.py`                            |
| `mfb_rust_toolchain`                      | `mfb_rust_toolchain.py`                      |
| `normalize_stale_embed_measurement_slots` | `normalize_stale_embed_measurement_slots.py` |
| `post_training_closure`                   | `post_training_closure.py`                   |
| `run_training`                            | `run_training.py`                            |
| `sandbox_validate`                        | `sandbox_validate.py`                        |
| `session_audit`                           | `session_audit.py`                           |
| `setup_private_db`                        | `setup_private_db.py`                        |
| `start_training_four`                     | `start_training_four.py`                     |
| `training_pipeline`                       | `training_pipeline.py`                       |

Additional Rust-native replacements use clearer names: `delivery_heatmap`
replaces the operational role of `media_conversion_delivery_heatmap.py`, and
`corpus_thresholds` replaces `mfb_corpus_thresholds.py` for Rust callers.

## Intentionally retained Python

- ML implementation: clustering, regression-model training, and Python API
  workers that depend on NumPy, scikit-learn, or LightGBM.
- Tests and fixtures: Python contract tests, media fixture helpers, workflow
  validation, and test-only compatibility coverage.
- Fuzzing: OSS-Fuzz and Python harness integration.
- Compatibility bridges: shared configuration/logging helpers and legacy
  entry points that still have an in-repository caller or parity test.

## Removal gate

A Python file may be archived or removed only when all of these are true:

1. A Rust implementation covers every supported option, exit code, side
   effect, and log contract.
2. Repository-wide reference search finds no production or CI caller.
3. Rust unit/integration tests and the Python compatibility contract tests pass.
4. Documentation and packaged automation point to the Rust binary.

This policy deliberately avoids deleting model code or compatibility helpers
solely because a similarly named Rust file exists.
