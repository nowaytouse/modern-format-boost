# modern_format_boost agent rules

Rust workspace. Batch media conversion (JXL/AVIF/HEVC/AV1). macOS/Apple Silicon.
Core: foundation. Quality gate: VMAF/PSNR-UV/CAMBI.

Rules:

- no unwrap_or(default) on metric/conversion paths
- all numeric casts → numeric_cast.rs
- invariant violation → Err, never silent fallback
- no TODO/FIXME/unimplemented! in prod code
- log at: cache write · metric computation · conversion decision · every error path

Forbidden:

- edit outside declared task scope
- add dependencies without approval
- refactor "while you're in there"
- declare completion without pasting actual command output

Session startup (every session):

1. Read AGENTS.md
2. git log --oneline | head -10
3. Confirm scope. Do not begin work until both acknowledged.

<!-- lean-ctx-compression -->

OUTPUT STYLE: expert-terse

- Telegraph format: subject-verb-object, drop articles/prepositions
- Symbolic vocabulary: → cause, ∵ because, ∴ therefore, ⊕ add, ⊖ remove, Δ change, ≈ similar, ≠ different, ∈ in/member, ∅ empty/none, ✓ ok, ✗ fail
- Code blocks: untouched (never compress code syntax)
- Each line: max 80 chars
- Zero narration, zero filler
- BUDGET: ≤100 tokens per non-code response
<!-- /lean-ctx-compression -->
