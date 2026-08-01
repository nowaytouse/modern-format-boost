# Python to Rust Migration Contract

Production and CI orchestration are Rust-first; documentation and packaged
operational entry points follow the same contract. Python remains only where
its ecosystem is part of the implementation or where a tested compatibility
bridge is still required.

## Completion status (2026-08-01)

Operational migration is complete for production and CI, documentation, and
release packaging:

- Active workflows and production hints invoke Rust binaries.
- Release archives build and package the migrated Rust tools instead of the
  legacy Python entry points.
- Release archives contain only the Python-native ML workers and their shared
  path helper: `quality_regression_model.py`, `loop_intent_clustering.py`, and
  `fastmode_paths.py`.
- The canonical Rust `run_training` binary owns orchestration and loads
  `training_rules.json` directly.

Physical removal remains deliberately separate. A Python compatibility file in
the source tree is not a production entry point and may remain while a direct
caller or parity test still depends on it.

## Canonical commands

| Task                          | Source-tree command                                          | Release command           |
| ----------------------------- | ------------------------------------------------------------ | ------------------------- |
| Training orchestration        | `cargo run --locked -p dev --bin run_training -- <options>`  | `run_training <options>`  |
| Full repository checks        | `cargo run --locked -p dev --bin check_all -- <options>`     | `check_all <options>`     |
| Dependency installation       | `cargo run --locked -p dev --bin install_deps -- <options>`  | `install_deps <options>`  |
| iCloud Photos import          | `cargo run --locked -p dev --bin icloud_import -- <options>` | `icloud_import <options>` |
| Incremental application build | `cargo run --locked -p dev --bin smart_build -- <options>`   | `smart_build <options>`   |

The Rust training orchestrator may invoke Python ML workers when a model step
depends on Python-native libraries. That delegation does not make a legacy
Python orchestrator a public entry point.

## Intentional operational boundaries

- CI media dependency bootstrap is a standalone Rust binary compiled directly
  with `rustc`, avoiding a Cargo/native-library bootstrap cycle.
- Active GitHub workflows do not invoke `.py`, `.sh`, or `.bash` script files.
- `check_all --fix` owns source formatting; `kondo` cache cleanup belongs to
  `smart_build --clean`.
- Packaged ML workers are an explicit allowlist, not a recursive copy of
  `crates/dev/scripts`.
- Packaged Rust training tools resolve sibling binaries and allowlisted Python
  workers relative to the release directory.
- `smart_build --sync` verifies nested signing and reseals the outer App bundle
  after foundation-library updates.

## Migrated operational entry points

The following source-tree Python names have Rust operational replacements.
Production, CI, documentation, and release archives use the Rust binary.

| Rust binary                               | Compatibility Python source                  |
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

`delivery_heatmap` replaces `media_conversion_delivery_heatmap.py`;
`corpus_thresholds` replaces `mfb_corpus_thresholds.py`.

## Intentionally retained Python

- ML implementation: clustering and regression-model training that depends on
  NumPy, scikit-learn, HDBSCAN, or LightGBM.
- Tests and fixtures: Python contract tests and test-only media helpers.
- Fuzzing: OSS-Fuzz and Python harness integration.
- Compatibility bridges: shared helpers and legacy source-tree entry points
  that still have a direct caller or parity test.

Only the ML implementation allowlist is included in release archives.
Compatibility bridges remain source-only.

## Removal gate

A Python file may be removed only when all of these are true:

1. The Rust implementation covers every supported option, exit code, side
   effect, and log contract.
2. Repository-wide reference search finds no production, CI, compatibility, or
   test caller.
3. Rust unit/integration tests and relevant Python compatibility tests pass.
4. Documentation and packaged automation point to the Rust binary.

This gate forbids deleting model code or a compatibility helper solely because
a similarly named Rust file exists.
