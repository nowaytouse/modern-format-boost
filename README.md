# Modern Format Boost

<img src="docs/assets/mfb-logo.png" alt="Modern Format Boost" width="128" height="128">

**A private media-archiving tool: optimize storage, preserve what matters, and verify the result.**

[English](README.md) · [简体中文](docs/README_ZH.md) · [Changelog](docs/CHANGELOG.md)

Modern Format Boost (MFB) applies a personal archival policy to images, video and
animation. It is not a general-purpose format converter or a media library manager.
Smaller files are useful only when the required quality, metadata and delivery
checks pass. When safety cannot be established, MFB retains the affected source
and reports the problem instead of treating it as a successful conversion.

The macOS interface is a native AppKit application. The Rust command-line tools
also have Linux and Windows support; Apple Photos integration is macOS-only.
Development currently targets **0.12.0** on the `nightly` branch.

## Choose A Workflow

| Workflow | What it does | Best fit |
| --- | --- | --- |
| **Images** / `img run` | Applies the full static-image policy, normally delivering JXL. Preserves existing modern lossy formats when another encode is inappropriate. | General image archiving. |
| **Fast IMG, JXL** / `img fast-img --strategy jxl` | Reversibly transcodes real JPEGs to JXL. Eligible modern lossy stills can use a separate verified Photos-delivery path. | Focused JPEG archiving and Photos delivery. |
| **Fast IMG, AVIF** / `img fast-img --strategy avif` | Uses a separate AVIF size/quality policy. Existing AVIF is not re-encoded. | Meme/image collections whose policy permits metadata cleanup. |
| **Videos** / `vid run` | Handles video and animation, with HEVC by default and optional AV1. | Time-based media, not static-image conversion. |

MFB identifies media from its contents, not just its extension. Images and videos
have separate owners: `img` does not silently hand an unsuitable file to `vid`.
Fast IMG is a narrower workflow, not a promise that every input finishes faster.

### Understand The Archive Policy

- JPEG-to-JXL archiving must reconstruct the original JPEG **byte for byte**.
  Its default search compares all eleven JPEG XL effort levels, E1 through E11,
  and reuses the smallest verified complete file. Higher E is not always smaller.
- Genuine lossless images remain lossless under the private IMG policy. Other
  eligible routes can search for a result that satisfies their quality and size
  requirements. Selecting a setting does not authorize a different media policy.
- Normal JXL archiving preserves required metadata and relationships. Fast AVIF
  has a **different metadata-cleanup policy**; it is not a substitute for that
  archival guarantee.

The [private IMG policy](docs/dev/config/IMG_PRIVATE_POLICY.md) explains the
routes and searches. The application's **?** button opens an in-app processing
guide with examples.

## Start Safely

**Keep an independent backup and try a small disposable copy first.** MFB can
delete verified source files in cleanup workflows. In particular, Fast IMG's
verified source cleanup is part of the workflow, not an opt-in enabled by
`--delete-source`. A completed encode alone never authorizes deletion.

### macOS App

1. Open `Modern Format Boost.app` and select or drop a file/folder.
2. Choose the matching image or video workflow. Open **Settings** to review its
   own quality/search choices, tool configuration and Photos preferences.
3. Use **Preview** before a first run. Enable **Import into Photos** only when
   delivery to the currently selected library is intended.
4. Review the final summary. **Processing History** provides recorded counts,
   paths, verification results, cleanup details and diagnostics without hunting
   through log folders.

Pause and Stop cooperate with the backend at safe boundaries; they do not freeze
an encoder midway through its work. An interrupted task is not a completed task.

The GUI supports English, Simplified Chinese and Japanese, plus system, light
and dark appearance. Linux and Windows currently use the CLI.

### Get Or Build The Tools

Check [Releases](https://github.com/nowaytouse/modern-format-boost/releases) for
the actual assets and their revision. Current release workflows publish
**macOS Apple Silicon CLI archives**, not a universal installer or a packaged GUI
for every platform. A nightly build is a development snapshot.

MFB invokes external media tools. Common requirements are FFmpeg/ffprobe,
JPEG XL (`cjxl`, `djxl`, `jxlinfo`), ExifTool, ImageMagick, WebP tools and libheif.
Source builds need the repository's pinned Rust nightly and **libheif 1.23.5 or
newer**; selected formats and features can require additional tools.

For example, install the common macOS dependencies with Homebrew:

```bash
brew install ffmpeg jpeg-xl exiftool imagemagick webp libheif pkgconf
```

Then build the CLI from source:

```bash
git clone https://github.com/nowaytouse/modern-format-boost.git
cd modern-format-boost
cargo build --locked --release -p img -p vid
```

On macOS, the native application builder is:

```bash
cargo run --locked -p dev --bin smart_build -- --all
```

It builds the packaged tools, assembles the app under
`target/release/bundle/macos/`, and replaces the repository-root
`Modern Format Boost.app`. **Do not run it while that app is processing media.**
Native builds require Apple's command-line development tools.

For Linux/Windows dependency setup, consult the
[media dependency installer](crates/dev/src/bin/install_media_dependencies.rs)
and [build workflows](.github/workflows). The GUI's **Inspect Processing Tools**
checks selected paths and version output; it is not a codec or delivery test.

### CLI Examples

These examples assume `img` and `vid` are on `PATH`; after a source build, use
`target/release/img` or `target/release/vid` instead.

```bash
# General static images, with a separate output and no source-deletion flag.
img run /path/to/copied-images --output /path/to/image-output

# Inspect Fast IMG's planned work without converting or deleting media.
img fast-img --strategy jxl --dry-run /path/to/copied-images

# Video/animation; use --codec av1 when that delivery policy is wanted.
vid run /path/to/copied-videos

# Inspect the effective configuration, or check a selected tool.
img config show --effective
img config validate
img config tool cjxl
vid config show --effective
```

Read `img --help`, `img run --help`, `img fast-img --help` or `vid run --help`
for the current command's options. Do not reuse one mode's flags blindly in
another mode. Standard `vid run` requires a reachable PostgreSQL database;
database-backed analysis/training also uses pgvector. `vid config`, `vid fast-gif`,
ordinary local IMG detection and Fast IMG do **not** require PostgreSQL.

## Settings, Failures And Recovery

GUI and CLI use the same configuration rules. Normal IMG, Fast IMG and VID
retain separate applicable settings. Use the [runtime configuration guide](docs/dev/config/RUNTIME_CONFIG.md)
for file locations, precedence, effective values and all supported parameters.

The default **record and continue** policy records recoverable file failures,
retains their originals and continues independent work. **Fail fast** stops
further work sooner; it does not undo already verified results. Shared safety
failures still block affected delivery. Failed files never become “skipped” to
make the summary look successful.

**Partially completed** means independently verified work succeeded while some
files failed. It is not an all-success result. Missing or conflicting evidence
remains unconfirmed. Encoding counts, verified Photos assets and cleanup counts
describe different facts; equal totals do not prove matching contents.

Fast IMG checks matching saved state before allocating a new output directory.
Reuse requires valid identities and checkpoints; starting fresh is a separate
choice. Do not manually delete `_optimized` folders, checkpoints or leftovers
solely because encoding finished. Check the recorded delivery and cleanup facts.

**Photos verification is local delivery evidence, not proof of iCloud sync.**
Unknown or changed outputs are retained rather than silently removed. Runtime
state and scratch are kept outside source trees, but filesystem ctime and dates
affected by unrelated external changes cannot be promised unchanged.

## Documentation

| Need | Read |
| --- | --- |
| Version changes and development history | [Changelog](docs/CHANGELOG.md) |
| Version/phase history and the original commit archive | [Git history](docs/dev/HISTORY.md) |
| Settings, configuration files, tools and failure policy | [Runtime configuration](docs/dev/config/RUNTIME_CONFIG.md) |
| Image routes, JPEG effort search and quality searches | [Private IMG policy](docs/dev/config/IMG_PRIVATE_POLICY.md) |
| Counts, partial results and verification scope | [Counting contract](docs/hardening/COUNTING_CONTRACT.md) |
| Persistent logs, history and diagnostic locations | [Logging layout](docs/hardening/LOGGING_LAYOUT.md) |
| Exact JPEG restoration and XMP relationships | [JXL/XMP archive contract](docs/hardening/JXL_XMP_ARCHIVE_CONTRACT.md) |
| Advanced FFmpeg setup | [FFmpeg guide](docs/FFMPEG_SETUP.md) |
| Architecture and extension boundaries | [Policy readiness](docs/hardening/POLICY_READINESS.md) · [Media conversion contract](docs/hardening/MEDIA_CONVERSION_LAYER_CONTRACT.md) |

## Development And License

The main components are `crates/img`, `crates/vid`, shared `crates/foundation`,
Rust engineering tools in `crates/dev`, and native GUI code in
`crates/gui/src-macos`. Changes should preserve the relevant contracts and use
focused regressions. [CI](.github/workflows/ci-quality.yml) checks a particular
revision; it does not certify an arbitrary media batch or live Photos behavior.

Training/analysis development uses the Rust entry point
`cargo run --locked -p dev --bin run_training --`; it is not required for normal
image archiving. Shared rules live in
[`training_rules.json`](crates/dev/src/config/training_rules.json); machine-specific
paths and ingest overrides belong in the gitignored sibling `training_rules.local.json`.
Append `--help` for current options, or `--background` to detach training; logs use
the [unified log layout](docs/hardening/LOGGING_LAYOUT.md).
See [Python/Rust migration](docs/PYTHON_RUST_MIGRATION.md#canonical-commands)
for canonical commands and the remaining Python roles. Contributions should keep
private media, libraries, local paths, databases and credentials out of commits.

MFB is [MIT licensed](LICENSE). External encoders and dependencies retain their
own licenses; see [dependency notices](docs/LICENSES.json). It is an actively
developed private tool, not a replacement for a backup or a promise of universal
format compatibility.
