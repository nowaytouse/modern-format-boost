# Runtime Preferences

`img` and `vid` load the same typed, versioned JSON model before database
initialization, worker startup or media processing. Each accepts an independent
explicit configuration file; GUI overrides use the matching worker's resolver.
No dependency is required beyond the existing JSON parser.

## Files And Precedence

From lowest to highest priority:

1. Built-in defaults.
2. Recognized environment preferences, including supported legacy preferences.
3. `$XDG_CONFIG_HOME/modern-format-boost/config.json`, or
   `~/.config/modern-format-boost/config.json` when XDG is absent.
4. `mfb.json` in the process working directory.
5. An explicit `--config /absolute/path/preferences.json` overlay.
6. Explicit command-line flags.

An example with all defaults is [mfb.example.json](mfb.example.json). Each file
must contain `"config_version": 1`; other fields may be omitted and inherit
lower-priority values. Unknown fields, invalid types, unsupported versions and
out-of-range settings stop startup with an error. `--no-config` skips files,
but environment settings and explicit flags still apply.

```sh
img config show --effective
img config validate
vid config show --effective
vid config validate
vid --config /path/to/video.json config show --effective
vid run /path/to/videos --codec av1
img run /path/to/images --error-mode log-and-continue
vid --error-mode fail-fast config show --effective
vid --tool 'ffmpeg=/path/to/tool dir/ffmpeg=custom' config show --effective
img --tool cjxl=/absolute/path/cjxl config validate
img config path
img config init ./preferences.json
img --config /path/to/preferences.json config show --effective
img run /path/to/images --allow-database --quality-heuristic
img fast-img /path/to/images --jpeg-effort 11 --fallback-policy strict
img run /path/to/images --jxl-effort 8
img run /path/to/images --jxl-effort-search adaptive --jxl-effort 8 --quality-search fixed
img fast-img /path/to/images --jpeg-effort-search adaptive --jpeg-effort 11
img run /path/to/images --jpeg-effort-search full
img fast-img /path/to/images --jpeg-effort-search full
img fast-img /path/to/images --strategy avif --quality-search fixed --avif-quality 80 --avif-speed 6
img --avif-quality 10 --avif-speed 0 config show --effective
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
`config validate` checks the same effective policy. IMG's `config path` reports the
user/project/explicit paths and precedence without loading potentially invalid
files. `config init PATH` atomically creates default JSON at an explicit path;
an existing file is never replaced. Relative targets are resolved to an absolute
path before temporary-file creation; the parent must exist. Missing parents,
empty targets and path-resolution failures are reported without selecting a
fallback destination. These commands do not access Photos.

## Policy Fields

Tool paths use a shared IMG/VID parser and typed resolver: repeated `--tool
NAME=PATH` overrides only named keys in `tools.paths`, retaining inherited tools.
The last duplicate name wins and the effective source chain records each CLI layer.
The launcher forwards independent `--img-tool` and `--vid-tool` values to the matching
worker, including Fast IMG, and rejects explicit values outside their supported scope.
Tool names use lowercase ASCII letters, digits, hyphens or underscores; paths must
be absolute and NUL-free. Spaces and additional equals signs are preserved; non-UTF-8
paths cannot cross the string-based launcher boundary and are explicitly rejected.
Configuration inspection parses paths without reading/executing the selected tools.
Actual processing retains existing executable health checks and fidelity gates.

Native Developer Tool Paths keeps independent IMG/Fast IMG/VID preferences. Its
file chooser, reset and effective source preview edit the parent Settings draft;
cancelling discards them and Apply validates through the worker before persistence.
No override means inheritance, not a saved copy of a discovered or inherited value.
No raw JSON editor or configuration-file rewrite is involved.

Image settings adjust the current private policy, not a second processing policy.
The algorithm reference and route-specific limits are recorded in
[IMG Private Policy](IMG_PRIVATE_POLICY.md).
The native question-mark guide explains those routes and search decisions inside
the app. Its workflow context is the selected operation, not a promise that a
particular file has already passed a Gate. Actual configuration and per-file
evidence remain available through settings, run details and history.

- **D: pixel distance.** `img.jxl_distance` is nullable: omission or `null` keeps
  the existing source/mode decision. An explicit value must be finite in 0..25.
  It can adjust an eligible legacy-lossy pixel encode, not relax a protected
  source's fidelity contract. PNG and confirmed-lossless input keep D0; JPEG
  uses reversible reconstruction. Existing modern-lossy/container retention
  remains in force. These are private policy decisions, not broken-media errors.
- **E: encoder effort.** `img.jxl_effort` is nullable: Normal uses E7,
  Ultimate/archive E10, and PNG E10 unless explicitly overridden. JPEG uses its
  independent manual `img.jpeg_effort`, initially E11. Explicit supported efforts do not
  change D, media routing, metadata policy or verification.
- **D exploration.** `quality_search=adaptive` is the default and requests search
  only where source and size admission permit. Explicit `mode-default` retains the existing
  Ultimate/explore/non-archive trigger. `fixed` disables D search; `adaptive`
  requests it only where source and size admission permit. D0 is the baseline;
  ladder screening, finalist verification and refinement use the same selected
  E. The algorithm prefers the lowest fitting D, not the smallest file at any D.
  Explicit `adaptive` leaves a saved manual D inactive without overwriting it.
  `mode-default` retains the manual D where the historical route does not explore.
- **E exploration.** `jxl_effort_search=adaptive` is the default. It
  compares verified pure-media payloads at one quality within the bounded E
  candidate range. Equal payloads prefer lower E. This is not low-E assistance
  for high-E D search and is not a speed guarantee. With both searches enabled,
  E is selected first, then D is explored in that single E domain. A Normal E7
  policy ceiling may admit only E7; automatic does not invent a wider domain.
  Select `fixed` explicitly to use one manual effort.

Fast JXL has JPEG E comparison, not pixel D controls.
Ordinary IMG and Fast JXL share `jpeg_effort_search` with three choices:

- `fixed`: manual opt-out; use `jpeg_effort`, initially E11.
- `adaptive`: compare the selected E and the lower members of E1/E7/E8/E10/E11.
  This bounded comparison does not promise the minimum across all levels.
- `full` (default): independently encode E1 through E11, apply the same delivery metadata
  policy, then compare complete JXL file bytes, not image payload alone. Ignore
  the saved `jpeg_effort`; equal sizes retain the lower E. The first candidate and
  every smaller contender need exact JPEG reconstruction proof. Larger/tied
  candidates cannot beat that proven winner and do not need another decode for
  selection. Reuse the winning file; final metadata and delivery Gates still run.

Full comparison usually takes more work than a single E11 transcode. No
Effort equivalence or monotonic size assumption is used. Failure, cancellation,
unsupported E or changing source/sidecar/tool aborts the search without claiming
an all-level minimum. Full mode disables parameter-changing compatibility and
repair retries, even when permitted by `fallback_policy`; fixed/adaptive retain
their existing recovery rules. A failed full search preserves the original.
Explicit saved choices are preserved, not forcibly migrated. Resetting GUI
overrides inherits the current typed configuration; old files that explicitly
request `fixed` remain manual. Defaults are shared by CLI and direct engine calls.
Fast AVIF keeps its own Q search and locator/final Speed domains; JXL D/E does
not substitute for Q/Speed. Its metadata policy is separate from archival JXL.

- **Fast AVIF fixed Q.** `img.avif_quality` accepts integers 10..100 (default 100).
  The existing progressive-output policy requires Q at least 10; the encoder's
  general non-progressive 0..100 range is not this route's supported range.
  It is active only when `img.quality_search=fixed`; the selected Q is probed once
  in the final speed domain, not silently lowered on failure or oversize. With
  `mode-default` or `adaptive`, the stored Q is inactive and the existing bounded
  search starts at Q100. A true-lossless candidate may still be accepted first
  for a confirmed-lossless source; existing AVIF retains its no-reencode route.
- **Fast AVIF final Speed.** `img.avif_speed` accepts integers 0..10 (default 0,
  slowest; 10 is fastest). It applies to true-lossless probes, final-domain Q
  probes and refinement. The existing fast locator remains Speed6; its temporary
  candidate is discarded and its coordinate must be re-probed at the selected
  final Speed. This is a speed selection, not Speed exploration or a JXL E value.
  No runtime or compression benefit is guaranteed by changing it.
- **Fast AVIF metadata intent.** `img.avif_metadata_policy` explicitly records the
  existing private `clear` policy. It is the only supported value; `preserve`,
  unknown values and null are rejected. `--avif-metadata-policy clear` is scoped
  to Fast AVIF and configuration inspection. The resolved policy binds candidate
  preparation, publication, existing-AVIF custody, local Gate 1 and policy version
  3 in recovery evidence. An AVIF extension alone never authorizes clearing.
  The native Fast AVIF settings show this effective intent read-only, with
  separate loading, unavailable and unsupported states. Standard JXL retains its
  preservation policy. Removable metadata clearing does not waive pixel, color,
  HDR, filesystem-property or source/sidecar deletion proofs.

`--avif-quality` and `--avif-speed` are valid for `img fast-img --strategy avif`
and `img config` inspection, not ordinary IMG, Fast JXL or unrelated commands.
The launcher uses `--img-avif-quality` and `--img-avif-speed` in Fast AVIF mode.
Explicit Speed0 survives all boundaries. Q100 is the encoder's highest quality
setting, not a guarantee of RGB mathematical losslessness for arbitrary inputs.
All existing health, pixel, payload-size, metadata and delivery checks remain
mandatory; a fixed Q request can fail them and must preserve the source.

CLI flags and GUI preferences enter the same typed resolver. Existing
`--explore`, `--match-quality`, `--ultimate` and `--archive` interfaces are not
retired by this work. Explicit typed search preferences control their own
algorithm; they do not remove source admission, size or delivery checks.

### Native GUI Overrides

The gear button shows the current image workflow, a standard Video tab,
Photos settings and Performance settings. Standard and Fast IMG retain
independent JPEG transcoding effort (1 through 11), fallback,
quality heuristic and database overrides. Standard IMG preferences are never
copied into Fast IMG on read; each workflow uses its own saved overrides and the
backend's effective configuration. Existing independent preferences stay intact.
Fast AVIF adds **Manual quality** with a Q10..100 input and a Speed0..10 popup.
Automatic or inherited AVIF-rule selection disables the Q input without deleting
its saved value. Selecting manual quality reactivates it. Reset removes only the
GUI overrides; effective source loading and validation still use the same worker
resolver as execution. These controls are absent from standard IMG and Fast JXL.
The ordinary Image tab keeps D and E values separate from their independent
exploration checkboxes. E-only remains available without D exploration; D-only
does not silently search E. Inactive fixed-D drafts remain saved, while D search
uses the policy's D0 baseline. Fast JXL shows JPEG E, not pixel D; Fast AVIF shows
its separate quality selection. Each workflow retains independent preferences.
The ordinary labels use quality (D), compression level (E) and lossless JPEG
transcoding rather than algorithm names. When E comparison is enabled, the
numeric control is labelled "Compare up to level (E)" instead of implying a final
effort. Turning it off restores the fixed-level label without changing the
saved value or worker arguments. Unspecified D/E display "Follow archive rules"
only after configuration is known; a failed lookup remains unknown.
The D checkbox distinguishes the inherited conditional policy from an explicit
automatic choice, and accessible labels follow the same visible state.
An unchanged inherited control does not save an override;
explicit selection retains one, and reset removes it without changing JSON.
Search locates the original setting by localized label, section or choice. In
Developer mode, runtime keys and flags are searchable too. Results include only
the current workflow's applicable controls; selecting a Photos advanced parameter
reveals it without changing its value. Search and navigation preserve the draft.
Photos exposes backend selection, expandable native/AppleScript/verification batch sizes,
adaptive sizing with minimum/maximum/target duration, root folder, album name
and subfolder preservation. Configuration-file overrides and standard IMG/VID failure
policies appear in Developer mode, enabled in About. Fast IMG has its own failure
handling control in Image settings, independent of the standard-mode preference. Image tool selection
is an ordinary Image/Fast IMG processing preference.
Video exposes HEVC/AV1 in Settings, plus an independent developer configuration
file and file-error mode. Long-video fusion quality checks are an ordinary Video
preference. The latter is typed
`vid.force_ms_ssim_long`, default `false`; direct standard VID uses
`--force-ms-ssim-long[=true|false]`, and the launcher uses
`--vid-force-ms-ssim-long[=true|false]`. Omission inherits the file setting, explicit
false overrides it, and GUI reset removes the override. Enabling can increase
quality-validation time, without changing codec routing or reducing other checks.
AV1 defaults to non-Apple delivery; an explicitly
conflicting `--apple-compat` request is rejected. HEVC keeps Apple compatibility
by default; `--no-apple-compat` still disables it. Video quality and preset are not exposed by the current
CLI and therefore are not editable controls.
Video settings apply only to standard `vid run` processing. Fast Video uses
`vid fast-gif`, which does not use `vid.codec`, the standard error-mode override or
long-video quality overrides; explicit launcher overrides and direct long-video
quality flags there are rejected. Direct Fast VID
configuration files can set its shared performance policy, not its GIF/AVIF
strategy. The GUI disables the settings button for that operation.
Fast IMG accepts the same typed `execution.error_mode` through its own GUI override
and launcher `--img-error-mode`. The default is `log-and-continue` (record failures,
preserve sources and continue independent work). Fail-fast drains and checkpoints
the already-started encoding wave, then stops before scheduling the next wave.
It does not discard completed encodes or turn retained failures into skips.

Image, Video and Photos effective values are queried from the backend, not duplicated
in Swift. Ordinary settings display resolved values without an extra inherited choice;
opening and applying unchanged controls does not create overrides. Developer failure
policy menus instead show each pipeline's actual inherited value, with explicit
choices to override it or return to inheritance. Explicitly selecting the same value
still saves an override; inheritance and tab reset remove it. Failed effective queries
show an unknown inherited value, never a fabricated continue policy. When standard
IMG and VID inherit different performance modes, the mixed workflow offers
"Use each pipeline's configuration" rather than fabricating one shared mode;
only an explicit shared selection overrides both. Developer
tooltips include configuration keys and sources. Efforts use numeric menus;
other numeric fields use steppers;
quality inference/database use Enabled/Disabled menus and Photos booleans use
ordinary two-state checkboxes. Apply
validates the relevant standard/Fast IMG and VID profiles, including adaptive
batch bounds, before persisting. Configuration query failures stay visible.
Overrides are saved in native GUI preferences,
passed as explicit launcher options, and only forwarded to the matching media
pipeline. Reset This Tab removes that tab's overrides, not the other tab or
the runtime JSON. Invalid saved overrides and unreadable explicit files fail
visibly instead of silently reverting to defaults. JSON schema validation
remains owned by the shared Rust model. Tool executable paths remain available through JSON or
direct `img --tool` overrides.

The launcher accepts `--img-config`, `--img-fallback-policy`,
`--img-jpeg-effort`, `--img-jpeg-effort-search`, `--img-jxl-effort`, `--img-jxl-distance`,
`--img-jxl-effort-search`, `--img-quality-search`, `--img-quality-heuristic=true|false`,
`--img-allow-database=true|false`, `--img-tool-policy`, `--img-error-mode`, `--vid-config`, `--vid-codec`, and
`--vid-error-mode`, plus `--vid-force-ms-ssim-long=true|false`. Error modes are `log-and-continue` and `fail-fast`; overrides
are forwarded as typed worker `--error-mode` flags, including PTY launches, without
mutating the launcher's global environment. For standard runs the launcher resolves
the relevant IMG/VID configuration files before routing, so parent continuation
uses each pipeline's inherited or explicitly overridden policy. Encoding options are not sent to verification,
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

`execution.error_mode` accepts only `log-and-continue` or `fail-fast` in JSON and
CLI. Its environment precedence, from lower to higher, is
`MFB_DRAG_DROP_ERROR_MODE`, an enabled `MFB_DRAG_DROP_FAIL_FAST`, then `MFB_ERROR_MODE`.
A false legacy boolean does not cancel another mode. Supported legacy aliases remain
limited to environment parsing; invalid, empty or non-UTF-8 environment values stop
configuration loading rather than silently choosing a policy. Files and explicit CLI
flags retain higher priority. `config show --effective` includes the policy's winning
source and ordered source chain; new captured history snapshots include its winning
source. This is not a failure-count threshold. Fast IMG encoding waves and local
verification use this policy; Fast GIF retains its own encoding loop. Shared Photos
import error handling continues to use the resolved policy.

Gate 1 treats ExifTool's exact `Bad IFD1 directory` diagnostic as a rejected
file, after the response has been bound to the complete requested source/output
set. In `log-and-continue`, other assets can complete the remaining checks and
the existing isolation/reverification flow. The malformed EXIF is not repaired,
discarded, or accepted because JPEG reconstruction succeeds. Missing or duplicate
responses, unknown diagnostics, and tool/read failures still block shared
verification; explicit `fail-fast` still stops at the first failed check.

The launcher reads its local `local_env.json` or fallback `local_env.sh` once at
startup, before temporary-root initialization, typed configuration installation
and signal-handler threads. These values participate in the environment layer;
configuration files and explicit flags still override them. Changes to local-env
files require restarting the launcher, including interactive and watch sessions;
active batches do not repeatedly mutate the process-wide environment.

Launcher resource snapshots retain unavailable CPU, memory and disk measurements
as unknown. System-monitor initialization errors keep their cause; dashboard
probes report them without substituting zero or an estimated disk capacity.

### Captured Runtime Configuration

Launcher-managed media invocations emit their already resolved `RuntimeConfig`
with the winning `sources` map and worker package version. The launcher validates
the full snapshot and stores `MFB_HISTORY_CONFIG` in `history.sqlite3`; it does
not reload files or fill missing snapshot fields with defaults. Structured config
records are not duplicated into terminal output or the worker-text table. Write
or marker validation failure is explicit after the child output is drained.
Known media counts and the original child exit remain available even on history
failure; unsafe downstream steps stop, and the final summary is attempted without
replacing the original error. A bad marker does not truncate later worker output.

Processing History shows distinct captured snapshots within the selected batch.
Repeated identical snapshots share a display entry, but captured invocation count
is separate from processed-file count. Missing historical config is not inferred
from current preferences. Invalid/future records retain valid earlier snapshots
and media counts, with an explicit issue; they never hide a batch's failed status.
This is the typed runtime snapshot, not a complete run manifest: command-specific
flags, all losing overrides, tool versions, build IDs and the backend actually
chosen after runtime fallback are not yet part of this record.

### Native Log Visibility

The native live log is a bounded view, not a durable audit receipt. Ordinary text,
diagnostic details, and result/control/safety records have independent bounded
budgets while accepted records retain their arrival order. Ordinary warnings,
per-file failures and valid ignored/file-failure detail rows cannot exhaust the
result reserve; serious safety diagnostics and malformed structured detail rows
remain control records. Replaceable file-progress snapshots are still coalesced.

`MFB_GUI_TRANSPORT_GAP` reports omitted `diagnostic_records` and `control_records`
separately. These are display-record counts, not source-log line or failed-media
counts. Diagnostic omissions leave intact result counts usable. Missing control
records still invalidate completion evidence even when a later summary arrives;
the interface never manufactures an empty `MFB_BATCH_RESULT` to represent display
pressure. Invalid gap reports also fail closed. Neither case alters worker or
history counts. Missing count reports receive a scoped notice; independent delivery
facts remain visible without being substituted for the missing processing totals.

Oversized source lines may span multiple bounded display records. The 3,000-line view includes a notice when older content has been removed and retains the newest
2,999 lines. Clearing or trimming this view does not delete backend records.

These notices direct users to inspect **saved content** in Processing History;
they do not promise complete persistence. Backend write failures remain explicit,
and no successful-write receipt is inferred from UI activity. Startup diagnostics
truncated by the bounded readiness check have a separate notice: only the prefix
was retained, with no invented omitted-line count or batch-history claim.

After draining a launcher-managed invocation and flushing its output writer,
`MFB_HISTORY_OUTPUT_STORAGE` records that invocation's observed records,
transaction-committed records and committed UTF-8 text bytes. Bytes exclude row
metadata, database overhead and implicit line delimiters; these are captured
text records, not original process bytes or media counts. Structured config
markers are stored separately and are not included in these totals. A rollback
does not advance the totals; retrying a buffered transaction cannot double-count.

The receipt distinguishes storage failure from incomplete output collection.
Even committed received text is not evidence that collection completed or the
worker succeeded. History shows both states, counters and the original storage
diagnostic for the bound run/invocation. Missing old receipts remain unknown;
invalid, unbound or conflicting receipts are not reconstructed from current UI
activity. A child-printed receipt is ordinary text, never a parent audit event.
Neither receipt state changes independently established media counts or delivery
proof. Storage or receipt-write failure remains an explicit launcher error after
the child is drained, without discarding the child's reported counts or exit.

History's file-failure details and their complete-detail copy include the
associated worker launch and execution evidence. The recorded run, invocation
and pipeline must match exactly, with one unique launch record; missing,
conflicting or mismatched records remain explicitly unavailable. The details
show the requested program, observed executable path, exact arguments and
pre-launch binary identity, followed by independently recorded child PID and
exit or termination signal when available. Missing process observations do not
turn a launch request into an observed start or exit. Invalid process evidence
does not hide a separately valid launch record. This identifies the processing
worker; it does not prove which encoder or verification tools ran for that
individual file, its failure stage, or current source safety. Under display
privacy, program names, paths and arguments stay hidden because the same worker
can receive routine files alongside a failed file; the explicit failure details
and recorded numeric process outcome remain available.

History actions use visible text alongside native symbols. Output viewing,
cleanup inspection and diagnostic export are grouped separately from record
location, copying and removal; progress and status have their own line. Viewing
cleanup details does not delete files. Record removal retains its existing
selection checks, preview and confirmation. Under display privacy, the copy
button reads **Copy visible content**, including after a language change, and
raw export remains unavailable.

The native history list uses an unstriped background and a centered empty-state
message. An empty history, a search with no matches, and records that could not
be read have different guidance; a read error is never presented as a clean,
empty history. The action area is separated from the records with a native
divider and rounded system buttons. These presentation changes do not change
stored results or the conditions for enabling actions.

### Optional Display Privacy

The native **Hide routine file names and paths** preference is off by default.
It changes presentation and copied visible text only; it is not a runtime media
policy and does not redact existing SQLite rows, disk logs, manifests, terminal
output or delivery proofs. Enabling it retains structured counts and progress,
hides normal-item paths and unclassified free-form text, and keeps explicitly
identified problem details available for diagnosis. A filename containing words
such as `error` or `failed` does not itself identify a problem.

Main-window and History rendering use the same preference, including tooltips,
configuration/argument details and visible-text copying. Raw log/JSON export and
external-log actions are unavailable while this display mode is enabled, because
those records have not been redacted. Turn the preference off to inspect them.
Unknown diagnostic continuations can remain hidden; this mode does not promise
that every arbitrary log line can be safely classified or anonymized.

### Native Photos Import Attribution

Photos/macOS may retain the importing application's identity on imported media.
The import setting and in-app guide explain that this may be unwanted and may
persist after import. Selecting another backend is not a guarantee that no
attribution will be recorded. The UI does not promise to erase existing source
attribution.

Packaged native helpers inherit the parent application's display name and icon
when built. The main and helper bundle identifiers and executable identities
remain unchanged. Updating future helper branding does not retroactively rename
attribution already stored by Photos, and the displayed name/icon ultimately
remain subject to macOS behavior.

### Runtime JSON

| Field | Default | Meaning |
| --- | --- | --- |
| `execution.error_mode` | `log-and-continue` | IMG/VID recoverable failure policy, including Fast IMG encoding waves and independently isolatable local verification failures. `fail-fast` stops after draining current work; shared safety failures always block and intentional skips remain skips. `--error-mode` overrides files. Fast GIF keeps its own encoding loop. |
| `img.allow_database` | `false` | Permits the optional image-quality database stack. Quality inference must also be enabled. Does not suppress Photos custody verification or explicit database maintenance commands. |
| `img.quality_heuristic` | `false` | Enables existing optional quality inference. A heuristic does not replace delivery proof. |
| `img.jpeg_effort` | `11` | JPEG bitstream transcode effort, 1 through 11; used by fixed/adaptive, inactive in full comparison. Independent of direct pixel encoding. |
| `img.jxl_distance` | `null` | Optional finite 0..25 pixel-distance override for policy-eligible sources; null retains the existing source/mode decision. Does not permit lossy archival of protected input. |
| `img.jxl_effort` | `null` | Optional pixel E override; null keeps Normal E7, Ultimate/archive E10 or PNG E10. Fixed encoding uses the value; opt-in E comparison can select a different verified candidate in its bounded range. |
| `img.jxl_effort_search` | `adaptive` | Verified E payload comparison at fixed quality, independent of D search. `fixed` is the manual opt-out. No cross-E auxiliary D screening. |
| `img.jpeg_effort_search` | `full` | `fixed`: one selected E; `adaptive`: bounded comparison; `full`: independent E1..E11 comparison by complete metadata-enriched JXL size. Every potential winner needs exact reconstruction proof; no partial minimum on failure. |
| `img.quality_search` | `adaptive` | Guarded standard D search or separate Fast AVIF Q search. `fixed` is the manual opt-out; explicit `mode-default` retains the historical trigger. Source/size admission remains mandatory. Not source-quality inference. |
| `img.avif_metadata_policy` | `clear` | Explicit private Fast AVIF removable-metadata policy; currently the only supported value. Does not change standard JXL preservation or authorize source/sidecar deletion. |
| `img.fallback_policy` | `strict` | `strict`: no compatibility/repair retries for a requested encode. `same-semantics`: allows compatibility retries and e11 to e10 on original JPEG bytes. `repair`: also permits the existing guarded repair paths. Full JPEG comparison never changes candidate parameters or repairs input. All delivery and exact reconstruction checks remain mandatory. |
| `vid.codec` | `hevc` | Standard video-run output codec, `hevc` or `av1`. VID `--codec` overrides the file and marks its source CLI. Does not select the Fast GIF strategy. |
| `tools.policy` | `fallback` | `single` forbids governed alternate image encoding/recovery tools. `fallback` permits alternatives only with the route's other required permissions; it does not itself authorize repair. Required detection, decoding, metadata and verification are unaffected. |
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

### Recovery Operations Versus Encoding Tools

The GUI calls `img.fallback_policy` **Encoding retries and repair**, and
`tools.policy` **Permitted encoding tools**. CLI names and saved values are
unchanged. These permissions do not select batch continuation or prove that an
operation ran, a tool is available, or a file passed verification.

- JPEG compatibility retries use `same-semantics` or `repair`; a supported E11 to
  E10 retry still requires exact reconstruction from the original JPEG bytes.
- The supported ICC D50 repair requires `repair` and its specific error trigger.
  It patches a temporary profile and retries the same `cjxl`, so `single` does
  not by itself forbid that operation.
- A D-search probe's alternate ImageMagick encoder requires tool permission and
  expert opt-in. `img.fallback_policy` is not a universal switch over all internal
  retries or alternate probes. External pixel recovery has additional repair
  permission requirements.
- Full E1..E11 JPEG comparison does not change parameters or repair input to fill
  a failed level. Recovery permission never relaxes that boundary or mandatory
  original-byte reconstruction and delivery checks.

The native guide explains these cases directly from Settings. Per-file executed
tool identities and recovery outcomes remain distinct from the saved permission.

### Inspect A Tool Without Processing Media

```sh
img config tool cjxl
img config tool cjxl --tool cjxl=/absolute/path/to/cjxl
vid config tool ffmpeg
```

This opt-in diagnostic loads the same effective configuration as the worker,
installs it only in the short-lived inspection process, and uses the shared tool
selection order. It executes bounded version commands, not a media conversion.
The GUI calls it using its immutable current draft, independently for IMG, Fast
IMG or VID; opening or closing the report does not apply the draft.

The schema-1 JSON reports `name`, `path`, `configured_path`,
`configuration_source`, `status`, `version` and `diagnostic`. A successful version
command with nonempty output is `version_reported`. A resolved tool whose version
cannot be established is `version_unconfirmed`; an unresolved selection is
`unresolved`. Missing information remains null, never an invented version.
Commands use the tool-appropriate version argument, accept version text from
stdout or stderr, and have a three-second deadline per probe. On Unix, each
diagnostic runs in an isolated process group; ordinary background descendants in
that group are stopped before output is drained, including when the parent exits
first. This is not a sandbox for programs that deliberately detach from the group.
Platforms without this isolation report unsupported inspection rather than run an
unbounded version command. Diagnostics and version lines are bounded; normal
processing's resolver/cache behavior is unchanged.

This is a fresh path/version observation, not an inventory of every route
dependency, a codec capability test, proof of an actual per-file invocation, or
permission to bypass tool/recovery policy. The common-tool GUI view does not claim
that every listed tool is needed for the selected file. Inspect again after a
configuration or environment change; execution still resolves and verifies its
own tools and artifacts.

### Photos Import And Verification Windows

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

## Cancellation And Result Evidence

Cancellation describes process control, not a reversal of completed work or an
explanation for an existing integrity failure. The native result overview and
History keep known verification conflicts above the cancelled status. Later
matching counts cannot clear those conflicts or change recorded media counts.
An ordinary cancellation without such evidence remains a neutral stopped state;
missing completion receipts after cancellation alone do not invent an integrity
failure. Original diagnostics and the cancelled process outcome remain available.

## Verification And Scheduling (2026-10-01)

Apple Silicon memory sampling reads the actual `vm_stat` page size rather than
assuming 4096 bytes. Admission workers share a sample for at most 250 ms,
including failed samples; expired samples are re-probed. This corrects a 4x
underestimate on 16 KiB systems without removing memory-pressure limits.

Gate 1 performs cheap count/size/hash checks before expensive proofs and records
each executed check's duration. In record mode, explicitly file-local metadata,
or orientation failures do not prevent the remaining required checks.
The initial failed Gate never passes: exact, unique source/output mappings and
unchanged hashes must authorize recording each rejected asset as failed and removing
only its owned output. The remaining subset then runs all six checks again before
delivery; no source is removed by isolation. Fail-fast and shared tool, protocol,
inventory, identity, persistence or cancellation failures still block delivery.
Missing or ambiguous scope is shared, never inferred from a nonempty path list.
Local progress reports these recoverable checks as `file_failures`, not a passed
Gate or a shared terminal failure. The human summary uses `FILE_FAILURES` only
after all six checks complete with identified file-local faults. GUI confirmation
still requires the final delivery/retention receipt and successful survivor
verification; shared `failed` events remain sticky even after a later PASS.
Gate 2/3 shared custody and asset-relationship failures remain blocking; record mode
does not waive them or turn unchecked assets into successful deliveries.
An external decoder's nonzero exit alone does not prove a file-local defect: resource,
storage and tool failures can share that exit code, so unclassified decoder errors
remain blocking until a reliable file-specific cause is available.
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
`--fallback-policy` takes precedence. The default remains strict. Full JPEG
comparison never silently changes a requested effort, even if recovery is enabled;
fixed/adaptive modes retain their explicitly permitted compatibility rules. No setting disables exact
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

### Deferred Photos Delivery

`photos_delivery_deferred` is a typed target-preparation outcome, not a verdict
inferred from an error message. It applies after primary verification when a
temporary native witness/activation failure leaves pending delivery before
primary source cleanup. Permission errors, invalid target evidence,
cancellation, unrelated finalization errors and
unreconciled native journals do not qualify. The worker and launcher still exit
nonzero; encoded and pending counts are not promoted to delivered counts.

The result also records `primary_verification_complete`, `source_directory` and
`output_directory`. The launcher persists the current result as
`MFB_HISTORY_FAST_IMG_RESULT`, bound to its run ID, only after reconciling counts
against captured worker statistics. A malformed, duplicate or conflicting result
does not authorize post-run cleanup. History and the native result overview use
the same receipt and retain the original error and exit details.

The waiting-delivery presentation currently covers one selected directory and
requires matching source/output scope, complete counts, expected normal nonzero
exits and no independent integrity or
failed-source verification fault. Missing or contradictory evidence remains
unconfirmed. The recorded output location is not proof of current contents or
permission to delete it. Resuming still requires ordinary checkpoint, content and
Photos reconciliation; this status adds neither an automatic retry nor a claim
that all pending outputs are reusable.

### Photos Operation Progress

Fast IMG emits `MFB_PHOTOS_PROGRESS` for native imports and checkpointed
AppleScript imports of primary outputs and tier-2 originals, using the existing
serialized terminal-marker channel. Schema 1 records a local `operation_id`,
increasing `sequence`, the `current_pending_import` scope, phase, fixed
`pending_total` and `verified` count. The phases distinguish target
preparation, old-intent reconciliation, import submission and local verification.

The inventory contains only this operation's pending source identities. Already
proven checkpoint assets are excluded. Reconciled old intents count only when
they belong to that pending inventory and their newly verified checkpoint has
been saved. A repeated or unknown identity cannot increase progress, and a
checkpoint write failure does not advance the verified count. Prefetched import
may overlap verification; the visible phase describes the current verification
work, not an assertion that no other request is in flight.

AppleScript resume reconciliation uses the initial pending inventory even when
it rebuilds the import plan. Newly proven assets in that inventory advance
progress; older proven assets do not. Group and individual verification results
advance only after checkpoint persistence, including successful items preceding
a later failure. Same-run recovered assets are already checkpointed before they
advance progress.

Checkpointed original-media imports share the native original-media plan. Prior
receipts must pass the existing live custody recheck and match the candidate's
delivery hash before they are excluded from pending work. Each newly verified
AppleScript original is saved separately, before the next item is submitted.
XMP-enriched deliveries retain distinct source, delivery and sidecar hashes in
the saved receipt. Both backends publish an updated in-memory tier-2 marker only
after its atomic write succeeds. A later item failure therefore does not erase
earlier durable receipts; the next attempt plans only the remaining candidates.
The progress scope starts after metadata staging: preparation failures remain
separate batch failures, not verified imports or part of its denominator.

`completed` requires the pending inventory to be fully checkpointed and the
existing asset-count checks to pass; the AppleScript path also requires zero
deferred item failures. An unfinished operation emits `stopped` when its scope
exits; that includes cancellation, failure and a permitted
pre-import handoff to the AppleScript backend. The latter opens a separate
operation without inheriting the native operation's counts. Neither terminal
event determines the whole run's outcome. The native GUI releases a closed
operation's progress row to subsequent activity. Without a positive denominator
it shows activity, not a percentage; an active operation is never rounded up to
100 percent.

Malformed, replayed, out-of-order or conflicting progress becomes unavailable,
not a fabricated completion or a change to authoritative batch results. Per-process
counter high-water marks reject closed-operation replay without the former
64-operation display limit. Same-batch PID reuse with a reset counter is rejected
conservatively; a new batch resets this local presentation state. These operation
IDs are presentation scope, not persistent file-task identities. The
events travel through the existing captured worker-output path; they do not
create a second state store, authorize deletion, prove later Gates or iCloud
completion. Generic candidate imports without a checkpoint do not emit this
durable progress. Local Gate 1 file-check activity is documented separately below. Synthetic
regression and GUI fixtures do not replace authorized live-library delivery
acceptance.

### Photos Original Custody Reverification Activity

The existing `MFB_GATE_ACTIVITY` schema 1 also accepts `gate=photos_custody`.
It reports the real tier-2 Photos-original custody recheck used before cleanup
and while validating existing receipts for resume or library binding. This is
not Gate 2's sampled receipt comparison or Gate 3's record consistency checks.

`inventory` has no file denominator. Once nonempty relative paths and Photos
UUIDs are present and unique, `original_bytes` uses that fixed receipt inventory.
The existing query resolves those UUIDs; each current original is read in full
and BLAKE3-compared with its recorded library-delivery hash, or source hash when
no separate delivery hash exists. The numerator advances only after a match.
Intermediate updates are coalesced at 250 ms; terminal events retain the exact
matched count, including the matches preceding a later failure.

An ambiguous inventory fails without a denominator. Query, missing-original,
read or hash errors remain failures; they do not count as matched files. Empty
inventories do not query Photos or display a percentage. Within one stage, the
GUI rejects changes to gate scope, check count or an established file denominator,
decreasing matched counts and a partial `passed` event. An active row cannot
round up to 100 percent, and later activity cannot erase a recorded safety risk.

This progress is a current local byte-custody observation, not a persisted
checkpoint, whole-batch completion, deletion authorization, iCloud proof or a
replacement for source, metadata and sidecar deletion gates. It reuses the
captured worker-output channel and adds no state store or extra media query.

### Local Gate 1 File-check Activity

The existing `MFB_GATE_ACTIVITY` schema 1 reports file-check attempts within the
local verification gate. Hash and metadata checks count the current recorded
source/output mapping pairs. Orientation and decode checks count the actual
output files being checked. These are stage-local inventories, not the batch's
discovered, successfully processed or Photos-verified media counts.

Hash and metadata progress requires a nonempty mapping inventory matching the
expected count. Orientation progress starts after its existing shared metadata
probe succeeds. An unavailable or inconsistent inventory has no invented file
denominator. Each completed attempt advances the numerator, including attempts
that fail; the existing check result, affected files and failure scope still
decide whether the gate passes or further work is blocked. A complete attempt
count does not prove a successful check.

Updates reuse the existing 250 ms coalescing and captured worker-output channel.
There is no additional media scan, hash, metadata query or separate state store.
Within a check, counts cannot decrease or change denominator. Starting another
check releases the preceding check's file-count scope. Malformed or conflicting
events make progress unavailable without changing verification results.

The native GUI and History retain the stage name and real counts. A complete
file count or a completed local gate does not establish whole-batch completion,
Photos custody, deletion authorization, a persistent checkpoint or iCloud sync.
Photos original-custody progress above has a different numerator: it counts
successful byte matches rather than completed check attempts.

## Launcher Preflight Content Identity

The read-only routing report uses schema 3. Every visible inventory entry,
including sidecars and retained non-media files, carries `content_blake3`: the
lowercase BLAKE3 digest of the complete bytes observed during that scan.
The native preflight inspector validates the field and displays it in file
details. Older or incomplete reports are rejected rather than assigned a guessed
identity.

Discovery checks the opened file's metadata before and after the read, the byte
count, and the logical path's resolved target. Routing/publication reuse that
observation with inexpensive metadata freshness checks. Standard routed IMG/VID
worker launches re-read their selected sources and compare the complete digest
before spawning; a changed, missing or unreadable source refuses that invocation.
Inspection does not write source media or create output directories.

This adds one full input read during discovery and one before each selected
source is launched. It is not a file lock, an immutable snapshot or a cross-process
execution manifest: a later change still requires worker-side admission and
final-artifact checks. Fast IMG's internal scan/resume and verified sidecar
relationships remain separate contracts. A matching source digest never
authorizes conversion, import or deletion on its own.

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

## Remove Selected Processing History

The native history window supports removing one or multiple completed database
runs. It first previews the selected run identities, captured-output rows and
owned logs. Confirmation sends the same selection and snapshot token to the
shared `cache_cleaner` backend; stale selections, active writers, ambiguous
identities and unsupported schemas are rejected before removal.

The backend deletes selected event/output rows in a transaction. External files
are eligible only when their registered ownership, current file identity and
complete content hash agree. Shared session logs and unregistered worker or
legacy logs are explicitly retained, not guessed from a filename or timestamp.
Media, output folders, cache and resume/checkpoint state are outside this action.
An unexpected owned-log failure produces a partial receipt with its retained or
recovery path; expected retention alone does not make the operation fail.

An identity-only `history_removed_sessions_v1` table prevents retained legacy
JSONL files from recreating removed sessions in the window. It contains no media
names or paths, and a retired identity cannot be reused by a writer. The database
file remains available to other runs; removal is not a secure-erasure operation
or an instruction to delete its live WAL/SHM files.

Every history writer, including selected-record removal, retains the WAL/SHM
access files when it closes. The native history window keeps its actual queries
read-only. If system SQLite specifically cannot open a closed history database,
the window first asks the backend to prepare read access, then verifies the
returned database identity and supported schema before retrying the read.
Other errors are reported without rebuilding or clearing history.

For an existing affected database, `cache_cleaner --history-prepare-readonly
--history-directory <existing-log-directory> --json` performs that same bounded
preparation. It opens only an existing, privately owned regular database, sets
query-only mode, validates its supported schema, retains SQLite's access files
and proves a cold read. It does not create a database, migrate the schema, delete
rows, or treat unreadable history as an empty history.

Controlled errors attempt no-clobber log restoration and report any recovery
path. A hard crash between renaming logs to `.pending` files and committing the
database can leave those logs in place without an automatic recovery journal.
Their bytes are retained; this action does not claim atomic crash recovery
across SQLite and the filesystem. That broader lifecycle work remains open.

Automation uses `--history-directory`, `--history-cleanup-preview` and
`--history-cleanup` with schema-1 JSON containing authoritative `session_id` and
`run_id` pairs. Execution additionally requires the exact preview
`snapshot_token` and `--yes`; `--json` returns the verified preview or receipt.

## Shared Application Session

`dev::app::session::AppSession` owns the session history lease, diagnostic scope,
artifact registration, per-attempt run identity, tool bindings, result/summary
recording and archive lifecycle. The launcher adapts CLI inputs to a typed `RunContext`
and delegates history execution to this shared layer; worker startup, capture,
drain and process-exit handling remain in `dev::app::worker`.

Context persistence and the attempt output cursor are established before work.
Context failure prevents worker execution. A terminal history-write failure is
reported without replacing an existing processing error or inventing counts.
Input scopes share the same lease and run identity; distinct attempts and
independent sessions retain their own identities. This is session-level
orchestration, not a complete persistent per-file task or delivery manifest.

## Fast IMG Processing Identity

New Fast IMG checkpoints bind a schema-1 typed `processing_contract` before the
first write. It contains route-specific effective JPEG E/search/archive/expert
or AVIF Q/Speed/search/metadata/expert settings, plus tool and recovery
permissions. Inactive manual E during full JPEG comparison and inactive Q during
automatic AVIF search are normalized to their actual execution semantics;
unrelated standard IMG, video, cache and interface settings are excluded.

Saved and requested contracts must agree before tool preparation, and again
under the output lock before the recovery choice, recursive inventory or output
preparation. `MFB_RESUME_CONFIG_CHANGED` reports a mismatch;
`MFB_RESUME_CONFIG_UNKNOWN` reports missing historical intent. Neither assigns
today's settings to old results or overwrites their recovery evidence. Restore
the recorded settings to continue, or explicitly start an isolated new task.
Legacy inspection without this intent does not migrate the marker as a side
effect. Unknown contract schemas, fields, strategies and invalid values fail
closed on read and write.

This contract complements the existing source, tool-runtime, metadata-version
and Photos-target checks. It does not establish complete sidecar/filesystem
identity or a complete persistent per-file plan. Failed-only child attempts add
their own admission evidence, as described below.

## Photos Submission References

Checkpointed Fast IMG imports persist schema-1 `photos_submission_history`
before sending media. Each entry binds the source and output relative paths and
full BLAKE3 identities, encoded/original route, album and selected library. Native
entries also reference the exact journal path and complete request BLAKE3;
AppleScript entries retain a task-scoped request identity. XMP-enriched original
imports distinguish the original source bytes from the enriched delivery bytes.

Both native import and prefetch, plus checkpointed AppleScript encoded/original
submission, persist before their actual send. AppleScript reserves requests in
the existing bounded groups, then consumes one matching in-memory permit per
initial send. An unused reservation may remain when a query fails, a prior asset
is recovered or the run stops. A retry needs a new persisted request reference.
A reservation write failure prevents the group's sends in either error mode and
does not advance the caller's in-memory checkpoint. The reference describes a
possible side effect, not a verified asset, a successful send or a commit.
Native journals and fresh UUID/original-resource verification
remain authoritative. Submission records never grant deletion permission or
increment media success counts.

New checkpoints record complete coverage of their own checkpointed media
submissions from creation; this is not a global Photos deduplication claim.
Missing historical coverage remains unknown; appending new requests to an old checkpoint does not
turn earlier unknown activity into an empty complete history. Invalid schemas,
paths, hashes, duplicate entries and conflicting request scopes are rejected.
Failure isolation and outcome resets retain these references after local outputs
are removed. They share the Core SQLite task transaction; no second status
database or per-file request sidecars are introduced. Grouping reserves bounded
initial AppleScript requests, and existing verified receipt persistence is
retained. SQL persists appended references and changed receipt rows rather than
rewriting the full collection as a JSON checkpoint. Complete DTO validation and
snapshot hashing still scan the logical state; this is not a measured throughput
claim.

AppleScript reconciliation reports `MFB_NOT_FOUND` only after a successful query
returns no match. Query failures and ambiguous folder/album or single-asset
matches propagate instead of authorizing a new import. Discovery still does not
replace original-resource identity verification. Failed-only children consume
these references before any new submission. A complete source/sidecar execution
manifest and general recovery of every interrupted stage remain separate work.

## Failed-source Child Attempts

Fast IMG distinguishes continuing an unfinished task (`--retry`), starting a new
isolated task (`--no-resume`), and retrying retained failures (`--retry-failed`).
The latter applies to a completed checkpoint with recorded failures and complete
source dispositions. It creates a child attempt without resetting the completed
parent's outcomes, gates or receipts. Successful parent media are outside the
child's work set, including when their sources and local outputs have already
been safely removed.

The immutable Core relation binds the exact parent snapshot, processing
contract, requested local/Photos delivery and admitted failed sources. Each
admission retains the parent's source hash separately from the newly observed
source hash and native identity, with explicit XMP/AAE presence or absence.
Changed source bytes are a new admission, not an assertion that old evidence
still matches. Unreadable, ambiguous or linked resources cannot be admitted by
guessing. Current admission requires the entire selected failure set to be
observable; per-file blocked admission is not implemented.

One parent snapshot reserves one stable child location. Before creating that
reservation, the processing entry holds the prospective output lease and refuses
any existing filesystem object at that location, including an empty directory.
Source input must name the recorded source directory or its sole failed file;
selecting one file from a multi-failure parent cannot silently widen the request.
New children use the current verified tool authority; resumed children retain
their own authority. Changed processing or delivery intent blocks reuse.
The default Photos library can bind to its actual selected identity once; it
cannot subsequently change or be cleared.

The child processes only its persisted admission list. Independent verification
uses that same source scope and still checks the entire child output directory;
unknown output files do not disappear from verification. Parent and sibling
outputs are not swept by the child's post-processing cleanup.

A manifest-bound unfinished child can use finish-only recovery when its entire
recorded source root is explicitly observed as absent. Both `--retry-failed` and
`--retry` with the exact selected child take this path before source scanning,
source-directory locks and ordinary tool preparation. Without an explicit output,
ordinary continue accepts only one verified child for the exact saved source
root; multiple children require explicit selection. A missing single file needs
an explicit saved locator and is never assigned to a task by guessing its parent
directory. Recovery does not recreate the
source, encode candidates, submit media or delete media. A primary
`CleanupComplete` stage with original-format delivery still pending is not a
terminal task and cannot return a completed-child reference.

Finish-only recovery requires complete successful admission coverage and the
applicable historical Gate boundary. It rechecks the complete current artifact
bytes, exact recorded output inventory, companion custody and, for Photos, the
bound library's unique UUID mapping and full original-resource bytes. Missing
local output can be accounted for only by those same exact Photos artifact bytes;
pixel similarity is not a substitute. Missing AAE custody, unknown output items,
links, failed queries, changed identities and missing Gate evidence block finish.
Historical Gate records remain historical: missing sources do not authorize
inventing a new source comparison or a source-deletion receipt.

Core finalization accepts only the verifier's sealed result, compares the exact
pre-verification snapshot and revision in one transaction, and validates complete
readback before committing. Ordinary writers cannot fabricate, change or clear
the finish receipt. The stored receipt binds the original snapshot, manifest,
request and actual reverified artifact/Photos counts. It records source absence
as an observation, not proof of who removed the source or restoration of
pre-interruption directory metadata.

Before importing, reconciliation examines all ancestor submission histories and
the resumed child's own history. Missing historical coverage, uncertain native
commits, conflicting targets, missing identifiers, ambiguous candidates and
query/export errors prevent another send. Native recovery validates the full
original journal request and unique identifier mapping before projecting only
the child's selected sources. Fresh original-resource hashes and source/output/
companion checks are still required before recording a child receipt. A
successful filename lookup with no result cannot dismiss a possible prior send.
Diagnostic wording never determines whether a query error is safe to ignore.

Requesting the same completed child returns a historical reference through
`MFB_FAST_IMG_RETRY_REUSED`, confirmed by the launcher against this invocation
and durable Core relation. It performs no new media scan, encoding, Photos query,
import or media cleanup; newly processed, delivered and saved-byte counts are
zero. Missing old media directories are not recreated. This reference is not a
fresh gate or Photos custody proof. To retry a completed child's remaining
failures, the user must explicitly select that child as a new parent through
History or `--retry-failed-parent`; the ordinary action never automatically
creates grandchildren.

Fresh finish-only recovery instead emits `MFB_FAST_IMG_RETRY_FINALIZED`. Before
starting the worker, the launcher records the exact selected input, parent/child
relation and nonterminal child snapshot. It confirms the returned evidence
against that snapshot and the durable Core receipt before emitting
`MFB_FAST_IMG_RETRY_FINALIZED_CONFIRMED` and the corresponding History event.
Duplicate, mixed, stale or mismatched outcomes are rejected. GUI and History
distinguish this current reverification from historical reuse: newly processed,
delivered and saved-byte totals are zero, while actual reverified converted
artifacts and Photos originals are reported separately. An absent or unconfirmed
receipt cannot be promoted from log wording into success. Live confirmation also
requires the unique IMG worker's complete normal exit, not only a launch record
or the outer launcher's exit code. History requires both start and end output
boundaries for a confirmed recovery outcome.

The native recovery dialog offers only actions supported by the backend's
validated availability record. History links the current request with its parent
and child without presenting historical successes as new work. Debug windows
retain their read-only restrictions. These contracts do not establish real
Photos or iCloud acceptance, a complete execution manifest, or cross-filesystem/
SQLite crash atomicity.

This closes the manifest-bound converted-artifact finish path, not every missing
source case. Older ordinary tasks without durable admissions remain blocked.
The ordinary failed-source admission/count model does not yet provide a complete
original-format-only child execution path; injected Photos verifier fixtures do
not establish that production route or real Photos acceptance.

## Canonical Fast IMG State

Fast IMG task headers, source outcomes, Photos request/entry references and
verified receipt records use the existing Core SQLite store, schema 5. Large
per-file collections are normalized rows, not a second authoritative JSON blob.
One IMMEDIATE transaction publishes a logical snapshot; failed writes and commits
preserve the last committed snapshot. The writer validates the saved state first,
updates only changed rows, and verifies exact readback before commit. Identical
logical snapshots make no row changes. Submission history is append-only, and
missing historical coverage cannot be promoted to complete coverage.

Canonical source/output paths bind the header, indexed task locator and complete
snapshot digest. The same output cannot silently become a different task or
strategy. Its reservation survives local directory cleanup, so a new task cannot
overwrite older recovery evidence merely because the output no longer exists.
Ordinary reads, discovery and dry-run do not create a database or migrate state.
Unknown schema, missing previously bound storage, identity changes, malformed
rows and hash drift are errors, not an empty history or permission to reset it.

Legacy JSON is handled only by the named `migrate_legacy_for_source` operation at
actual processing admission, before output discovery and tool preparation.
Read-only discovery can inspect bounded old-format locators solely to report
`MFB_STATE_MIGRATION_REQUIRED`; it does not consume JSON as recovery proof. A
known-format migration validates ownership and all marker fields, then commits
normalized state together with the original bytes, BLAKE3 and migration receipt.
Only verified committed state permits retirement of the unchanged legacy file.
Unknown, conflicting or changed legacy files remain protected. Interrupted
retirement is resumable from its receipt; old configuration or submission gaps
are never filled with current defaults.

Legacy discovery matches the selected source or output without requiring every
unrelated historical media tree to remain present. A normalized, unselected
locator with an ordinary absent ancestor remains untouched and supplies no
recovery evidence for this task. Existing aliases still undergo canonical
comparison; dangling links, changed ancestors, access errors and invalid records
remain errors. A matching legacy task still requires explicit migration.

Before retiring each legacy marker, a separate transaction saves the original
directory observations and their BLAKE3, then verifies committed readback before
creating a holding directory or moving/removing the marker. Recovery reuses
those original observations; it does not recapture post-interruption dates as
the baseline. Native directory/entry identity and unrelated entry changes remain
checked before restoring access/modification times. The restored directory is
synced before confirming retirement. Multiple markers in one parent retire
sequentially, with pending recorded recovery completed before a fresh baseline.
This retains the existing cleanup comparison: known removals can change directory
times. The snapshot cannot distinguish an unrelated timestamp-only directory edit
from those changes and does not establish exclusive access against other programs.

An older interrupted retirement with a missing origin or an existing holding
path, but no original directory snapshot, remains explicitly unresolved. An intact
origin without a holding path can acquire a baseline before this retirement;
that cannot establish directory dates before earlier versions' actions.
Historical completed rows keep their missing coverage; schema migration does
not invent past observations.
A reappeared retired origin is a conflict, not permission to apply old dates to
a new directory generation. These snapshots authorize neither media deletion
nor Photos delivery, and do not establish complete crash atomicity across the
rest of the processing pipeline.

Core schema migrations from CRC32 v1 and BLAKE3 v2/v3/v4 to v5 are transactional.
Known v2/v3/v4 databases remain inspectable read-only without pretending they already
have the newer tables, directory evidence or retry relations. Integrity inspection
verifies all eight Fast IMG tables and migration backups in a consistent SQL snapshot. Durable
directory evidence is parsed, hashed and scope-checked without revisiting media
paths. It does not open media files or
confer Photos custody or deletion permission. Cache management counts these as
protected State and includes them in verified backups; cache pruning never
deletes them. Reported payload bytes cover actual stored headers, legacy marker
bytes, directory snapshots and retry admission/request payloads, not database pages, indexes or estimated
disk recovery.

The formal Rust Core root is fixed for the process; invalid or unavailable
configuration cannot redirect durable state to temporary storage. The store
selection follows startup environment loading: the bootstrap configuration
location is read before the resolved state root is bound, and cannot be used
to rebind an already active state session. The store
still lives at `cache/mfb_store.sqlite` and also contains protected legacy
Checkpoint/processed records. This change does not physically separate Cache and
State, implement a complete file-task Manifest or prove cross-filesystem crash
atomicity. Failed-only child attempts provide the narrower recovery contract above.

## Standard Worker Preflight Admission

Standard launcher runs submit one selected media file per IMG/VID invocation in
both record-and-continue and fail-fast modes. They do not send the source
directory to a worker for a second recursive media scan. Files added after the
inventory are therefore outside those invocations. An empty selected inventory
does not fall back to an unplanned directory run.

The launcher passes the original source observation as a bounded, versioned
internal `--planned-source` claim. Before configuration, ghost-mode, logging or
database initialization, each worker checks the actual input path, resolved
target, native file identity and complete BLAKE3. A changed, missing, retargeted
or unreadable source refuses that invocation. A claim on a directory or a command
other than standard `run` is rejected. The claim is invocation evidence, not a
persistent preference or an authorization to convert, import or delete media.

This adds a full source read at worker admission, in addition to discovery and
the launcher's pre-spawn check. No intermediate media or manifest file is written
for transport. Fail-fast still stops subsequent invocations after failure, but
uses the same per-file launch boundary as record mode; it no longer obtains
directory-worker batching throughput. No general throughput improvement is claimed.

The check is not a file lock or immutable snapshot. Inputs can still change after
admission, so actual conversion, associated metadata, final-artifact and deletion
checks remain required. This does not implement the Fast IMG inventory handoff,
verified sidecar relationships, a persistent execution plan, or failed-only retry.

## Background Native Validation

### Main and debug windows

Ordinary macOS launches expose one main window. A repeated ordinary launch
returns to that window; there is no debug-window menu item or normal shortcut.
If macOS delivers an ordinary reopen to a debug instance, it routes the request
back to the normal window role instead of presenting the debug window.
Development tools may explicitly start a separate native debug instance with
`open -n "/path/to/Modern Format Boost.app" --args --debug-window`.
There can be one normal main window and one debug main window;
Settings, History and other child panels do not consume another main-window
slot. Starting the same role again brings its existing window forward. Role
leases are kernel locks in the real user's state directory, independent of
`MFB_HOME_ROOT`; exiting or crashing releases them without deleting lock files.

The debug window is labelled explicitly and uses separate MFB preferences and
a private temporary state/log directory. It supports interface inspection,
log viewing and the existing read-only standard input preflight. Its process
and terminal entrypoints reject media processing and Photos import, and it
cannot remove history or run cache cleanup/backups. Configuration can still be
inspected from existing sources; debug preferences do not rewrite those files.
History browsing may read the existing production records; debug will not run
the writable access-file repair path. Explicit log export may save a new local
diagnostic artifact without changing the source records.
The normal app's bundle identifier is unchanged.

On Unix, real launcher/IMG/VID processing and the packaged verification,
collection, XMP merge and Photos import tools acquire one per-user production
task lease. Internal workers and managed tool subprocesses inherit the same
lease, so a GUI or parent exit does not release it while those children are
still working. An independent task is rejected before processing; read-only
preflight and configuration queries remain available. Existing per-directory
locks continue to protect their narrower operations. The LaunchServices
PhotoKit helper holds its own shared native-writer lock; new production tasks
check this lock as well, including after the parent exits during an import.
Non-Unix CLI
behavior is unchanged by this native-window feature.

`--self-test`, including the check invoked during GUI compilation, runs with
application activation prohibited and never brings its windows forward. Build
validation uses a temporary `LSBackgroundOnly` bundle with a separate identity,
the freshly compiled host and links to the real resources, so LaunchServices
does not first register it as a regular application. The production bundle's
metadata and final signing flow stay unchanged. It
still creates the real AppKit controls and exercises localized content, layout,
decision handling and close/reopen state. Alerts are laid out without modal or
sheet presentation; lifecycle notifications detect presentation during the run,
in addition to the final visible-window and active-application checks.
Foreground visibility, focus and actual user interaction remain explicit native
UI fixture/manual checks, not claims made by this background validation.

An optional external developer observer lives at
`crates/dev/native-qa/BackgroundBuildObserver.swift`. It is not part of normal
build or app startup. Compile it with the installed macOS Swift compiler and
run its deterministic lifecycle regressions before observing a build:

```sh
qa_root="$(mktemp -d)"
xcrun swiftc -swift-version 6 -strict-concurrency=complete -warnings-as-errors \
  crates/dev/native-qa/BackgroundBuildObserver.swift -o "$qa_root/observer"
"$qa_root/observer" --self-test
"$qa_root/observer" "$qa_root/report" /usr/bin/env \
  cargo run --locked --release -p dev --bin smart_build -- --gui
```

The command must actually run the private native self-test; a skipped GUI build
does not establish observation coverage. Reports must use a new directory.
Application equality binds samples to one lifecycle rather than treating a lost
PID as a new process. A previously observed application's confirmed termination
retains its valid identity and adds a terminal sample. The observer accepts
AppKit termination notifications and stops reading that lifecycle's process
properties or attributing later windows to its old PID after confirmed exit.
Polling also checks termination before reading those properties. Never-observed children,
unavailable window capture, conflicting identity and a child still running at
command completion cannot pass. Activation, visible windows and non-prohibited
policy observations remain failures after termination. The original native
presentation guard remains authoritative for activity between external samples.

## IMG Tool Runtime Lifetime

After effective configuration and explicit tool overrides resolve, IMG binds
its tool selection before media inventory or output preparation. On macOS the
required JPEG tools use a verified private executable/dependency snapshot;
standard IMG additionally captures its required FFmpeg, and AVIF Fast IMG binds
its AVIF tools instead of requiring unrelated JXL tools. Snapshots are recovery
state, not disposable cache. A subsequent run can select newly installed tools.

`MFB_TOOL_SNAPSHOT=` is a flushed schema-1 control event with `pipeline: "img"`,
`state` (`preparing`, `ready`, `blocked`), `boundary`, exact `tool_names`, optional
`identity` and `manifest_path`, and a bounded cause/message. Only the listed
snapshot tools are independent of their original installation. Ancillary media
executables are bound to selected absolute paths and observed when used;
ImageMagick delegates/plugins, external configuration and operating-system
libraries are not claimed to be frozen. Non-macOS currently reports
`live-observed`; it does not claim an independent dependency snapshot.

The launcher keeps the first verified snapshot authority for each pipeline in
the current batch and supplies it to later workers through their individual
environments. Starting another batch resets that in-memory binding. It does not
change the parent process environment or let one pipeline replace another's
tool selection. Matching tool selections and content reuse the verified runtime directory;
tools and dependency files are not copied for every media file. Source and
private-member hashes, original selection paths and the protected OS boundary
are retained in the manifest. Explicit `DYLD_*` loader overrides are rejected
rather than silently discarded or presented as a closed runtime.

The IMG binding scope is the shared media-tool name list. Operating-system
resource/control helpers such as `sysctl` keep their existing resolution policy
and cannot poison the media runtime merely because they are outside that list.
Asking whether an optional media tool exists does not count as executing it;
missing optional capabilities return unavailable without a shared-failure latch.
Actual execution still cannot substitute an unbound PATH encoder or verifier.

Fast IMG markers and standard IMG checkpoint headers retain the original private
manifest and identity. Resume loads that authority, checks its private material,
and refuses a conflicting supplied manifest or explicit tool override. Removing
or upgrading the original installation does not invalidate a complete private
snapshot. Historical completed-file proofs without a recorded tool authority
require a new task and fresh verification; they are not silently relabelled as
proofs from the new toolchain. Non-macOS has no persisted snapshot authority for
that reuse guarantee.

Before each encoding wave and critical delivery/deletion boundary, private
snapshot hashes are checked. Executable filesystem identity is an additional
early change detector, not content proof. After a failed operation, full private
snapshot hashes and bounded executable probes distinguish shared tool failure
from media failure. Shared failure stops
further admission; independently verified results from the current wave are
persisted, while its unresolved items and later inputs remain pending rather
than becoming fabricated media failures. The control event does not increment
media failure counts. A missing bound selection cannot fall through to a newer
bare PATH tool. These checks preserve the existing E1–E11 comparisons and all
final verification/deletion requirements; no speed improvement is claimed.

A slow or unconfirmed version/help probe is not evidence of a damaged tool or
media file. The shared probe uses the existing tool readiness budget with a
diagnostic soft deadline. An unresolved response retains pending work for retry
and reports `tool_probe_unconfirmed`; it does not poison the stored tool
identity. Actual missing/changed materials and explicit failed probes remain
distinct, recorded errors. Probe output is bounded and child processes remain
managed for cancellation and cleanup.

The GUI shows a compact, localized tool status independently from media counts.
Shared faults retain their stated cause even if a later worker emits another
ready event. The in-app tools guide explains fixed copies, upgrades, resume and
the external configuration/platform boundaries; detailed diagnostics remain in
the worker output and history. `--tool-runtime-ui-fixture` renders synthetic
tool status without launching processors or querying a real Photos library.

## Metadata Diagnostics and History Overview

A successful metadata audit emits one structured INFO event, rendered once by
each diagnostic sink. Its source, destination, policy or XMP context, category
and audit call site remain available; the terminal uses the same event for its
human-readable message. This does not reduce metadata checks or remove captured
external-tool evidence. Existing logs and history records are not rewritten.

JPEG encoder failures retain the operating system's actual termination status,
including a signal number and native core-dump indication where available.
A signal alone does not identify its cause; it must not be labelled as an OOM,
signature failure or corrupt input without further evidence.

The ordinary History storage overview does not scan captured-output rows or
their content-byte totals. Schema-2 `output_rows` and `output_bytes` are null
when these details were not scanned, rather than fabricated as zero. The GUI
labels this as an overview choice, independently from missing-schema support or
a failed read. `--history-stats --check-integrity` still scans the records and
returns exact output totals alongside its read-only structural checks. Those
checks do not establish media, event-JSON or Photos correctness.
