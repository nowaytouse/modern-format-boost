# CI execution scope

**Temporary desktop-only policy:** Linux quality, security-audit, and deep-audit jobs are paused, as is the gated automatic nightly release. Workflow validation and the macOS dispatch2 check remain active. The stable publishing job is paused pending macOS SBOM validation; its macOS build remains available. The standalone nightly workflow retains its macOS build and publishing path. GitHub-managed Dependabot and Dependency Graph are unaffected. The execution scopes below describe the retained configuration for when paused jobs are restored; skipped jobs do not pass their gates.

The quality workflow currently runs workflow validation and, when code checks are selected, the macOS dispatch2 check. Linux quality, security-audit and deep-audit jobs are paused. Their skipped results do not pass release gates. GitHub-managed Dependabot and Dependency Graph remain unaffected.

| Trigger | Active checks |
| --- | --- |
| Code push or pull request to `main` / `nightly` | Workflow validation and macOS dispatch2. |
| Monday–Saturday, 03:17 UTC | Workflow validation only. |
| Sunday, 03:17 UTC | Workflow validation and macOS dispatch2; Linux audits and the gated nightly snapshot are paused. |
| Manual run | Workflow validation and macOS dispatch2; Linux audits and the gated nightly snapshot are paused, including when `full_audit` is enabled. |

In Shanghai time, scheduled checks start at 11:17, subject to runner availability. The default branch is `nightly`. Docs-only pushes retain the existing path exclusion; pull requests still receive their checks. The standalone nightly workflow retains its macOS build and publish path. The stable release workflow retains its macOS build, but publishing is paused pending validation of SBOM generation on macOS.

The following paragraphs describe retained Linux configuration for a future resumption; those checks are currently paused.

Formatting runs `cargo fmt --all --check` directly. It does not install media
toolchains or compile the workspace merely to check formatting. Compilation,
tests and audits retain their pinned media dependencies and failure handling.
The existing source-only shared Cargo cache is retained; large cross-lock target
caches previously exhausted runner disk space and are not reintroduced.

The Linux media bootstrapper still builds FFmpeg's rolling development branch,
but fetches it from the [mirror listed by FFmpeg](https://ffmpeg.org/download.html#repositories)
instead of depending on the mutable `ffmpeg.org` snapshot tarball. It resolves
`master` once to a full commit ID, fetches that exact commit with bounded retries,
and verifies the detached checkout before publishing the build source directory.
Failures discard the owned staging directory and stop installation; they do not
select a different commit, an older package, or an unverified archive.

Check and deep-audit diagnostic directories and LCOV output are not uploaded as
artifacts. The runner's console output remains visible in GitHub Actions logs,
so test inputs and diagnostic messages still must not contain private data.
Existing artifacts, published release assets and local user data are unaffected.
Existing branch concurrency continues to cancel superseded push/PR runs.

Validate changes with `actionlint` and the `smoke::structural` tests. Compare
summed job durations, not only elapsed workflow time, when measuring consumption.
Local checks cannot prove hosted runtime savings or establish that unrun weekly
checks passed.
