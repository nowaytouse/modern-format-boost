# Python to Rust Migration Contract

Production and CI orchestration are Rust-first; release tooling and
non-training developer commands are Rust only. Python is retained solely
inside the training/ML implementation and in tests or fuzz harnesses that
exercise that ecosystem.

## Completion status (2026-08-08)

The operational migration is physically complete:

- Every retired operational Python entry point is absent from
  `crates/dev/scripts` and has a canonical Rust binary.
- Active workflows invoke Rust tools and do not execute script files.
- Release archives package Rust tools plus only the explicitly allowlisted ML
  workers: `quality_regression_model.py`, `loop_intent_clustering.py`, and their
  shared `fastmode_paths.py` helper.
- `mfb_logger.py` was removed because it had no repository caller; logging is
  owned by the Rust infrastructure logger.
- The non-executable Python runtime-gate dictionary was converted to the
  language-neutral `mfb_runtime_env.example.env` template.

The exact file boundary is locked by
`crates/dev/scripts/tests/test_python_rust_migration_contract.py`: it rejects a
return of any retired Python tool, requires every Rust replacement, and fails
if a new top-level Python script is not in the training/ML allowlist.

## Canonical commands

| Task                          | Source-tree command                                          | Release command           |
| ----------------------------- | ------------------------------------------------------------ | ------------------------- |
| Training orchestration        | `cargo run --locked -p dev --bin run_training -- <options>`  | `run_training <options>`  |
| Full repository checks        | `cargo run --locked -p dev --bin check_all -- <options>`     | `check_all <options>`     |
| Dependency installation       | `cargo run --locked -p dev --bin install_deps -- <options>`  | `install_deps <options>`  |
| iCloud Photos import          | `cargo run --locked -p dev --bin icloud_import -- <options>` | `icloud_import <options>` |
| Incremental application build | `cargo run --locked -p dev --bin smart_build -- <options>`   | `smart_build <options>`   |

Rust training orchestration may invoke Python-native ML workers. That is an
implementation boundary, not a second public command surface.

## Operational boundaries

- CI media dependency bootstrap is a standalone Rust binary compiled directly
  with `rustc`, avoiding a Cargo/native-library bootstrap cycle.
- Active GitHub workflows do not invoke `.py`, `.sh`, or `.bash` script files.
- `check_all --fix` owns source formatting; `kondo` cache cleanup belongs to
  `smart_build --clean`.
- Packaged ML workers use an explicit allowlist instead of recursively copying
  `crates/dev/scripts`.

## Physically retired operational Python

| Rust owner                                | Removed Python source                        | Behavioral lock                                                             |
| ----------------------------------------- | -------------------------------------------- | --------------------------------------------------------------------------- |
| `cache_cleaner`                           | `cache_cleaner.py`                           | Rust binary unit tests                                                      |
| `check_all`                               | `check_all.py`                               | Rust binary unit tests and CI workflow contract                             |
| `collect_optimized`                       | `collect_optimized.py`                       | snapshot/restore failure-propagation unit tests                             |
| `create_live_photo`                       | `create_live_photo.py`                       | `create_live_photo` runtime integration test                                |
| `generate_test_media`                     | `generate_test_media.py`                     | FFmpeg failure-propagation unit test and CI fixture generation              |
| `icloud_import`                           | `icloud_import.py`                           | album, rename, lock, and preflight unit tests                               |
| `install_deps`                            | `install_deps.py`                            | dependency-plan and canonical follow-up command unit tests                  |
| `delivery_heatmap`                        | `media_conversion_delivery_heatmap.py`       | delivery heatmap unit tests and hardening contract                          |
| `mfb_rust_toolchain`                      | `mfb_rust_toolchain.py`                      | Rust toolchain resolution unit tests                                        |
| `normalize_stale_embed_measurement_slots` | `normalize_stale_embed_measurement_slots.py` | vector normalization and single-transaction SQL unit tests                  |
| `sandbox_validate`                        | `sandbox_validate.py`                        | sandbox argument and command-construction unit tests                        |
| `session_audit`                           | `session_audit.py`                           | explicit audit-write error unit tests                                       |
| `setup_private_db`                        | `setup_private_db.py`                        | runtime tests for default, existing, legacy, override, and EOF cancellation |

Deletion is part of the contract. Reintroducing one of these Python files as a
wrapper or compatibility copy fails the migration contract test.

## Intentionally retained Python

The top-level Python allowlist is finite and training-specific:

- Training orchestration and lifecycle: `run_training.py`,
  `training_pipeline.py`, `start_training_four.py`,
  `post_training_closure.py`, `backfill_directory_scores.py`, and
  `database_manager.py`.
- Python-native ML workers: `quality_regression_model.py`,
  `loop_intent_clustering.py`, and `python_api.py`.
- Training and ML bridges: `fabrication_policy.py`, `fastmode_paths.py`,
  `media_scope.py`, `mfb_config_load.py`, `mfb_corpus_thresholds.py`,
  `mfb_dylib.py`, `mfb_entry_guard.py`, `mfb_log_paths.py`,
  `mfb_performance.py`, `mfb_training_scan.py`,
  `mfb_training_session_audit.py`, and `mfb_ui_tokens.py`.
- Tests and fixtures: Python contract tests and Python-native training tests.
- Fuzzing: OSS-Fuzz and Python harness integration.

Only the three release-worker files named above are copied into release
archives. The rest remain source-only training implementation.

## Removal gate

A future Python-to-Rust replacement is complete only when all of these are
true:

1. Rust covers every supported option, exit status, side effect, and error
   boundary.
2. Repository-wide reference search finds no remaining caller of the Python
   implementation.
3. Equivalent Rust unit or integration tests pass before Python compatibility
   tests are removed.
4. Documentation, CI, and release automation name only the Rust entry point.
5. The Python implementation is physically deleted and the exact allowlist
   contract still passes.

Do not delete Python-native model code merely because a similarly named Rust
orchestrator exists.
