# JXL, XMP, and JPEG Archive Contract

This document is the production contract for reversible JPEG-to-JXL delivery,
lossless raster and proven-lossless modern-container JXL encoding, native XMP
overlays, FastImg Tier 2 custody, and JPEG restoration.

## Ownership boundary

| Layer                                                     | Owner                | Mutation policy                    |
| --------------------------------------------------------- | -------------------- | ---------------------------------- |
| JBRD and reconstructed JPEG bitstream                     | Reconstruction-owned | Immutable                          |
| Original Exif, XMP, ICC, JUMBF, unknown boxes, codestream | Reconstruction-owned | Immutable                          |
| Appended XMP overlay                                      | Overlay-owned        | Append only after validation       |
| MFB audit fields and manifests                            | Application-owned    | Versioned, atomic, and hash-linked |

An external XMP sidecar is never merged by rewriting a reconstructible JXL.
The complete existing container is copied byte-for-byte, one validated `xml `
box is appended, and exact JPEG reconstruction is proved again. Repeating the
same overlay is an idempotent no-op.

The decoder is invoked with `--reconstruct_jpeg` on the real input first. Its
help output is not used to infer support because released/development builds may
accept an option they do not list. Only a diagnostic that explicitly rejects
that option permits the extension-selected `.jpg` compatibility interface. A
supported strict operation that rejects the media never falls through. In both
cases, a zero exit status is insufficient: positive reconstruction output,
non-empty JPEG, no pixel fallback, JPEG health, and the outer BLAKE3/byte proof
remain mandatory.

## Archival source routing

The default IMG route protects functional archive value, not only visible
metadata:

- true JPEG, including UltraHDR/MPF JPEG, enters reversible JXL only when the
  original JPEG bytes can be reconstructed exactly;
- ordinary lossless raster sources such as PNG, BMP, and TIFF, plus modern
  WebP, AVIF, HEIC/HEIF, and JP2 sources with positive lossless evidence, enter
  pixel-lossless JXL with decoded RGBA16 equality and metadata audit;
- AVIF is decoded through the authoritative `avifdec` path only after an
  explicit gain-map probe. A present or unprovable gain map retains the native
  source instead of flattening HDR. HEIC/HEIF gain maps use the dedicated HDR
  JXL path with verified auxiliary sidecars;
- an existing JXL and every lossy or semantically unknown modern container
  remains byte-for-byte unchanged. Unknown archive structure is never inferred
  to be disposable from primary-pixel equality alone.

All `cjxl` outputs explicitly request the JXL container so append-only metadata
boxes remain available. Direct pixel encoding uses effort 7 normally and effort
10 for ultimate/archive work because effort 11 can become impractically slow in
that workload. JPEG bitstream transcode is a distinct, fast workload and uses
effort 11 by default; an encoder that rejects expert options is retried at effort
10. Neither path may bypass its pixel or exact-reconstruction proof.

## Durable overlay commit

The commit order is fixed:

1. Capture source length, modification time, device/inode identity, and BLAKE3.
2. Copy the complete JXL into a same-directory unique temporary file.
3. Append the validated XMP box.
4. Flush the temporary file and verify its size.
5. Prove that the complete pre-existing byte prefix and JBRD hash are unchanged.
6. Hash XMP through the same open descriptor used for copying, then verify the
   final `xml ` payload against that hash.
7. Recheck the live source identity and BLAKE3 to reject concurrent edits.
8. Atomically rename, then flush the committed file and parent directory.
9. Re-prove exact JPEG reconstruction or the original non-JBRD classification.

Session audit events link the transaction without storing media content:

- `MFB-JXL-001`: overlay UUID/schema, original container/JBRD/XMP hashes,
  overlay hash, final container hash, MFB version, and timestamp.
- `MFB-JXL-002`: final container/JBRD hash plus exact reconstructed-JPEG hash,
  or the explicit non-JBRD classification proof.

A crash may leave only a hidden, uniquely named `.tmp` recovery artifact; its
suffix cannot be rediscovered as media by JXL scans, and it cannot expose a
partially written final JXL. No source deletion is authorized by an unfinished
overlay transaction.

## Overlay lifecycle and parser bounds

The overlay chain is deterministic: an overlay UUID is derived from the prior
container/XMP hash, new overlay hash, and final container hash. Reapplying the
same latest XML is a no-op. A different XML becomes a new append-only version,
so historical bytes remain auditable; a container with 64 XML boxes refuses a
further overlay instead of growing without bound.

Sidecars must be regular non-symlink files and one complete XML document. The
parser rejects empty or over-100-MiB sidecars, document type declarations,
nesting beyond 256 levels, more than one million events, malformed XML, unsafe
container boundaries, duplicate top-level JBRD, and excessive JXL box counts.
These are fail-closed limits, not permission to truncate metadata.

## FastImg Tier 2 and AVIF Meme Mode

FastImg has one byte-preserving custody tier: JXL strategy Tier 2 admits only
positively proven lossy, static WebP, JP2, JXL, AVIF, HEIC, or HEIF
originals. It imports the admitted source without re-encoding and performs the
live Photos UUID/content-hash checks before cleanup.

AVIF strategy has no Photos-custody Tier 2. It has two input-driven paths:

1. an existing AVIF is never re-encoded: a metadata-clean file is adopted
   byte-for-byte; otherwise ExifTool edits only a staged container copy;
2. the staged AVIF is accepted only when its exact primary-image SHA-256 is
   unchanged, `avifdec` still reports the same codec/HDR/gain-map features, and
   the clear-metadata audit passes;
3. every other confirmed-static input is decoded, searched, and encoded in the
   AVIF domain with Exif, XMP, and ICC embedding disabled;
4. the final AVIF must pass pixel, dimension, payload and clear-metadata gates;
5. if a matching XMP sidecar exists, validate it before processing and remove that
   exact sidecar only after the same final delivery proof; retain it whenever a
   gate fails.

When no sidecar exists, the JXL Tier 2 path imports the original media bytes and
verifies them unchanged. When an adjacent XMP exists on a JXL container, the
eligible Tier 2 candidate instead:

1. validates the XMP;
2. creates an isolated copy of the media;
3. merges XMP into that copy (append-only for JXL);
4. imports the enriched copy;
5. verifies the live Photos asset UUID and enriched BLAKE3;
6. keeps separate hashes for the on-disk source and Photos-delivered original;
7. rechecks both the admitted source hash and sidecar hash after staging;
8. persists the admitted sidecar hash with the Photos proof;
9. removes the source and sidecar only while their current hashes still match.

If staging, metadata merge, import, or live-library verification fails, both the
source and sidecar remain. Tier 2 never deletes an unproved sidecar.

AVIF, HEIC/HEIF, WebP, and JP2 preserving routes use a staged format-native XMP writer
instead of a generic container rewrite. Commit requires an unchanged primary
image-data hash, dimensions and frame count, unchanged stable non-XMP
properties, and successful XMP readback. ISOBMFF aggregate `mdat` size is not an
invariant because the XMP item itself is stored there. Any failed or incomplete
proof retains both source and sidecar. AVIF Meme Mode is deliberately outside
this metadata-preserving writer: it strips embedded metadata and does not merge
the sidecar into the output. JXL uses the append-only overlay contract above.
Any JPEG APP11 segment blocks generic XMP rewrites because APP11 can carry JPEG
XT, JUMBF, provenance, or other protected structure; a PNG `caBX` chunk blocks
rewrites because it carries C2PA data.

## `restore-jpeg` input-driven behavior

There is one command and no user-selected action mode:

```text
img restore-jpeg INPUT
```

For ordinary files and folders, byte-identical JPEGs are restored. Source
cleanup remains behind the durable manifest and final hash gate;
`--keep-source` disables cleanup. The same run atomically commits
`.mfb_restore_jpeg_audit.tsv`: exact records get no marker, recovery-needed
records are mirrored under `Reconstruction Blocked`, and uncertain/invalid
records under `Needs Review`. Non-reconstructible media is never converted by
a pixel-to-JPEG fallback and remains untouched.

JPEG reconstructibility and metadata-layer agreement are separate facts. If
the JXL reconstructs the original JPEG but its container XMP differs from an
adjacent XMP, restoration still succeeds byte-for-byte and the adjacent XMP is
committed as the output sidecar. The source JXL and source sidecar are retained,
and the manifest records `MFB_RESTORE_JPEG_ATTENTION`; this preserves both XMP
layers for review without falsely calling the JPEG non-reconstructible.

For a Photos library or one concrete asset path inside it, the command switches
automatically to live-library audit. It resolves and re-queries real asset UUIDs,
hashes the current originals, and adds references for affected assets to
`MFB JXL Audit/Recovery Needed` or `MFB JXL Audit/Needs Review`, mirroring the
source folder/album hierarchy. Exact-reversible assets remain unmarked. MFB does
not rewrite media bytes or edit Photos database files directly; Photos records
only album membership. A separate atomic BLAKE3/UUID checkpoint records verified
membership for idempotent resume. Every CLI, interactive, and native-GUI launch
uses this same input detector and therefore never supplies a local output tree
to a Photos audit. Query and re-query results must contain valid, unique, exact
UUID sets; native album mutations are bounded into 50-asset transactions and
ambiguous duplicate folder or album names fail closed instead of selecting one.

Whole-library audit is the default. `img photos-albums LIBRARY` and the AppKit
picker expose the same live native container UUIDs. `--photos-album-id` selects
one exact album; `--photos-folder-id` expands the native parent graph to all
descendant album UUIDs before database filtering. The implementation does not
compare a native folder UUID with an unrelated database folder identifier and
does not use a display name as identity. Generated `MFB JXL Audit` containers
are excluded from selection, and each selected scope receives an independent
BLAKE3-derived checkpoint path.

## Recovery-original collection

`collect_optimized AUDITED DEST --backup BACKUP` is the only backup handoff;
it does not add another `restore-jpeg` mode. A single JXL accepts one exact
same-basename backup file or a backup folder. Folder inputs re-probe every JXL
from its magic bytes and accept a backup original only when the same relative
directory and basename resolve to one true JPEG payload. Photos inputs
re-probe live assets already referenced by `MFB JXL Audit/Recovery Needed`, then
resolve an exact original filename by one unique same UUID or album hierarchy;
capture time is reported as evidence but never selects a candidate. They export
the original version plus an XMP sidecar through `osxphotos`.

The collector never edits either backup or a Photos database. Missing,
duplicate, changed, JXL-only, or escaped-path results fail closed. Every copied
media/XMP output is BLAKE3-checked and recorded in the atomic
`.mfb_recovery_collection.json`; the Photos export database and update mode make
an interrupted export idempotently resumable.
`--dry-run` emits every resolved folder/Photos identity without copying media,
providing a review list that can be redirected for custom export tools.

Single-file input uses the selected file as its overlap boundary and its parent
only for relative output naming. Both the library default and the GUI's
sibling-of-file output are valid, while selecting the source file itself as an
output remains forbidden. Directory input and output roots must be fully
disjoint in both directions.

Valid pixel-only JXL, advertised-but-rejected reconstruction records, and probe
failures are classified in the manifest and marker tree or live audit albums.
They are not converted through pixel-to-JPEG fallback and are not deleted.

Pixel readability is not proof that the original JPEG file is still exactly
recoverable. JPEG reconstruction may depend on the reconstruction-owned Exif,
XMP, or JUMBF bytes referenced by JBRD. If a historical metadata writer changed
those bytes, exact recovery is possible only when the original metadata change
can be undone byte-for-byte or an exact original/backup is available. An
adjacent sidecar is useful metadata but is not assumed to be those original
container bytes. Reordering boxes, removing overlays, substituting a sidecar,
or accepting `--pixels_to_jpeg` must never be reported as original-JPEG
recovery. A pixel decode may be exported to a lossless pixel format for visual
rescue, but that is a derivative and does not repair JPEG bitstream identity.

## Read-only backup comparison

`collect_optimized CURRENT REPORT --backup BACKUP --compare` accepts **two
Photos library packages only** and never mutates either library or its assets.
Photos-library comparison uses `osxphotos compare --json`. An atomic
`mfb_backup_comparison.json` reports matched, source-only, backup-only and
different native-asset entries without storing absolute filesystem paths.
Folder/file comparison is intentionally outside MFB's scope; users should use
a dedicated external deduplication tool for ordinary filesystem inputs.
Non-Photos inputs, symlinks and incomplete upstream reports fail closed before
any report is written.

## Restore manifest and delete gate

Manifest V3 records safe relative paths, source-JXL hash, fresh reconstruction
hash, committed JPEG hash, optional XMP hash, verification time, MFB/djxl
versions, optional Photos UUID, and source-deletion state. The manifest is
written to a unique temporary file, flushed, atomically renamed, and followed
by a parent-directory flush.

Deletion order is non-negotiable:

1. exact reconstruction succeeds;
2. committed JPEG is a non-empty true JPEG;
3. fresh reconstruction and committed JPEG hashes match;
4. optional XMP sidecar is valid and hash-matched;
5. manifest with `source_deleted=false` is durably committed;
6. the current source/output hashes still match the proof;
7. source JXL and its matching XMP are removed;
8. manifest is durably updated to `source_deleted=true`.

Any failed or ambiguous gate retains the source. A verifier treats a manifest,
filesystem, or Photos-state disagreement as an integrity warning rather than a
successful migration.

Reconstruction caches, when introduced for performance, are never authoritative
for deletion. The final delete gate always performs a fresh hash and state proof.
