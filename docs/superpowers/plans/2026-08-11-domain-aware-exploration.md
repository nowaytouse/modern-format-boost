# Domain-aware exploration implementation plan

## 1. Shared policy contract

- Add failing unit tests for strict-smaller equality, bounded growth, probe
  classification, domain equality and detailed optimization outcome.
- Add a small foundation exploration-policy module.
- Keep delivery/copy semantics out of `SizePolicy`.

## 2. VID final-domain settlement

- Add tests that a different preset or timeline requires final-domain
  calibration/materialization.
- Replace direct search-CRF transfer with a final-domain bracket seeded around
  the locator.
- Make Phase 5 select the lowest verified fitting CRF, not the smallest output.
- Keep failed probes out of size bounds and restore the previous product on
  failure.

## 3. VID final evidence

- Add tests proving search VMAF/PSNR cannot become final metrics.
- Freshly measure final VMAF, PSNR-UV and CAMBI from the selected output.
- Keep search metrics as telemetry only.
- Bind result evidence to the materialized candidate domain.

## 4. IMG policy migration

- Replace local AVIF strict-smaller and probe enums with the foundation policy.
- Route JXL finalist and refinement size checks through the same policy.
- Record final effort/speed domain in exploration results.
- Preserve one-domain search unless a true final-domain calibration is added.

## 5. Outcome migration and delivery

- Add detailed optimization outcome to shared task results without breaking
  the existing coarse outcome API.
- Populate it for direct adoption, lossless transcode, explored optimization
  and failure paths that participate in exploration.
- Update user-facing exploration logs and changelog.
- Update the hardening document only with verified completion evidence; delete
  it only if every acceptance item is satisfied.

## 6. Verification and handoff

- Run only the verification explicitly permitted by repository hardening rules.
- Format the workspace, audit the lockfile update and inspect the complete diff.
- Commit and push the complete scoped change.
