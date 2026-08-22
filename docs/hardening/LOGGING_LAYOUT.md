# SOURCE: LOGGING_LAYOUT.md

# Logging layout contract

Python SSOT: `crates/dev/scripts/mfb_log_paths.py`  
Rust mirror: `foundation::logging::LogConfig::unified_log_dir()` and `progress_mode::set_default_run_log_file`.

## Log root resolution

| Priority | Source                        | Typical use                                       |
| -------- | ----------------------------- | ------------------------------------------------- |
| 1        | `MFB_LOG_DIR`                 | Session override, CI, manual                      |
| 2        | `MFB_HOME_ROOT/logs`          | Tests, app bundle cache (`FROM_APP`)              |
| 3        | `~/.modern_format_boost/logs` | Default for dev, drag-and-drop, training, img/vid |
| 4        | System temp                   | Last resort when `$HOME` is unavailable (Rust)    |

**Never** writes under `<repo>/logs` or `target/training_*`. If `MFB_LOG_DIR` (or an old shell export) still points at those paths, Python/Rust **coerce** to the persistent home log root and print a one-line stderr warning. If `MFB_HOME_ROOT` itself is mis-set to the workspace, Python/Rust reject `<repo>/logs` and fall back to `~/.modern_format_boost/logs` before using temp as a last resort.

Every production script calling `guard_main()` runs `ensure_unified_log_dir()` (except under `PYTEST_CURRENT_TEST`). Subprocess env from `child_env_for_script()` always carries the coerced `MFB_LOG_DIR`.

Drag-and-drop and `run_training.py` re-pin `MFB_LOG_DIR` at session start so img/vid Rust workers share the same directory.

## Filename patterns (under log root)

| Pattern                         | Owner                        | Purpose                                                           |
| ------------------------------- | ---------------------------- | ----------------------------------------------------------------- |
| `{binary}_run_{stamp}.log`      | Rust `progress_mode`         | Human-readable run trace                                          |
| `{binary}_{stamp}.jsonl`        | Rust `init_logging`          | Structured tracing                                                |
| `MFB_Session_{stamp}.log`       | drag-and-drop                | Session summary                                                   |
| `verbose_{stamp}.log`           | drag-and-drop                | ROUTED / HANDOFF / RSYNC audit                                    |
| `Bundle_{stamp}/`               | drag-and-drop                | Archived session artifacts + `manifest.json` (move, not merge)    |
| `session_audit_{stamp}.jsonl`   | drag-and-drop                | Structured session audit (ROUTED / HANDOFF / heartbeat / archive) |
| `mfb_audit_{day}.log`           | `mfb_logger`                 | Python tooling audit                                              |
| `diagnostic_report_{stamp}.txt` | `verify.py`                  | Post-batch integrity report                                       |
| `run_training_{stamp}.log`      | `run_training.py`            | Training session log                                              |
| `training_session_audit.jsonl`  | `run_training.py`            | Lifecycle audit: start, phase, heartbeat, signals, exit           |
| `training_session_exit.json`    | `run_training.py`            | Last exit snapshot (`reason`, `phase`, `exit_code`)               |
| `training_tier_audit.jsonl`     | `run_training.py`            | Static tier probe stream (per session; archived on exit)          |
| `replica_audit_{stamp}.jsonl`   | `run_training.py`            | Phase 1/2 replica audit                                           |
| `TrainingBundle_{stamp}/`       | `run_training.py`            | Archived training artifacts (move, not merge) + `manifest.json`   |
| `run_training.pid`              | `run_training.py` / launcher | Background / lane PID                                             |

### Parallel training lanes

Four concurrent jobs use **separate** `MFB_LOG_DIR` subdirectories (never one shared `run_training.pid`):

| Lane dir       | Command                                                                            |
| -------------- | ---------------------------------------------------------------------------------- |
| `static_high/` | `--training-mode static --label high --no-loop`                                    |
| `static_low/`  | `--training-mode static --label low --no-loop`                                     |
| `loop_high/`   | `--training-mode loop --loop-intent-label high`                                    |
| `loop_low/`    | `--training-mode loop --loop-intent-label low` (grey-zone / uncertain loop corpus) |

Launcher (detached, `start_new_session`):

```bash
MFB_INVOKER=direct python3 crates/dev/scripts/start_training_four.py
# or ~/.modern_format_boost/bin/start_training_four.sh
```

On normal exit, `run_training.py` moves session files into `TrainingBundle_{stamp}/` under that lane (same policy as drag-and-drop `Bundle_*`: no giant merged log). `manifest.json` includes an `exit` block when `training_session_exit.json` was written.

If a lane dies with no `run_training_*.log` left, check **`training_session_audit.jsonl`** (heartbeats every 60s during scan) and **`training_session_exit.json`** (SIGTERM/SIGINT/atexit). **SIGKILL/OOM** leaves only the last heartbeat — no Python exit handler runs.

```bash
tail -20 ~/.modern_format_boost/logs/loop_high/training_session_audit.jsonl
cat ~/.modern_format_boost/logs/loop_high/training_session_exit.json
```

Set `MFB_TRAINING_ARCHIVE_LOGS=0` to skip archival. Set `MFB_TRAINING_SESSION_STAMP` before launch so shell redirect and Rust replica audit share one stamp. Override heartbeat interval with `MFB_TRAINING_HEARTBEAT_SECS` (default 60).

`{stamp}` = `YYYYMMDD_HHMMSS` (local). Legacy bundles may use `YYYY-MM-DD_HH-MM-SS`; Python `parse_session_stamp` accepts both.

## Overrides

```bash
export MFB_LOG_DIR=/path/to/logs   # force all tooling + Rust workers
export MFB_LOG_PROGRESS=1        # include mfb::progress in Rust run/jsonl logs
export MFB_LOG_PTY_PROGRESS=1      # include indicatif/progress lines in MFB_Session PTY capture
```

On exit, drag-and-drop moves worker `img_*` / `vid_*` logs and jsonl traces, `MFB_Session_*`, `verbose_*`, `session_audit_*`, and `diagnostic_report_*` from the session window into `Bundle_{stamp}/` with a `manifest.json` file list.

## Training corpus thresholds

Python SSOT: `crates/dev/scripts/mfb_corpus_thresholds.py`  
Rust SSOT: `foundation::algorithm_runtime` (`loop_corpus_*`, `quality_corpus_*`, `TrainingCorpusMaturity`).

`db-health` / `check_database_health` reports combined loop + static shortfalls; counts are clamped with `.max(0)` before evaluation.

## Verification

```bash
# Default log root (~/.modern_format_boost/logs) or pass explicit paths:
python3 crates/dev/scripts/verify.py --verify /path/to/src /path/to/opt
python3 crates/dev/scripts/verify.py ~/.modern_format_boost/logs/Bundle_*/ --verify src/ opt/

cargo run -p dev --bin verify_log_layout
```

---

