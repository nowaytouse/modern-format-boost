# Processing Counts

Counts describe processing outcomes, not proof of content preservation by
themselves. The payload, metadata, reconstruction and Photos custody gates still
decide whether a delivery is safe.

## Outcome Inventory

For a standard IMG/VID batch:

```text
processed = succeeded + failed + skipped + ignored
discovered = processed + unprocessed
```

The batch reconciles these equations after workers finish. Overflow, missing
terminal outcomes and more outcomes than discovered inputs are errors, not
clamped counts. Pause and fail-fast both report remaining work. A stopped batch
must not force its progress bar to 100 percent.

- `succeeded`: this route's required delivery proofs completed.
- `failed`: an explicit per-file failure; the source is retained.
- `skipped`: an intentional policy decision or valid checkpoint reuse, not a
  new conversion. Required source-preserving copies must succeed first.
- `ignored`: outside this processor's domain, not a delivery by this processor.
- `unprocessed`: no terminal result yet. In Photos workflows this also includes
  ready output that has not acquired the required library delivery proof.

Each processor's inventory is scoped to its own discovered candidates. Combined
IMG/VID totals sum processing outcomes, not necessarily unique filesystem files
or unique Photos assets. An item ignored by one processor may be owned by the
other. Resume counts describe valid entries in the current candidate set, not
all entries ever stored in a checkpoint.

## Evidence And Reporting

- Conversion success rate is `succeeded / (succeeded + failed)`. Skipped,
  ignored and unprocessed items are excluded. With no active outcomes the rate
  is unavailable (`N/A`), not 100 percent.
- Conversion byte totals cover successful conversions only, not copied,
  checkpoint-reused, skipped or ignored input. Their labels make this scope
  explicit.
- Passthrough copies have a separate inventory: `total_files = copied +
  skipped + failed`. Excluded media/sidecars and traversal errors are reported
  separately. An existing destination is skipped only after content hashes
  match, not just file lengths. XMP merge and sidecar-fallback failures prevent
  the main copy from being counted as successful.
- Output count expectations come from terminal delivery outcomes and eligible
  passthrough results. They are not recomputed from a source tree that may have
  been cleaned. An incomplete scan fails closed. Matching counts do not imply
  matching identities or bytes; extra outputs remain an explicit warning.
- Fast IMG receipts must have unique relative paths within each delivery tier
  and unique recorded Photos UUIDs across tiers. Output-root and source-root
  paths are separate domains. Duplicate proof is rejected at marker read,
  write and reporting boundaries, not silently deduplicated.
- Fast GIF reports known outcomes even when probing, conversion or finalization
  fails. With Photos delivery selected, ready encodes without verified receipts
  remain unprocessed; encoded and Photos-verified totals stay separate.
- Integrity issues and infrastructure errors can fail the whole command but
  must not invent failed files or erase already proven successes. Missing or
  malformed child counts remain unknown through the launcher and native GUI.
- Dry-run does not require a media-outcome inventory, but real command errors
  remain errors. Early exits from the standard pipeline still finalize session
  reporting. Child/session log write failures propagate to the caller.
  Log destinations are opened before child work begins; separate output pipes
  are drained concurrently. Malformed final labels invalidate previous values;
  only known report decorations are accepted after a number.
- GUI log backpressure reserves bounded capacity for result/control/error
  records. Critical overflow invalidates the batch result, even when a later
  valid result arrives. Display truncation never substitutes for complete proof.

## Regression Scope

Focused regressions exercise mixed outcomes, partial batches, fail-fast,
source-copy failure, no-attempt rates, scan errors, same-size different-content
copies, immutable output expectations, duplicate receipts, arithmetic overflow,
child report parsing and GUI aggregation. Tests use synthetic records and
temporary directories, not a production Photos library. Live library acceptance
is separate evidence and requires the explicitly authorized debug library.

### Validation Record (2026-10-04)

- Focused foundation/IMG/VID count, copy, progress, receipt and Fast GIF
  regressions passed before integration.
- Dev library (72), launcher (25), smart build (25), workflow contract (1) and
  smoke suite (42) passed after integration; none were ignored.
- The silent-fallback contract suite passed all 396 checks. Strict Clippy for
  foundation, IMG, VID and dev targets and workspace formatting passed locally.
- Native-host self-tests passed in English, Simplified Chinese and Japanese.
  Actual macOS window inspection confirmed full-width form controls and compact
  image/performance settings sheets. All packaged tools, helper, GUI and dylib
  were refreshed; strict bundle signature verification passed.
- No production or debug Photos library was opened by these checks. Live
  custody/cleanup acceptance and the next hosted CI run are not implied by
  these results. OSS-Fuzz's pinned image manifest was verified, not executed.
