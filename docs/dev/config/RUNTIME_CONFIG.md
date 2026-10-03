# Runtime Preferences

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
img config validate
img config path
img config init ./preferences.json
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
`config validate` checks the same effective policy. `config path` reports the
user/project/explicit paths and precedence without loading potentially invalid
files. `config init PATH` atomically creates default JSON at an explicit path;
an existing file is never replaced. These commands do not access Photos.

## Policy Fields

### Native GUI Overrides

The gear button shows the image workflow currently selected, Photos settings
for Fast IMG, and shared Performance settings. Standard and Fast IMG retain
independent JPEG transcoding effort (1 through 11), fallback,
quality heuristic and database overrides. Existing shared image preferences
migrate once to Fast IMG; subsequent edits and resets stay independent.
Photos exposes backend selection, expandable native/AppleScript/verification batch sizes,
adaptive sizing with minimum/maximum/target duration, root folder, album name
and subfolder preservation. Configuration-file overrides and per-file failure
policies appear only in Developer mode, enabled in About. Video exposes HEVC/AV1
on the main screen and an independent developer file-error mode. AV1 disables
Apple compatibility; video quality and preset are not exposed by the current
CLI and therefore are not editable controls.
Video settings apply only to standard `vid run` processing. Fast Video uses
`vid fast-gif`, which supports neither option; explicit launcher options there
are rejected and the GUI disables the settings button for that operation.
The image file-error override also belongs to standard processing. FastImg
retains its existing checkpointed per-file failure handling; the launcher rejects
an explicit error-mode override there rather than pretending it controls the
FastImg encoding waves. Its fallback, effort and other image preferences still apply.

Image and Photos effective values are queried from the backend, not duplicated
in Swift. The GUI displays the resolved value without an extra inherited choice;
opening and applying unchanged controls does not create overrides. Developer
tooltips include configuration keys and sources. Numeric fields use steppers;
quality inference/database use Enabled/Disabled menus and Photos booleans use
ordinary two-state checkboxes. Apply
validates the resolved standard and Fast IMG profiles, including adaptive
batch bounds, before persisting. Configuration query failures stay visible.
Overrides are saved in native GUI preferences,
passed as explicit launcher options, and only forwarded to the matching media
pipeline. Reset This Tab removes that tab's overrides, not the other tab or
the runtime JSON. Invalid saved overrides and unreadable explicit files fail
visibly instead of silently reverting to defaults. JSON schema validation
remains owned by `img`. Tool executable paths remain available through JSON or
direct `img --tool` overrides.

The launcher accepts `--img-config`, `--img-fallback-policy`,
`--img-jpeg-effort`, `--img-quality-heuristic=true|false`,
`--img-allow-database=true|false`, `--img-error-mode`, `--vid-codec`, and
`--vid-error-mode`. Error modes are `log-and-continue` and `fail-fast`; overrides
apply only to the child process, including PTY launches, without mutating the
launcher's global environment. Encoding options are not sent to verification,
restore or maintenance tools.
The launcher also accepts the shared `--photos-*` options and
`--preserve-folder-structure` for Fast IMG only. `img` and the launcher use the
same typed Photos argument definition, including `--photos-native-min-batch-size`,
`--photos-native-max-batch-size` and `--photos-target-batch-seconds`.

Fallback selects permitted attempts; error mode selects whether a recoverable
file failure stops processing. Strict fallback is not fail-fast. Failed files
retain their sources and count separately from intentional skips; fatal errors
still stop. Versioned `MFB_BATCH_RESULT` events report each completed media
child's exit and known counts. Missing counts are JSON `null`, not fabricated
zeroes; the GUI retains that uncertainty when aggregating results. Error logs
and nonzero exits are preserved even when a count summary is unavailable.

### Runtime JSON

| Field | Default | Meaning |
| --- | --- | --- |
| `img.allow_database` | `false` | Permits the optional image-quality database stack. Quality inference must also be enabled. Does not suppress Photos custody verification or explicit database maintenance commands. |
| `img.quality_heuristic` | `false` | Enables existing optional quality inference. A heuristic does not replace delivery proof. |
| `img.jpeg_effort` | `11` | JPEG bitstream transcode effort, 1 through 11. Pixel encoding retains its normal/ultimate mode policy. |
| `img.fallback_policy` | `strict` | `strict`: one requested JPEG encode attempt. `same-semantics`: allows compatibility retries and e11 to e10 on original JPEG bytes. `repair`: also permits the existing guarded repair paths. All delivery and exact reconstruction checks remain mandatory. |
| `tools.policy` | `fallback` | `single` forbids alternate image encoding/recovery tools. `fallback` permits them only where the fallback policy allows recovery. Metadata and verification tools remain required. |
| `tools.paths` | `{}` | Tool-name to absolute executable-path overrides, also accepted as repeated `--tool NAME=PATH` flags. Existing tool health checks still apply. |
| `performance.mode` | `adaptive` | Shared IMG/VID memory-aware scheduling. `relaxed`, `balanced`, and `tight` request fixed tiers without removing memory safety caps. `--performance` overrides JSON; legacy `MFB_PERF_TIER` remains lower priority. |
| `photos.backend` | `native` | Requires PhotoKit with a proven target library. `auto` opts into observable AppleScript fallback before import intent; `applescript` explicitly selects compatibility. `photokit` is an alias for `native`. |
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

Default names now derive directly from the original input directory for both
converted outputs and retained originals. Collision-generated output names
such as `Batch_optimized_2` and custom output paths cannot create a second
output-derived album. Legitimate suffixes in source names are preserved.
Existing checkpointed asset UUIDs remain authoritative and are not reimported;
this fix does not automatically merge or delete previously created albums.

## Verification And Scheduling (2026-10-01)

Apple Silicon memory sampling reads the actual `vm_stat` page size rather than
assuming 4096 bytes. Admission workers share a sample for at most 250 ms,
including failed samples; expired samples are re-probed. This corrects a 4x
underestimate on 16 KiB systems without removing memory-pressure limits.

Gate 1 performs cheap count/size checks before expensive proofs, records each
executed check's duration, and stops after the first mandatory failure. Remaining
checks are explicitly reported as not run; import and cleanup stay blocked.
Orientation tags are queried in batches of up to 128 files, with exact per-file
response accounting. Missing, duplicate, unexpected or error responses fail
closed. JPEG reconstruction and decoder checks remain separate and mandatory.

An isolated 32-image comparison using the installed ExifTool returned identical
orientation results: batch 115 ms versus individual probes 2525 ms. This is a
local orientation-probe measurement, not a full-pipeline or Photos speed claim.
No production library was accessed during this increment's tests.

Embedded metadata audits also query source, output and optional XMP in a single
bounded invocation (up to four unique files), reusing the source-sidecar map
inside that audit. No cross-run or timestamp-only proof cache is introduced.
Responses must identify each requested path exactly once; empty output and
per-file tool errors cannot become an empty successful metadata map. A local
pair comparison returned identical maps in 115 ms versus 238 ms separately.
Sidecar precedence, wrong-source rejection and exact APP13/JXL reconstruction
requirements remain covered by regression tests.

The legacy `--allow_expert_options` flag maps to `repair`; an explicit
`--fallback-policy` takes precedence. The new default is strict: JPEG e11
failure no longer silently requests e10 in `img`. No setting disables exact
JPEG reconstruction, metadata preservation, source retention on failure or
pre-delete Photos custody verification.

Image policy belongs to `img`; the launcher and `vid` also resolve the shared
performance policy. Not every developer/debug variable or separate video
quality setting has been migrated. Database credentials
remain in the existing private database configuration. Real-library throughput,
TCC prompts, crash/restart acceptance and manual Photos comparisons are
separate from parser and isolated regression checks.

Native backend selection applies to checkpointed outputs, modern originals and
generic media import. Original imports have a separate durable journal namespace
and persist verified UUIDs after each window. Ephemeral XMP staging paths may
change on resume only when filename, resource type and original-byte hashes
still match. Generic media are batched by source parent/album, with durable
state outside user media folders. Previously verified assets are rechecked by
UUID rather than filename; explicit Native failures never silently fall back.

Fast IMG reports `MFB_FAST_IMG_RESULT` before propagating delivery/finalization
errors. Known success/failure/skip/ignored counts survive a nonzero exit;
unprocessed entries and unknown retention have explicit fields. Primary cleanup
completion is persisted before the independent original import begins. Retained
primary failures remain eligible for `--retry` even with pending original imports.

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
reordering is not implemented. The September 30 GUI editor adds per-media
overrides above; it does not rewrite runtime JSON or change tool ordering.
