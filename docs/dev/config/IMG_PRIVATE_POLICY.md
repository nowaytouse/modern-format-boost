# Current Private IMG Policy

Algorithm reference: `a8df153932f34869a7871108b30c759bca1d4605`.
The relevant encoder/explorer files match `dea69c6b` at that reference.
This document describes the current private policy, not a generalized policy
editor, a new algorithm or complete end-to-end acceptance.

## Policy And Settings

Policy chooses the archival route and its fidelity obligations. Settings adjust
only parameters that route permits. A global preference is not a per-file proof
and cannot authorize lossy conversion, container flattening or source deletion.

Nullable pixel D/E preferences retain source/mode decisions when unset. CLI,
configuration and GUI use the same typed preferences. Existing mode flags remain
interfaces to their existing behavior, not an additional source of quality rules.
Normal direct encoding has an E7 policy ceiling; Ultimate/archive uses E10. The native launcher
retains its historical Ultimate mode. PNG's dedicated default is E10, while
ordinary reversible JPEG transcode defaults to full E1..E11 comparison. E11 is
the initial manual value if fixed is explicitly chosen. Eligible pixel E and D
searches default to adaptive. Persisted explicit choices are not overwritten.

## Source Routes

| Source Or Entry | Current Policy | Important Boundary |
| --- | --- | --- |
| Ordinary JPEG | Reversible JPEG-to-JXL transcode, independent JPEG E | Exact source-byte reconstruction, not pixel D selection |
| Genuine PNG and confirmed-lossless stills | Lossless JXL pixels, D0 | D0 is a private archival contract, not merely a GUI default |
| Eligible legacy-lossy stills | Existing near-lossless/fixed D or guarded D search | Content-based compression admission, size and verification remain required |
| Modern lossy static containers or existing JXL | Preserve native encoding | Selecting D/search does not override retention |
| Unknown or structurally protected containers | Existing retain/ignore/error decisions | Missing evidence does not unlock lossy encoding |
| Strictly verified one-frame video container | Existing static IMG route | Frame count, streams, timing and metadata admission remain required; not a VID handoff |
| HEIC/UltraHDR synthesis helpers | Historical mode D1 or Ultimate D0.001 | These helpers are not ordinary lossless PNG encoding |
| Independent UltraHDR JPEG migration helper | Historical mode E7/E10 with reversible JPEG encoding | Unused legacy representation; explicitly rejects full search rather than claiming the current archive minimum |

These are route summaries, not substitutes for detailed container admission.

### JPEG Structure, Geometry And Color Evidence

JPEG completeness follows the primary image's length-delimited segments and
entropy scans through its EOI. Markers inside metadata or after that EOI do not
define another primary scan; arbitrary trailer bytes remain covered by exact
JPEG reconstruction. This structural check does not replace codec validation.

Shared analysis decodes raw sample geometry with the configured dimension and
output-allocation limits. EXIF orientation is applied explicitly by geometry
verification, not implicitly a second time by a high-level image reader.
After exact JPEG reconstruction, the independent geometry proof requires the
source's IFD0 orientation and raw JPEG dimensions to match the actual JXL
codestream fields reported by `jxlinfo -v`. The independently decoded display
dimensions must also match that source orientation. Equal dimensions are only
one required geometry fact, never an orientation oracle. Missing, conflicting,
invalid or failed probes reject the proof; an incomplete source metadata read
cannot silently become orientation 1. A failed PNG rendering decode cannot be
replaced with JPEG reconstruction and called a successful rendering proof.

Pixel-encoded JXL uses exact samples in the source display domain, preserving
integer precision, alpha and floating-point bit patterns through orientation
transforms. Pearson correlation, flat-field exemptions and choosing whichever
raw/oriented reference looks closer cannot authorize JXL delivery or recovery.
The existing D/E search algorithms are not changed, but a changed-sample D>0
candidate currently lacks a separately validated strict lossy acceptance
contract. Such candidates are retained as failures rather than delivered by the
old statistical check. This is an explicit remaining limitation, not completion
of the private IMG algorithm/acceptance loop. The format-only default for AVIF,
HEIC/HEIF and WebP is also exact: choosing a codec does not opt into a statistical
acceptance gate. Existing changed-sample lossy outputs can therefore be rejected
until their separate strict acceptance contract is established; selecting a
lossy codec or enabling source-quality inference is not a validation bypass.

AVIF preprocessing and independent sample verification use the official decoder
with sample-transform layers enabled (`avifdec --sato --depth 16`). A 16-bit AVIF
can carry a 12-bit primary image plus residual sample data: expanding only the
primary image into a 16-bit buffer does not preserve those original samples.
HDR AVIF RGB/RGBA intermediates retain all integer samples in metadata-free PAM,
with explicit source-derived PQ/HLG signaling. The synthetic regression requires
the original generated PNG, decoded AVIF and decoded JXL to agree sample by
sample, including transparent pixels and low alpha bits. Decoder failure or
unsupported sample-transform options reject the path and retain the source.

PNM/PAM reference images are identified from their actual content and must fully
decode before their format's absence of an orientation transform is used. An
unrecognized or truncated file does not inherit a normal orientation merely
because its metadata tool cannot read it.

The float32 Display P3 regression uses deterministic source samples and assigns
ICC bytes without a color transform. Successful conversion must retain every
float32 bit pattern, ICC bytes and XMP. Some encoder builds reject valid negative
samples; rejection must identify the encoder failure, leave source and sidecar
unchanged and publish no output. This is an encoder limitation, not permission
to clamp samples or a policy forbidding negative values. A supporting encoder
must satisfy the same exact-success proof.

Exact JPEG reconstruction, filesystem metadata, embedded tags and usable color
profiles are separate proofs. A malformed embedded ICC profile is not accepted
merely because the original JPEG can be reconstructed. Current reversible
encoding does not silently trim, replace or discard such a source profile.
Similarly, a readable but truncated Exif block can be ignored by the encoder
while still carrying a non-normal source orientation. An exact JPEG round trip
does not resolve that conflict. Rewriting the source metadata is not an
original-byte-preserving repair and is not performed implicitly.
For example, complex TIFF/ICO/auxiliary-image inputs may require retaining their
original container despite having a recognizable raster. Nothing here changes
those checks, source identity, XMP relationships, ICC, gain maps or timestamps.

### Independent JPEG Reconstruction And Rendering Flags

Strict restoration compares the complete JPEG produced by the official decoder
with the complete independent `jxl-oxide`/JBRD reconstruction. The local `jxl-jbr`
patch accepts adaptive LF smoothing because reconstruction reads raw integer LF
coefficients, while display rendering smooths a separate floating-point copy.
It does not reconstruct JPEGs from rendered pixels or waive a byte mismatch.
External LF-frame references and the remaining frame/quantization restrictions
still apply. Source identity, XMP agreement, final publication and cleanup proofs
remain separate requirements.

A generated gradient regression carries the actual smoothing flag, proves exact
JPEG bytes and rejects appended bytes. A disposable copy of the reported JXL also
completed strict local restoration with the source retained and its restored
JPEG matching the official decoder's bytes. This is scoped restoration evidence,
not certification of other files or Photos delivery. Patch provenance and fixture
generation are recorded in `vendor/jxl-jbr/MFB-PATCH.md`.

### Positive Modern Static Admission

Fast IMG's direct modern-lossy route requires positive static and lossy evidence.
WebP admission parses top-level RIFF chunks, requires valid native codec features
and nonzero dimensions, rejects animation and requires a complete bounded decode
whose dimensions agree. A `VP8 ` chunk name or failed animation probe does not
prove admission. Compressed input is bounded while reading, including growth
after the initial size check. Source hashes before and after classification must
agree with the final detected format.

An executed forensic validator's rejection remains an error; native evidence
cannot replace that failure. JP2 without its required structural validator is
retained. Explicit frame counts must be exactly one, rather than zero or merely
at most one. These corrections do not complete the review of every modern
container, auxiliary-image relationship or final delivery path.

## Metadata Preservation Evidence And Limits

Maximal metadata retention is the archival goal, not an unconditional guarantee
for every field in every container. Exact JPEG reconstruction retains the complete
original JPEG bytes, including opaque/vendor data, but does not establish that
all applications can read those fields directly from the JXL. Portable EXIF,
XMP/IPTC, ICC, codestream orientation, gain maps, filesystem attributes and
delivery custody have separate evidence requirements. Unsupported or unchecked
fields cannot be certified from a successful exit or matching file counts.

Shared XMP discovery scans the complete adjacent directory for supported stem
and compound names. Exactly one regular, non-symlink candidate may be selected;
multiple candidates, directory-entry failures and invalid file types are errors,
not absence. Content validation, overlay/reconstruction custody and source/sidecar
identity checks remain necessary. Filename agreement does not prove historical
ownership. Tier-2 delivery isolates a failed association to the affected asset
and retains its source and sidecars; it does not silently import it without XMP.

Passthrough copies obey the same discovery rule. Matching media bytes alone do
not authorize a clean skip: the supported filesystem metadata and any companion
XMP must also verify. Stale or missing metadata uses the existing preservation
path rather than copying source media over an already identical destination;
a failed repair remains a file failure. Ambiguous, malformed or conflicting XMP
is not absence.

Copied-file XMP embedding runs on staging. Source, selected packet and previous
destination identities/bytes, paired embedded metadata and supported filesystem
attributes are checked before publication. The explicit sidecar fallback copies
the original packet bytes, rather than running a metadata writer over the packet
itself. It verifies bytes and filesystem metadata, refuses a differing existing
companion and publishes a new companion without clobbering concurrent output.
This adds bounded staging work to copied-file XMP embedding. An unproved
container is refused before that staging work, allowing the verified companion
fallback without a redundant media copy. Ordinary encoding routes are unchanged;
these checks do not establish a general performance improvement.

The reconstructible JXL commit uses the full existing final integrity proof on
the metadata-enriched staging file, including exact JPEG bytes, native gain maps
and explicit stream orientation, before atomic publication. Later batch,
delivery and deletion gates remain independent. Encoder partial bytes are not
published merely because they have a final extension.

Fast AVIF Meme Mode deliberately clears removable embedded metadata. It is not
the maximal-preservation archival route. Filesystem precision/policy exclusions
also remain explicit; verified Photos local resource custody does not certify
every visible Photos field or iCloud synchronization. The native question-mark
guide explains these distinctions in its metadata topic.

## Fast IMG Saved-Task Choice And Recovery Boundaries

Without an explicit resume/fresh selection, a saved Fast IMG task requires a
choice before recursive inventory, codec probes or source hashing. The native
host owns that choice; a GUI launch cannot wait for terminal input even if output
uses a PTY. Continue means revalidate and continue, not trust an old success flag.
Explicit resume still performs the complete existing source/output/hash and
delivery reconciliation; fresh selects a separate output rather than overwriting
the recorded task. Dry-run remains observational.

Checkpoint updates use a private unpredictable temporary file in the state
directory, sync its bytes and atomically persist it, then sync the directory.
A stale predictable `.json.tmp` path is neither followed nor truncated. The
source/output locks remain required; atomic persistence is not a replacement for
writer serialization or per-asset evidence.

`_optimized` remains a working directory until its applicable gates and delivery
finish. A hard crash after a verified file is published but before its checkpoint
entry is recorded can still leave an unregistered complete artifact. Its suffix
does not authorize reuse, import or deletion. Recovery must retain unknown
ownership and can stop on extra outputs; this change does not implement atomic
filesystem/checkpoint journaling or complete failed-only retry.

## Inspection Progress And Process Interruption

Modern-source inspection reports actual completed probes against that stage's
known inventory. Eligible, ineligible and failed probes all count as inspected;
none is thereby a successful conversion or Photos delivery. Stage completion is
not batch completion, and progress is not a substitute for the admission proof.

Worker OS observations are shared by live presentation and persisted history.
Session exit diagnostics retain normal exit codes and termination signals as
separate facts. A final bound SIGTERM/SIGINT without file results or conflicting
evidence can be shown as a process termination with unsummarized file outcomes.
The failed process result remains recorded. This does not identify the sender of
the signal, certify retained originals or invent missing file counts. Other
signals, mismatched observations and integrity conflicts keep their serious or
unconfirmed presentation.

## Search Semantics

Fixed E passes one effort to the encoder. Optional E exploration compares
verified pure-media payload sizes at one quality in a bounded candidate range;
equal sizes prefer lower E. Search is the default, subject to the policy ceiling:
Normal E7 may admit E7 alone. A selected E is not evidence
that an exhaustive optimum was found, and E exploration is not a speed optimizer.
JPEG `adaptive` E comparison independently proves exact reconstruction for every
bounded candidate. Default `full` comparison covers all eleven E values using
complete JXL bytes after the same metadata enrichment, with lower E breaking
ties. E9 establishes the first size reference, followed by E1..E8, E10 and E11;
this changes execution order, not coverage or the winner's priority. It verifies
the first candidate and every contender with a better `(complete size, E)` pair, retains only
the winner and challenger, and reuses the winning physical file. It neither
changes JPEG quality nor uses pixel D. Fixed is an explicit manual alternative.
Full comparison does not use compatibility/repair retries or unproven equivalent
Effort groups: a failed/unsupported candidate makes the search incomplete and
preserves the source. Input, associated XMP and encoder executable content must
remain stable. Normal delivery verification and cleanup ownership still apply.
Equal-size lower-E replacements also require reconstruction proof. Reordering can
increase checks for some inputs; it is not a guaranteed speedup for every JPEG.

The existing Distance Explorer starts from the encoded D0 baseline when the
active size policy requires further work. That measurement is explicitly D0:
when it does not fit, the explorer separately encodes D0.001 before using that
floor for planning or early exit. Historical `a8df1539` also mislabeled the D0
size as D0.001; correcting this old coordinate defect is not evidence of a recent
algorithm regression. Ladder screening, finalist selection,
binary search and refinement remain in the same selected E domain. Among fitting
candidates the lower D is preferred; file size breaks ties at equal D.
Source admission and the minimum source-size gate still apply.

Distance screening returns its bounded-search report even when there is no
shortlist. It distinguishes the caller-supplied baseline measurement from floor,
ladder, discovery and binary probe requests, and records the measurement budget
and actual stop condition. A probe request is not an encoder-process count: a
request may include compatibility work or fail before launching an encoder.
Empty payload measurements and invalid baseline coordinates are rejected rather
than treated as evidence of a fitting or oversized candidate. Invalid final
distance/telemetry evidence is not clamped into a valid delivery result.

The final log separately records finalist attempts/failures, refinement attempts
and whether refinement stopped at local precision, its budget, an unavailable new
coordinate or a failed probe. A failed refinement can retain an already verified
candidate under the existing rules, but its failure stays visible. The selected
result is the best verified tested candidate; local binary-search precision does
not prove monotonic file sizes, a continuous global optimum or full media delivery.

Direct-cjxl distance probes retain actual candidate files for reuse across
screening, finalist comparison and refinement within one exploration. Identity
includes source and actual encoder input, selected XMP (including absence), ICC,
encoder executable content, effective arguments, working directory and frozen
process environment. Only the output-path argument is normalized; D and E remain
exact. Changed or unreadable input evidence rejects the exploration, even when
an individual screening callback would otherwise return no candidate.

The disposable pool holds at most 16 candidates and 64 MiB of complete files per
exploration. Evicted, oversized, missing or corrupt candidates require a fresh
probe; a recorded size alone cannot supply a candidate. Reuse checks complete
file content before and after transport, then repeats health and payload checks.
Transport refuses an occupied destination. Cancellation rejects a retained hit,
and finishing the search explicitly cleans remaining owned candidates. The log
separates probe requests, verified reuse, invalidation, eviction and direct launch
attempts; fallback requests are not a count of their internal processes.

ImageMagick fallback candidates are not retained because this path does not yet
provide the complete fallback-pipeline identity. This pool is neither a persistent
resume cache nor proof of dynamic-library or external configuration identity.
Its fresh command builder captures the inherited environment once and executes
that prepared command. It does not support arbitrary previously env-cleared
commands. Final metadata enrichment, pixel/geometry, size and publication Gates
remain independent and mandatory. Fewer duplicate encodes do not by themselves
prove lower total I/O, a representative speedup or a drive-lifetime improvement;
identity hashing and sidecar discovery also have a cost. XMP association is
discovered at search entry and checked again before accepting its result;
individual probes recheck the bound sidecar's content without rescanning the
source directory. A changed association rejects the whole exploration before
final metadata processing, including a search that otherwise found no winner.

`mode-default` retains the historical mode trigger and the manual D where that
route does not explore. Explicit adaptive D search leaves the saved manual D
inactive without replacing it. The native checkbox distinguishes inherited
policy from an explicit choice; resetting the tab restores inheritance.

Both explorations enabled means sequential E selection followed by D exploration
in that domain. It does not mean exhaustive joint search. Low-E assistance for
high-E distance search is a separate research proposal, not this implementation.
It must not replace the historical explorer without comparative evidence.

All screening, finalist and refinement probes obey the same alternative-encoder
permission: `tools.policy=single` blocks switching to ImageMagick even when
expert options are enabled. Required detection, decoding, metadata and final
verification tools are not disabled by that policy. Tool permission does not
authorize otherwise forbidden repairs or weaken the route's fidelity checks.

## Fast IMG

Fast JXL retains its independent JPEG reconstruction and native-media delivery
workflow. Its settings expose JPEG E and applicable verified E comparison, not
the pixel encoder's D. Normal IMG and Fast JXL use the same full-search engine,
while GUI profiles keep their own selected settings. Same-semantics compatibility
retries in fixed/adaptive modes remain subject to the configured fallback policy
and exact reconstruction checks; full mode requires exact independent efforts.

Fast AVIF retains the existing bounded Q search, locator Speed and final Speed
domains. `img.avif_quality` (10..100, default 100) supplies the one requested
quality only when search is fixed; automatic search retains but does not use it.
The existing progressive-output policy requires the fixed quality to be at least
10; general encoder support for non-progressive Q0 does not relax that policy.
`img.avif_speed` (0..10, default 0) selects the final domain for lossless and
quality probes. The locator remains Speed6 and its candidate is discarded before
re-probing in that final domain. Confirmed-lossless input still attempts the
existing true-lossless route first; existing AVIF is not re-encoded by these
settings. Final verification uses the actual delivery domain. Its Meme Mode
metadata cleanup policy is distinct from ordinary JXL archival metadata
preservation; these settings do not silently unify them. Fixed Q is not lowered
to manufacture success if its candidate fails verification or the size policy.

## Photos Query Recovery

The native backend's selected-library witness is a read-only query performed
before any new import request. Thirty seconds is a diagnostic threshold, not a
delivery failure. AppleEvent timeout or unavailable-connection responses can
trigger at most two non-destructive Photos activations and fresh queries, with
per-attempt budgets of 120, 240 and 480 seconds. The AppleEvent itself gets the
same budget; the subprocess gets five extra seconds to return its error.
These are bounded cold-start recovery windows, not proof of actual progress.

Every recovered query must return a valid identifier, and the existing PhotoKit
witness and active-library checks still prove the target. Permission failures,
invalid identifiers and a changed selected library do not become success through
retry. Exhaustion defers the native attempt, preserving the encoded outputs and
checkpoint; it never authorizes deletion or counts an import as delivered. Auto
mode may still use the existing AppleScript fallback, but only before native
intent and when no native journal exists; that path retains its own delivery
verification.
Cancellation is checked again after the query and native startup; it must not
select the compatibility backend or continue preparing delivery.

Import transactions are different: a timeout after an intent may have committed
assets. Existing journal/UUID reconciliation must resolve that uncertainty before
any replay. Recovery must not force-kill Photos. A graceful quit is followed by
an actual process-state check, and an unconfirmed exit defers the restart.
The osxphotos query timing cache learns only from validated responses covering a
nonempty request completely, never from partial results, command failure,
malformed JSON, permissions errors or conflicting UUIDs.

## Evidence And Remaining Work

### Direct Distance Candidate Reuse (2026-10-24)

A synthetic 64-by-64 RGB probe compared the new pool disabled and enabled, with
cjxl `0.13.0 777e88f4`, one encoder thread, E1 and exact D coordinates 0.001,
0.04 and 0.1. Three screening requests followed by the same three finalist
requests launched six encodes without retention and three with retention. All
three complete-file hashes, D bit patterns and payload sizes agreed. The repeat
measured 358 ms versus 223 ms for these six requests. Both variants include the
new identity checks; this is not a comparison with the historical implementation,
a representative batch benchmark or a claim about physical drive writes.

Twelve focused tests cover content corruption, missing/evicted/oversized files,
occupied and symlink paths, ownership cleanup, input/tool/ICC/XMP/context changes,
actual command preparation, candidate equivalence and cancellation in an isolated
child using the real batch-control protocol. Six existing distance/probe/final
rejection regressions also passed. These probe tests do not establish the
remaining strict D>0 acceptance contract or Photos acceptance.

### Full JPEG Search Order Comparison (2026-10-18)

Compared the ascending implementation from `6675cccd` with the reference-first
order on Apple M4, cjxl `0.13.0 ef67fde2`, two encoder threads and one search at a
time. Five inputs comprised three disposable media copies and two synthetic JPEGs;
the synthetic texture included an XMP sidecar. Three alternating runs per order
included candidate metadata enrichment, unchanged input/tool checks and exact
JPEG reconstruction. All eleven complete candidate hashes and the final output
bytes/effort matched across orders and repeats: 30 full searches, 330 encodes.

| Input | Winning E | Ascending Median | Reference-First Median | Reconstruction Calls |
|---|---:|---:|---:|---:|
| Synthetic gradient | 9 | 1.840 s | 1.768 s | 3 -> 1 |
| Media copy A | 9 | 3.681 s | 3.510 s | 5 -> 1 |
| Media copy B | 9 | 3.523 s | 3.455 s | 4 -> 1 |
| Media copy C | 3 | 6.837 s | 6.734 s | 2 -> 2 |
| Synthetic texture + XMP | 8 | 2.057 s | 2.003 s | 3 -> 2 |

The sum of per-input medians was 17.938 s versus 17.471 s, an observed 2.6%
difference; reconstruction calls per five-input sweep fell from 17 to 7. This is
a small experiment, not a statistical or representative batch-speed guarantee.
It excludes input scanning, final delivery Gates and Photos. An all-equal-size
curve requires two reconstruction checks instead of one because E1 replaces E9;
the deterministic regression records that cost rather than hiding it.

The correction is covered by Foundation JXL and IMG library/CLI regressions,
launcher forwarding checks, source contracts and native self-test. Native dark
and light windows were inspected. Small synthetic CLI runs confirm PNG D0/E10,
ordinary JPEG E11 and exact JPEG reconstruction, unchanged PNG RGB pixels after
fixed or E7/E8 comparison, and byte-identical lossy-WebP retention. These are
targeted correctness checks, not representative performance or full media
acceptance. See the dated changelog for the executed validation scope.

This correction establishes historical algorithm/configuration boundaries and
native parameter semantics. Full per-file persistent execution plans, a complete
private policy file/runtime binding, further AVIF metadata-policy configuration,
representative performance comparisons and full real-media/Photos acceptance
remain separate work. Do not delete the temporary strategy requirements merely
because this document or individual controls exist.

Encoder success, a smaller output and matching counts never authorize deletion
on their own. Source cleanup still requires the owning route's verified delivery
and durable proof.
