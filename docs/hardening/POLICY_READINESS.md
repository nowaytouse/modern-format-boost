# Policy Readiness Inventory

Status: current-policy inventory and responsibility boundaries, 2026-10-11.
This is readiness work, not a generalized product or a new processing pipeline.

The 2026-10-15 IMG correction uses `a8df1539` as the historical algorithm
reference. [Current Private IMG Policy](../dev/config/IMG_PRIVATE_POLICY.md)
records source/mode defaults, source-specific exceptions and fixed versus
sequential exploration. A settings preference does not replace those archival
rules. Cross-effort D assistance is not part of this first closure.

## Current Decisions

| Decision | Classification | Current Owner |
| --- | --- | --- |
| Standard IMG exposes JXL, not the Fast IMG AVIF mode | Product choice | `delivery_codec_strategy::ImgCodec::parse_cli_label` |
| Animated inputs leave the standard static IMG domain | Product/domain boundary | `img::conversion_api::determine_strategy` |
| Existing lossy modern containers retain their native encoding | Preservation constraint plus route choice | `img::conversion_api::determine_strategy` |
| Lossless modern inputs may enter the JXL archive route | Product choice constrained by fidelity proof | `img::conversion_api::determine_strategy` |
| Fast IMG JXL archival delivery and AVIF Meme Mode have different metadata contracts | Explicit mode choice | `img fast-img`, `foundation::image::fast_img` |
| JPEG effort 11, strict fidelity fallback, analysis/database disabled by default | Current preference/default, not an immutable format limitation | `runtime_config::ImgPolicy::default` |
| Confirmed video loop intent is routed to animated-image processing unless forced | Product routing choice | `vid::conversion_api::determine_strategy` |
| AV1 with Apple-compatible delivery is rejected | Current backend/delivery capability boundary | `VidCodec::validate_delivery_flags` |
| Lossless archival MKV currently requires HEVC | Implemented delivery capability, not a claim that other codecs cannot be lossless | `VidCodec::validate_lossless_archival_delivery` |
| Photos backend, root, album names and folder structure are configurable | User preference constrained by library identity and delivery proof | `runtime_config::PhotosPolicy` |
| Batch sizes, concurrency, cache capacity and TTL | Scheduling/resource policy; cannot weaken fidelity | Shared runtime configuration and owning stage |
| Codec-tool fallback and media-fidelity fallback are separate decisions | Tool capability versus preservation policy | `runtime_config::ToolsPolicy`, `ImgPolicy` |

These classifications do not remove any current interface. In particular, existing
CLI aliases and environment compatibility in `runtime_config::load` are not
silently retired by this inventory. Any cleanup needs its own usage review and
explicit rejection/migration behavior.

## Responsibility Boundaries

1. **Analysis** establishes content type, metadata and media relationships. It
   returns facts or an explicit error; it does not delete, import or choose a
   different fidelity contract.
2. **Policy** combines those facts with the resolved configuration and current
   product choices. A preference cannot turn missing evidence into success.
3. **Planning** describes work, retained sources and dependencies. A complete
   read-only run-wide Preflight is still pending; existing scans must not be
   described as that complete architecture.
4. **Execution** produces candidates in staging. Encoder success alone is not a
   delivery result, and failure does not authorize modifying the source.
5. **Verification** proves the active route's payload, metadata, color,
   orientation and relationship contracts. Reusable evidence must be bound to
   the same immutable artifact; a changed artifact invalidates it.
6. **Delivery** commits verified outputs or imports exact assets. Source cleanup
   remains gated by the corresponding durable evidence, not a progress counter.

The current code already supplies content detection, typed effective configuration,
per-file strategy selection, staged commits and route-specific proofs. A future
Preflight should connect those owners, not introduce another detector, another
configuration resolver or a parallel source of truth for counts.

## Behavior Preservation

- Do not change default JXL/AVIF/HEVC routing as part of readiness work.
- Preserve strict distinction between failed, skipped, ignored and unprocessed.
- Preserve original media and sidecars on incomplete or conflicting proof.
- Preserve unrelated embedded XMP when a sidecar supplies only part of the fields.
- Compare semantic metadata and binary thumbnail contents, not relocated IFD1
  thumbnail offsets. MakerNotes offsets remain subject to their existing contract.
- Copy the complete native XMP packet without rationalizing sidecar values.
  Preserve unrelated embedded fields by adding only missing tags to a staged
  sidecar first; retain full post-write verification of the original and sidecar.
- Do not add Profiles, Presets, plugins or user-selectable weaker verification.

The inventory and boundaries are documented. A unified read-only Preflight,
complete per-item planning and a generalized policy engine are **not implemented**
by this work. Existing route and synthetic metadata regressions remain the
behavior-preservation checks; there is no claim of production-library acceptance.
