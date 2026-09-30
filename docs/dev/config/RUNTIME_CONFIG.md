# Image Runtime Preferences

`img` loads versioned JSON preferences before database initialization, worker
startup or media processing. The native GUI's image operations launch `img`
and therefore inherit the same user configuration. No dependency is required
beyond the existing JSON parser.

## Files And Precedence

From lowest to highest priority:

1. Built-in defaults.
2. Recognized legacy environment preferences.
3. `$XDG_CONFIG_HOME/modern-format-boost/config.json`, or
   `~/.config/modern-format-boost/config.json` when XDG is absent.
4. `mfb.json` in the process working directory.
5. An explicit `--config /absolute/path/preferences.json` overlay.
6. Explicit command-line flags.

An example with all defaults is [mfb.example.json](mfb.example.json). Each file
must contain `"config_version": 1`; other fields may be omitted and inherit
lower-priority values. Unknown fields, invalid types, unsupported versions and
out-of-range settings stop startup with an error. `--no-config` skips files,
but legacy environment settings and explicit flags still apply.

```sh
img config show --effective
img --config /path/to/preferences.json config show --effective
img run /path/to/images --allow-database --quality-heuristic
img fast-img /path/to/images --jpeg-effort 11 --fallback-policy strict
img fast-img /path/to/images --fallback-policy same-semantics --tool-policy single
img fast-img /path/to/images --shortest-path --photos-backend native \
  --photos-import-root Archive --photos-album-name Family \
  --preserve-folder-structure=false --photos-native-batch-size 250 \
  --photos-verification-batch-size 500 --photos-adaptive-batching
```

`config show` prints JSON containing `config` and a `sources` map. It does not
initialize databases, acquire media locks or process files. Media runs also
log the resolved configuration. Booleans accept a bare enable flag or an
explicit `=false`, so a saved opt-in can be disabled for one run.

## Policy Fields

| Field | Default | Meaning |
| --- | --- | --- |
| `img.allow_database` | `false` | Permits the optional image-quality database stack. Quality inference must also be enabled. Does not suppress Photos custody verification or explicit database maintenance commands. |
| `img.quality_heuristic` | `false` | Enables existing optional quality inference. A heuristic does not replace delivery proof. |
| `img.jpeg_effort` | `11` | JPEG bitstream transcode effort, 1 through 11. Pixel encoding retains its normal/ultimate mode policy. |
| `img.fallback_policy` | `strict` | `strict`: one requested JPEG encode attempt. `same-semantics`: allows compatibility retries and e11 to e10 on original JPEG bytes. `repair`: also permits the existing guarded repair paths. All delivery and exact reconstruction checks remain mandatory. |
| `tools.policy` | `fallback` | `single` forbids alternate image encoding/recovery tools. `fallback` permits them only where the fallback policy allows recovery. Metadata and verification tools remain required. |
| `tools.paths` | `{}` | Tool-name to absolute executable-path overrides, also accepted as repeated `--tool NAME=PATH` flags. Existing tool health checks still apply. |
| `photos.backend` | `auto` | Native PhotoKit when available, with observable AppleScript compatibility fallback before import intent; `native` requires PhotoKit, `applescript` explicitly selects compatibility. `photokit` is an alias for `native`. |
| `photos.import_root` | `null` | Top-level Photos folder name. `null` keeps the existing default. |
| `photos.album_name` | `null` | Base Photos album name. `null` keeps the source-derived name. |
| `photos.preserve_folder_structure` | `true` | Appends relative source subfolders below the configured names. |
| `photos.native_batch_size` | `100` | Native import transaction size, 1 through 1000. |
| `photos.import_batch_size` | `50` | AppleScript scheduling batch, 1 through 50. Existing transaction safety cap of 10 remains. |
| `photos.verification_batch_size` | `250` | Native verification window and shared Photos query cap, 1 through 1000, independent of transaction size. AppleScript retains its existing checkpoint windows. |
| `photos.adaptive_batching` | `false` | Opt-in native transaction sizing after complete verified cycles. Also accepts `--photos-adaptive-batching=false`. |
| `photos.native_min_batch_size` | `50` | Adaptive lower bound, at least 1 and no greater than the upper bound. |
| `photos.native_max_batch_size` | `1000` | Adaptive upper bound, at most 1000. Initial transaction size must fall within these bounds when adaptation is enabled. |
| `photos.target_batch_seconds` | `10` | Verified-cycle latency target, 1 through 600 seconds. |

Native import and verification windows can have unequal sizes. Only committed
identifiers are queued, never media bytes; the pending identifier count stays
below the verification size plus the configured maximum transaction size.
Verification applies backpressure once its threshold is reached. One subsequent
transaction can overlap verification and checkpointing, with one writer only.
Every successful verification window creates a durable checkpoint; an error
stops submission and leaves sources and native journals for reconciliation.
An in-flight reply is drained even on verification failure before leaving the
transaction boundary; journals remain authoritative for recovery.

Adaptive mode halves transactions on high memory pressure or a slow verified
cycle, and doubles them after two cycles below half the target, within the
configured bounds. Split verification tails do not count as separate healthy
cycles. Decisions are logged and never retry a failed transaction. The native
profile reports committed/verified counts, peak identifier backlog, transaction
p50/p90/p95/p99 and verified throughput; these are measurements of this run,
not a Photos or iCloud completion guarantee.

`MFB_FAST_IMG_ICLOUD_VERIFY_BATCH_SIZE` remains supported as a legacy preference
with its historical cap of 128; JSON and explicit CLI values override it.
Without this override, the configured shared query cap is now 250 instead of
the older unconfigured compatibility default of 64.

Photos names are single components: empty names, dot components, path
separators and control characters are rejected. Both primary output imports
and modern-format original imports use the same naming policy. A checkpoint
binds the naming policy before import; a changed policy cannot reuse that
checkpoint's custody proofs. Restore the previous names or start a separate
task with `--no-resume`. Legacy checkpoints containing Photos state require
legacy naming until reconciled.

The legacy `--allow_expert_options` flag maps to `repair`; an explicit
`--fallback-policy` takes precedence. The new default is strict: JPEG e11
failure no longer silently requests e10 in `img`. No setting disables exact
JPEG reconstruction, metadata preservation, source retention on failure or
pre-delete Photos custody verification.

This configuration layer currently belongs to `img` and its GUI-launched
operations. It does not claim that every developer/debug environment variable
or the separate `vid` configuration has been migrated. Database credentials
remain in the existing private database configuration. Real-library throughput,
TCC prompts, crash/restart acceptance and manual Photos comparisons are
separate from parser and isolated regression checks.

Native backend selection applies to the checkpointed output importer. The
modern-original tier and the noncheckpoint compatibility importer currently
support AppleScript only; explicit `native` selection fails before importing
on those paths. `auto` retains their established compatibility behavior.

## Validation Status (2026-09-28)

This configuration increment is implemented. The wider
Photos throughput project remains partial, not performance-accepted.
Subsequent implementation and real-library measurements are tracked in
[Photos import follow-up](../../hardening/PHOTOS_IMPORT_FOLLOWUP.md).

The first configuration increment used `--locked --profile dev-fast -j 2`
without changing Cargo manifests or the lockfile:

| Check | Result |
| --- | --- |
| Foundation configuration layer tests | 2 passed |
| Configured JPEG effort, database gates and Photos naming/checkpoint tests | 3 passed |
| CLI override and invalid-argument tests | 2 passed |
| Real CLI with isolated HOME/XDG/CWD: precedence, no-config and invalid input | 1 passed |
| JPEG command-attempt policy regression | 1 passed |
| Native journal identity mapping regression | 1 passed |
| Scoped import metrics regression | 1 passed |
| Repeated APP13/IPTC JPEG reconstruction using installed cjxl/djxl/ExifTool | 1 passed, not skipped |
| `cargo clippy --locked --profile dev-fast -j 2 -p img --all-targets -- -D warnings` | Passed |
| Photos helper Swift typecheck (`-parse-as-library`) | Passed |
| `git diff --check` | Passed |

Pre-push expansion also checked all IMG targets. One incomplete-JPEG message
regression failed initially; the production summary was corrected while keeping
the failed outcome and source-retention assertions. The affected IMG library
suite then passed all 96 tests; 158 CLI tests and 27 integration tests had passed
in the all-target run. Native GUI self-tests passed in English, Chinese and
Japanese from an isolated temporary bundle compiled with warnings as errors.
macOS emitted a sandbox-extension warning for that temporary app despite the
successful self-tests; signed deployment and live TCC remain unverified.

These checks do not prove TCC behavior, real Photos crash recovery, or sustained
import throughput. Local validation did not perform a real-library import.
The subsequent dependency/artifact refresh and scheduling checks are recorded
in the changelog. Git publication and remote CI are
separate evidence: verify the exact pushed SHA in the quality workflow.
The September 30 acceptance record supersedes the earlier no-live-import status:
native 1K/10K originals and real recovery were exercised on the original debug
library. Manual Photos, larger production stress and iCloud evidence remain
separate, as listed in that record.

Tool selection currently exposes executable paths and single/fallback policy.
The established recovery ordering is retained; arbitrary per-format tool
reordering and a GUI preferences editor are not implemented by this increment.
