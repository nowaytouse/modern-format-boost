# Domain-aware IMG/VID exploration

## Problem

The project has several exploration loops with a common product goal but no
common evidence contract. IMG uses JXL distance and AVIF quality while VID
uses CRF/CQ. Each loop currently decides size eligibility, probe failure and
final outcome locally. More importantly, some VID paths transfer a coordinate
found with a search preset directly to the final preset and reuse search-time
quality metrics.

Raw quality coordinates are not portable. Equal `distance`, `quality` or
`CRF` values produced by different encoders, effort levels, presets, speeds,
timelines or pixel pipelines do not identify equivalent media products.

## Product objective

Every lossy explorer selects the highest-quality **verified** product that
satisfies the active size policy. Output size is an eligibility constraint
and a tie-breaker, not the primary optimization target.

Lossless adoption and lossless transcoding remain separate outcomes and do not
enter a lossy quality search merely to satisfy this abstraction.

## Shared contract

Foundation owns the small policy vocabulary; individual IMG and VID explorers
keep their independent search implementations.

### Encoder domain

An encoder domain identifies every setting that changes the meaning of a
quality coordinate:

- codec/encoder family;
- effort, preset or speed;
- still, sampled or full-timeline encode mode;
- pixel/colour pipeline identity where applicable.

A coordinate may only be ordered against another coordinate from the same
domain. A result from a cheaper domain is a locator hint, never final evidence.

### Size policy

All explorers ask one `SizePolicy` whether a measured pure-media payload fits.
The first production policies are strict-smaller and bounded-growth. Equality
is rejected by strict-smaller. File delivery requirements remain a separate
concern and are not disguised as a size policy.

### Probe and outcome semantics

A probe is one of `Fits`, `Oversize`, `Failed` or `Unverifiable`. Only a real
measurement can establish a size boundary. Failed or unverifiable probes are
recorded but cannot move either bound.

The optimization result distinguishes at least:

- `Adopted`;
- `LosslessTranscoded`;
- `ExploredOptimized`;
- `Failed`.

The existing coarse task outcome remains available during migration so callers
do not lose compatibility.

## Search protocol

1. Detect source semantics and select the active size policy.
2. Select the final encoder domain before searching.
3. Optionally use a cheaper domain to produce only a locator hint.
4. Encode real anchors in the final domain around that hint.
5. Establish a bracket from measured `Fits` and `Oversize` anchors.
6. Refine only inside the final domain; failures do not alter the bracket.
7. Select the highest-quality verified fit. Size breaks ties only at equal
   quality.
8. Ensure the selected domain/coordinate is materialized on disk.
9. Freshly measure the current product's size and applicable quality metrics.
10. Bind final evidence to that product identity; never reuse another
    candidate's or another domain's metrics.

If final-domain anchors are not monotonic, the explorer stops interpolating and
uses bounded final-domain midpoint/neighbor probing. It never invents a
boundary from the locator domain.

## VID corrections

- Search-preset CRF is only a locator for a different final preset.
- Final-preset settlement must establish its own fit/oversize evidence.
- Downward quality exploration accepts a larger-than-current product whenever
  it still satisfies the active size policy; this is a quality improvement,
  not a failed size optimization.
- Final VMAF, PSNR-UV, SSIM/MS-SSIM and CAMBI are measured from the selected
  materialized output. Search metrics may be telemetry but not final truth or
  a candidate-specific final threshold.
- Sampled-timeline coordinates are also locator-only for a full-timeline
  product.

## IMG corrections

- AVIF Meme probes and JXL probes use the shared size and probe semantics.
- AVIF speed and JXL effort are encoder-domain identity, not independent
  quality axes.
- The current single-final-domain JXL search is valid and remains the safe
  default. A future cheaper-effort locator is allowed only with final-effort
  anchors and recalibration.
- AVIF Meme currently searches at its final speed; any future fast-speed coarse
  pass follows the same locator-only rule.
- JPEG/lossless/protected-source routing stays outside inappropriate lossy
  exploration.

## Verification matrix

Contract tests cover strict equality, bounded growth, failed probes, domain
mismatch and outcome semantics. VID tests cover preset/timeline mismatch,
final-domain anchoring, quality-first Phase 5 selection, stale metric rejection
and final materialization identity. IMG tests cover AVIF and JXL use of the
shared size policy, highest-quality fitting selection, protected-source routing
and one-domain effort/speed identity.

The hardening task is complete only when all production explorers use the
shared policy vocabulary and final evidence is tied to the current product.
