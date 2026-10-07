# Photos Import Acceptance Record

Updated: 2026-09-30. Implementation and observed runtime evidence are separated
below. No scheduled monitoring is configured. Use the original authorized
`~/Pictures/debug.photoslibrary` directly, not a copy or another library.
All benchmark media is generated; no private originals belong in this repository.

## Implemented Architecture

- One persistent, versioned Swift PhotoKit helper and one writer. File-backed
  resources are grouped into transactions; album membership is added once per
  album per transaction. Placeholder identifiers bind to manifest entry IDs,
  never enumeration order or identical-content guesses.
- Independent import and verification windows. At most one next import overlaps
  verification/checkpointing. Only paths, hashes and identifiers are queued,
  not decoded images. Backpressure bounds pending work by the two window sizes.
- Submitted/identifier-known/committed/uncertain journals precede new work;
  existing marker proofs represent verified/checkpointed/cleanup progress.
  Recovery reconciles all old intents before importing, even if batch size changed.
  Verification failure drains the in-flight reply before leaving the safe boundary.
- Fresh UUID, original-byte, metadata and checkpoint gates still precede source
  deletion. A partial or uncertain commit is not a rollback claim and is never
  blindly replayed. Auto fallback is permitted only before any native intent.
- Optional bounded adaptive batching responds to verified-cycle latency and
  memory pressure. It remains off by default; one writer remains the default.
- Native xattr access eliminates one child process per file at both preparation
  and verification. Missing attributes are distinct from unreadable/missing files;
  unexpected errors fail closed and unrelated attributes are retained.
- Profiles report actual transaction timings, query/hash/checkpoint durations,
  percentiles, committed/verified counts, backlog, helper peak RSS and CPU.
  Timings may nest and explicitly exclude encoding and final source cleanup.
  The native GUI developer panel shows these measurements and unknown fields
  honestly, retains failures and resets between batches.
- Parent and nested helper both declare Photos permission usage. This fixes the
  observed TCC termination caused by attribution to the parent app.
- The existing AppleScript compatibility path remains available. Compound
  resource lists are supported by the protocol; Live Photo/RAW+JPEG end-to-end
  delivery is not enabled by this change. Tier-2/noncheckpoint imports retain
  their explicit AppleScript-only boundary.

Configuration, source provenance and safety limits are documented in
[runtime configuration](../dev/config/RUNTIME_CONFIG.md).

## Recorded Local Results

Synthetic 96x64 JPEG-to-JXL originals on the same original debug library.
These are observed runs, not promises for full-size photographs or other Macs.
The backend-only fixture and production-pipeline fixture are distinct scopes.

| Run | Transactions | Import + original verification | Result |
| --- | --- | --- | --- |
| Native backend, 1K, batch 250 | 4 | 24.745 s | 1,000 distinct UUIDs and original hashes |
| Native backend, 10K, batch 500 | 20 | 304.148 s | 10,000 distinct UUIDs and original hashes |
| Production native 1K before xattr/album fix, 500/250 | 2 | 101.073 s including preparation | Resume 88.765 s; no duplicate imports |
| Production native 1K after xattr/album fix, 500/250 | 2 | 22.586 s including preparation | Resume 8.515 s; no duplicate imports |
| Production AppleScript 1K, batch 10 | 100 | 792.249 s including preparation | Resume 1578.553 s; no duplicate imports |

The final 1K import is about 4.48x faster than the preceding production run.
Within the measured native profile, verification windows fell from 48.889 s
to 7.900 s; the helper peak RSS fell from 272,744,448 to 35,749,888 bytes.
The measured bottleneck was per-file xattr process startup plus repeated album
change requests, not BLAKE3 hashing (0.038 s in the improved run).
All source/output files were retained by the benchmark; no cleanup timing is
claimed. The 1K pipeline fixture includes an embedded XMP overlay.

The completed AppleScript baseline retained all 1,000 source files, returned
1,000 distinct UUIDs, and left the library count unchanged on resume. Its total
import/resume run took 2372.264 s. The observed native import was about 35.1x
faster for this small synthetic fixture, but the runs were not simultaneous
controlled trials: library history and concurrent build activity differed.
This is not a general speedup claim for real photos or a manual Photos baseline.

Live debug smoke also verified AppleScript import and both resume forms, native
post-commit verification interruption, retained sources, unchanged recovered
UUIDs, and no duplicate asset count. Local-only custody is not iCloud upload proof.

## Repeatable Harness

The ignored foundation tests are explicit opt-ins because they write Photos:

- `photos_native_debug_benchmark`: backend transactions, unique identity and
  original-byte verification. `MFB_PHOTOS_BENCH_COUNT` accepts 1 through 100000;
  `MFB_PHOTOS_BENCH_BATCH_SIZE` accepts 50/100/250/500/1000 (plus small smoke sizes).
- `photos_pipeline_debug_benchmark`: actual checkpointed production importer,
  generated file-backed JXL/XMP inputs, source retention and resume identity.
  Select `MFB_PHOTOS_IMPORT_BACKEND=photokit` or `applescript`; native transaction
  size uses `MFB_PHOTOS_NATIVE_BATCH_SIZE`.
- `photos_import_live_smoke_debug_library`: real custody and resume test.
  `MFB_LIVE_PHOTOS_SMOKE_NATIVE_FAILURE=1` with the native backend injects a
  post-commit verification interruption, including a next in-flight transaction.

All require `MFB_LIVE_PHOTOS_SMOKE_DEBUG_LIBRARY` pointing to the authorized
original debug library. Native tests need `MFB_PHOTOS_NATIVE_HELPER_APP` pointing
to the signed bundled helper; backend-only benchmarking also requires an existing
`MFB_PHOTOS_NATIVE_WITNESS` identifier. Target mismatch fails before import.
Each benchmark prints its private evidence directory and preserves its report,
inputs and journals. Run one live Photos test at a time.

## Independent Runtime Acceptance

Keep these evidence gaps open until executed; unit tests or CI cannot close them:

1. Controlled repeated manual Photos/AppleScript/native comparisons across 1K/10K/50K,
   full 100K production stress, batch sweep, adaptive calibration and repeated
   long-run RSS/Photos-daemon measurements. Harness support is not a passed run.
2. OS-kill/timeout and actual PhotoKit partial-result experiments, denied/revoked
   permission, empty-library targeting and interactive pause/stop/quit checks.
   The fail-closed implementation does not assume transaction atomicity.
3. Representative EXIF/IPTC/timezone/GPS/ICC/HDR/high-bit-depth JXL import and
   public Photos re-export matrix. Exact stored-original hashes are proven for
   the synthetic fixtures; rendered appearance and all metadata interpretations
   are separate acceptance criteria.
4. iCloud-enabled runs under separate cloud-operation authorization. Distinguish
   local commit, visibility, original availability and upload completion.

The long private task draft is not publication material. Retain only this
sanitized acceptance record and durable implementation contracts after cleanup.
