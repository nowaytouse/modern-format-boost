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
Normal direct encoding uses E7; Ultimate/archive uses E10. The native launcher
retains its historical Ultimate mode. PNG's dedicated default is E10, while
ordinary reversible JPEG transcode defaults to E11. Explicit E overrides and
opt-in E comparison are separate from those defaults.

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
| Independent UltraHDR JPEG migration helper | Historical mode E7/E10 with reversible JPEG encoding | Not the ordinary JPEG E11 default; retain reconstruction proof |

These are route summaries, not substitutes for detailed container admission.
For example, complex TIFF/ICO/auxiliary-image inputs may require retaining their
original container despite having a recognizable raster. Nothing here changes
those checks, source identity, XMP relationships, ICC, gain maps or timestamps.

## Search Semantics

Fixed E passes one effort to the encoder. Optional E exploration compares
verified pure-media payload sizes at one quality in a bounded candidate range;
equal sizes prefer lower E. It is off by default. A selected E is not evidence
that an exhaustive optimum was found, and E exploration is not a speed optimizer.
JPEG E comparison independently proves exact reconstruction for every candidate.

The existing Distance Explorer starts from the encoded D0 baseline when the
active size policy requires further work. That measurement is explicitly D0:
when it does not fit, the explorer separately encodes D0.001 before using that
floor for planning or early exit. Historical `a8df1539` also mislabeled the D0
size as D0.001; correcting this old coordinate defect is not evidence of a recent
algorithm regression. Ladder screening, finalist selection,
binary search and refinement remain in the same selected E domain. Among fitting
candidates the lower D is preferred; file size breaks ties at equal D.
Source admission and the minimum source-size gate still apply.
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
the pixel encoder's D. Same-semantics compatibility retries remain subject to
the configured fallback policy and exact reconstruction checks.

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

## Evidence And Remaining Work

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
