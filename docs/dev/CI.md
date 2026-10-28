# CI execution scope

The quality workflow separates feedback on each commit from periodic full audits.
Every job reports its actual scope; a skipped audit is not evidence that it passed.

| Trigger | Checks |
| --- | --- |
| Code push or pull request to `main` / `nightly` | Workflow validation, standalone Rust formatting, IMG/VID package checks, workspace compilation, strict Clippy, workspace CI tests, both dispatch2 platform checks and dependency advisories. |
| Monday–Saturday, 03:17 UTC | Workflow validation and current dependency advisories, without repeating code compilation for the same commit. |
| Sunday, 03:17 UTC | All commit checks plus the existing shared audit, coverage and Deep Production Audit. A nightly snapshot is published only after all release gates pass. |
| Manual run with `full_audit` enabled (default) | Same full checks and gated nightly publication. Disable this input to run commit checks without publishing. |

In Shanghai time, the scheduled checks start at 11:17, subject to GitHub runner
availability. The default branch is `nightly`. Docs-only pushes retain the existing
path exclusion; pull requests still receive their checks.

Automatic nightly snapshots are now weekly rather than one macOS release build
per push. Use **Continuous Quality & Fuzzing → Run workflow → full_audit** when a
fresh fully checked snapshot is needed sooner. Dispatching the standalone release
workflow remains a separate existing entry point.

Formatting runs `cargo fmt --all --check` directly. It does not install media
toolchains or compile the workspace merely to check formatting. Compilation,
tests and audits retain their pinned media dependencies and failure handling.
The existing source-only shared Cargo cache is retained; large cross-lock target
caches previously exhausted runner disk space and are not reintroduced.

Check and deep-audit diagnostics expire after 14 days; coverage artifacts expire
after 7 days. This does not remove published release assets or local user data.
Existing branch concurrency continues to cancel superseded push/PR runs.

Validate changes with `actionlint` and the `smoke::structural` tests. Compare
summed job durations, not only elapsed workflow time, when measuring consumption.
Local checks cannot prove hosted runtime savings or establish that unrun weekly
checks passed.
