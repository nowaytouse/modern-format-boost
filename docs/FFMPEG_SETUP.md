# FFmpeg Full-Feature Setup on macOS

This document describes the FFmpeg configuration used by **Modern Format Boost (MFB)** on macOS.

The goal is not to maintain a frozen list of optional libraries. Instead, the setup dynamically enables the **widest currently supported feature set exposed by `homebrew-ffmpeg`**, while excluding only options that:

1. do not add media-processing capabilities;
2. require external proprietary SDKs;
3. are temporarily broken by an upstream compatibility issue; or
4. change the selected FFmpeg release model.

This makes the setup substantially more resistant to future changes in the Homebrew formula.

---

## 1. Installation Strategy

For the full-feature macOS environment described here, use:

```text
homebrew-ffmpeg/ffmpeg/ffmpeg
```

as the single Homebrew-managed FFmpeg implementation.

Do **not** attempt to keep both:

```text
homebrew/core/ffmpeg
```

and:

```text
homebrew-ffmpeg/ffmpeg/ffmpeg
```

installed as interchangeable providers.

Homebrew does not treat a third-party tap formula as a transparent replacement for a dependency declared on the core `ffmpeg` formula.

The tap version therefore owns the normal commands:

```text
ffmpeg
ffprobe
ffplay
```

---

## 2. Why Options Are Generated Dynamically

Do not maintain a permanent command containing dozens of manually written options.

The available feature set changes over time as the formula adds, removes, or renames dependencies.

Instead, obtain the current option list directly from Homebrew:

```bash
brew options homebrew-ffmpeg/ffmpeg/ffmpeg
```

For scripting:

```bash
brew options homebrew-ffmpeg/ffmpeg/ffmpeg --compact
```

The installation command then enables everything except a small explicit blacklist.

This has several advantages:

- new optional features are automatically picked up;
- removed options do not remain stale in this document;
- renamed dependencies require less manual maintenance;
- the actual installed configuration follows the current formula rather than an old snapshot;
- the document does not need to duplicate the formula itself.

---

## 3. Prerequisites

Install and tap the FFmpeg repository:

```bash
brew tap homebrew-ffmpeg/ffmpeg
```

Because this is a third-party Homebrew tap, explicitly trust the FFmpeg formula:

```bash
brew trust --formula homebrew-ffmpeg/ffmpeg/ffmpeg
```

Formula-level trust is preferred over trusting the entire tap when only FFmpeg is required.

Install external prerequisites used by optional FFmpeg features:

```bash
brew install flite
brew install tesseract-lang
```

`flite` is required for FFmpeg's Flite text-to-speech integration.

`tesseract-lang` is not required to compile FFmpeg, but it installs the full Tesseract language dataset instead of the limited default language set.

---

## 4. Full-Feature Installation

### Current blacklist

The full-feature build intentionally excludes:

```text
--with-alt-name
--with-decklink
--with-openapv
--HEAD
```

Their meanings are explained below.

### Install

```bash
brew install homebrew-ffmpeg/ffmpeg/ffmpeg \
  $(brew options homebrew-ffmpeg/ffmpeg/ffmpeg --compact \
    | tr ' ' '\n' \
    | grep -vE '^(--with-alt-name|--with-decklink|--with-openapv|--HEAD)$' \
    | xargs)
```

For an existing installation, use the same option-generation logic with `reinstall`:

```bash
brew reinstall homebrew-ffmpeg/ffmpeg/ffmpeg \
  $(brew options homebrew-ffmpeg/ffmpeg/ffmpeg --compact \
    | tr ' ' '\n' \
    | grep -vE '^(--with-alt-name|--with-decklink|--with-openapv|--HEAD)$' \
    | xargs)
```

This is the normal MFB FFmpeg configuration.

---

## 5. Why These Options Are Excluded

### `--with-alt-name`

This option changes the command names to variants such as:

```text
ffmpeg-alt
ffprobe-alt
ffplay-alt
```

It does not add codecs, filters, demuxers, encoders, decoders, or other media-processing functionality.

It is therefore excluded.

---

### `--with-decklink`

DeckLink support requires the separately distributed **Blackmagic DeckLink SDK**.

This is relevant primarily for professional Blackmagic capture/output hardware and is not part of a normal Homebrew-only installation.

MFB does not require DeckLink.

If DeckLink hardware support is required in the future, install the appropriate Blackmagic SDK first and remove this option from the blacklist.

---

### `--with-openapv`

**Temporary compatibility exclusion — September 2026.**

OpenAPV changed the API signature of:

```c
oapvm_create()
```

from the interface expected by the FFmpeg OpenAPV integration.

With the currently encountered combination:

```text
FFmpeg 9.0.2
OpenAPV 1.1.1.0
```

FFmpeg fails while compiling:

```text
libavcodec/liboapvenc.c
```

with an error similar to:

```text
error: too few arguments to function call, expected 2, have 1
```

This is an upstream compatibility problem, not a macOS, Clang, Apple Silicon, or MFB configuration failure.

The OpenAPV library integration (`liboapv`) is therefore temporarily disabled until FFmpeg and OpenAPV agree on the updated API. This does not imply that every native APV implementation is unavailable.

See **Restoring OpenAPV** below.

---

### `--HEAD`

`--HEAD` selects the latest FFmpeg Git development branch rather than the current packaged release.

A full feature set and a development snapshot are separate concepts.

MFB normally uses the latest stable FFmpeg release exposed by the tap, with the maximum usable feature set enabled.

Therefore `--HEAD` is explicitly excluded from the dynamically generated option list.

Use an FFmpeg HEAD build only when intentionally testing unreleased FFmpeg changes.

---

## 6. Current Observed Configuration

Earlier setup notes recorded FFmpeg 9.0.2. A live inspection on **September 19, 2026** instead found the following installed development build:

```text
FFmpeg N-126655-gbfac54a03b
Homebrew ffmpeg HEAD-bfac54a
Apple Silicon
macOS 27 Golden Gate
homebrew-ffmpeg
```

The stable-release recipe above remains a separate installation policy; it does not describe this HEAD snapshot. The running binary's configuration reports capabilities such as:

```text
libaom
dav1d
SVT-AV1
x264
x265
VideoToolbox
AudioToolbox
JPEG XL
WebP
JPEG 2000
rav1e
OpenH264
OpenVINO
TensorFlow
Tesseract
Whisper
libplacebo
VMAF
zimg
SVG
DVD
Blu-ray
SRT
RIST
Chromaprint
Rubber Band
SoXR
Xvid
AMR
OpenAL
ZeroMQ
```

This list is illustrative rather than authoritative. Configuration and encoder/decoder enumeration establish availability, not successful processing of every media type. The snapshot also lists native `apv` decoding and `apv_vulkan` encoding, but has no `--enable-liboapv`; Vulkan hardware encoding has not been exercised by this audit.

Always inspect the actual installed binary instead of relying on this document as a frozen capability list.

---

## 7. Whisper and GGML

The formula exposes GGML support as part of the machine-learning feature set, while FFmpeg ultimately reports Whisper integration as:

```text
--enable-whisper
```

Do not expect the FFmpeg `configure` output to necessarily contain:

```text
--enable-ggml
```

simply because the Homebrew build option enabled the corresponding dependency.

Verify the actual FFmpeg feature:

```bash
ffmpeg -hide_banner -buildconf | grep whisper
```

Expected output:

```text
--enable-whisper
```

Option names exposed by the Homebrew formula can also change over time.

For that reason, the installation procedure deliberately obtains them dynamically from:

```bash
brew options homebrew-ffmpeg/ffmpeg/ffmpeg --compact
```

rather than hardcoding the Whisper option name in the main installation command.

---

## 8. FDK-AAC

Do not hardcode:

```text
--with-fdk-aac
```

into this setup.

Older versions of this document included it, but it is not part of the currently exposed option set used by this installation.

If the tap adds or restores an optional codec in the future, the dynamic option-generation strategy will pick it up automatically unless it is explicitly blacklisted.

This document should describe the installation policy rather than preserve historical option names indefinitely.

---

## 9. Verify the Installed FFmpeg

Never infer capability from the package name alone.

Inspect the actual binary.

### Build configuration

```bash
ffmpeg -hide_banner -buildconf
```

### Encoders

```bash
ffmpeg -hide_banner -encoders
```

### Decoders

```bash
ffmpeg -hide_banner -decoders
```

### Filters

```bash
ffmpeg -hide_banner -filters
```

### Formats

```bash
ffmpeg -hide_banner -formats
```

### Probe version

```bash
ffprobe -version
```

### Important MFB-related features

```bash
ffmpeg -hide_banner -buildconf \
  | grep -E 'jxl|webp|openvino|tensorflow|whisper|rav1e|vmaf|placebo|tesseract|zimg'
```

A validated build should contain entries including:

```text
--enable-libjxl
--enable-libwebp
--enable-libopenvino
--enable-libtensorflow
--enable-libtesseract
--enable-libvmaf
--enable-libplacebo
--enable-librav1e
--enable-libzimg
--enable-whisper
```

Exact output can change as FFmpeg and the tap evolve.

---

## 10. Verify Dynamic Linking

Check FFmpeg:

```bash
brew linkage --test ffmpeg
```

Check mpv:

```bash
brew linkage --test mpv
```

A report such as:

```text
Indirect dependencies with linkage:
  cairo
  glib
  libpng
  ...
```

does **not** by itself mean the installation is broken.

The important failure condition is missing or unresolved libraries.

For an explicit exit-code check:

```bash
brew linkage --test ffmpeg
echo "ffmpeg linkage exit=$?"

brew linkage --test mpv
echo "mpv linkage exit=$?"
```

A successful linkage check should exit with status:

```text
0
```

The September 19 live check did **not** meet that gate: `brew linkage --test ffmpeg` exited 1 and listed indirect dependencies with linkage, without reporting missing libraries. `ffmpeg -version` exited 0. Record these as separate results; neither a clean linkage check nor a broken media pipeline follows from the startup check alone.

---

## 11. Dependency Upgrades

Optional libraries can occasionally change ABI or library filenames independently of FFmpeg.

Typical examples include machine-learning or image-processing dependencies.

If FFmpeg stops launching after a dependency update, or:

```bash
brew linkage --test ffmpeg
```

reports a missing versioned library, rebuild FFmpeg using the same full-feature installation policy:

```bash
brew reinstall homebrew-ffmpeg/ffmpeg/ffmpeg \
  $(brew options homebrew-ffmpeg/ffmpeg/ffmpeg --compact \
    | tr ' ' '\n' \
    | grep -vE '^(--with-alt-name|--with-decklink|--with-openapv|--HEAD)$' \
    | xargs)
```

Then repeat the verification commands.

Do not fix ABI mismatches by:

- creating fake compatibility symlinks;
- force-linking a different FFmpeg formula;
- keeping core and tap FFmpeg installed as competing providers;
- arbitrarily downgrading dependencies without first identifying the actual compatibility problem.

A clean rebuild against the currently installed libraries is preferred.

---

# mpv and Homebrew Core FFmpeg

## 12. Why mpv Can Conflict With This Setup

Homebrew's core `mpv` formula depends on:

```text
ffmpeg
```

from Homebrew core.

Homebrew does not allow a dependency of a core formula to be transparently replaced by a same-named formula from a third-party tap.

Therefore this state:

```text
homebrew-ffmpeg/ffmpeg/ffmpeg installed
+
brew upgrade mpv
```

can produce an error similar to:

```text
Error: ffmpeg is already installed from homebrew-ffmpeg/ffmpeg!
Please `brew uninstall ffmpeg` first.
```

This does not mean either FFmpeg or mpv is damaged.

It is a Homebrew dependency-provider conflict.

---

## 13. Updating mpv Safely

When mpv requires an update and Homebrew insists on installing core FFmpeg, temporarily bridge through the core dependency.

### Step 1 — Remove the tap FFmpeg

```bash
brew uninstall --ignore-dependencies homebrew-ffmpeg/ffmpeg/ffmpeg
```

### Step 2 — Upgrade mpv

For an mpv HEAD installation:

```bash
brew upgrade mpv --fetch-HEAD
```

Homebrew may temporarily install core FFmpeg as an mpv dependency.

### Step 3 — Remove core FFmpeg

After mpv finishes installing:

```bash
brew uninstall --ignore-dependencies ffmpeg
```

Confirm:

```bash
brew list --versions ffmpeg
```

At this point there should be no core FFmpeg keg remaining.

### Step 4 — Restore the full-feature tap FFmpeg

```bash
brew install homebrew-ffmpeg/ffmpeg/ffmpeg \
  $(brew options homebrew-ffmpeg/ffmpeg/ffmpeg --compact \
    | tr ' ' '\n' \
    | grep -vE '^(--with-alt-name|--with-decklink|--with-openapv|--HEAD)$' \
    | xargs)
```

### Step 5 — Verify both packages

```bash
ffmpeg -version
mpv --version

brew linkage --test ffmpeg
brew linkage --test mpv
```

---

## 14. Topgrade

Normal Homebrew upgrades can be run through Topgrade.

However, when Topgrade reaches an mpv update, the Homebrew dependency conflict described above can reappear.

Typical symptoms are:

```text
Error: ffmpeg is already installed from homebrew-ffmpeg/ffmpeg!
```

or Homebrew attempting to install:

```text
homebrew/core/ffmpeg
```

while the tap FFmpeg is already installed.

Do **not** repeatedly retry the same Topgrade step.

Exit the failed Homebrew upgrade and perform the manual mpv bridge procedure from the previous section.

After the tap FFmpeg has been restored and linkage checks pass, normal package maintenance can continue.

---

# OpenAPV Recovery

## 15. Restoring OpenAPV After the Upstream Fix

OpenAPV should not remain permanently disabled.

Periodically check whether the FFmpeg/OpenAPV compatibility issue has been resolved.

Once the current OpenAPV release successfully builds with the current FFmpeg release, remove only:

```text
--with-openapv
```

from the blacklist.

The normal blacklist then becomes:

```text
--with-alt-name
--with-decklink
--HEAD
```

Rebuild:

```bash
brew reinstall homebrew-ffmpeg/ffmpeg/ffmpeg \
  $(brew options homebrew-ffmpeg/ffmpeg/ffmpeg --compact \
    | tr ' ' '\n' \
    | grep -vE '^(--with-alt-name|--with-decklink|--HEAD)$' \
    | xargs)
```

Verify:

```bash
ffmpeg -hide_banner -buildconf | grep -i oapv
```

Only after the build succeeds should this document remove the OpenAPV compatibility warning.

---

# Maintenance Model

## 16. What This Document Should Track

This document should track:

- the installation strategy;
- intentional exclusions;
- known compatibility exceptions;
- package-manager conflicts;
- verification procedures;
- recovery procedures.

It should **not** attempt to permanently mirror every optional dependency in the Homebrew formula.

The formula itself is the authoritative source for the current option list:

```bash
brew options homebrew-ffmpeg/ffmpeg/ffmpeg
```

---

## 17. Expected Long-Term State

Normal target:

```text
homebrew-ffmpeg/ffmpeg/ffmpeg
├── all currently usable Homebrew options
├── stable FFmpeg release
├── full Tesseract language data
│
├── alt-name     excluded: no additional capability
├── DeckLink     excluded: external Blackmagic SDK required
├── HEAD         excluded: development release, not a feature
└── OpenAPV      temporary exclusion until upstream compatibility is restored
```

After the OpenAPV issue is resolved:

```text
homebrew-ffmpeg/ffmpeg/ffmpeg
├── all currently usable Homebrew options
├── OpenAPV enabled
├── stable FFmpeg release
├── full Tesseract language data
│
├── alt-name     excluded
├── DeckLink     excluded unless Blackmagic hardware support is required
└── HEAD         excluded unless explicitly testing FFmpeg master
```

That state represents the intended **maximum practical Homebrew feature set** for MFB.

---

## 18. Quick Health Check

For routine maintenance:

```bash
ffmpeg -version
ffprobe -version
mpv --version

ffmpeg -hide_banner -buildconf \
  | grep -E 'jxl|webp|openvino|tensorflow|whisper|rav1e|vmaf|placebo|tesseract|zimg'

brew linkage --test ffmpeg
brew linkage --test mpv
```

For a deeper audit:

```bash
ffmpeg -hide_banner -buildconf
ffmpeg -hide_banner -encoders
ffmpeg -hide_banner -decoders
ffmpeg -hide_banner -filters
ffmpeg -hide_banner -formats
```

---

## 19. Current Known Exception Summary

As of **2026-09-19**:

| Component | Status | Notes |
|---|---|---|
| FFmpeg HEAD-bfac54a | ✅ Starts successfully | Actual installed tap snapshot; media-path tests are separate |
| JPEG XL | ✅ Enabled | `libjxl` |
| WebP | ✅ Enabled | `libwebp` |
| AV1 | ✅ Enabled | Multiple implementations available |
| OpenVINO | ✅ Enabled | DNN backend |
| TensorFlow | ✅ Enabled | DNN backend |
| Whisper | ✅ Enabled | Speech recognition |
| Tesseract | ✅ Enabled | Full language data installed separately |
| VMAF | ✅ Enabled | Video quality analysis |
| VideoToolbox | ✅ Enabled | Apple hardware acceleration |
| AudioToolbox | ✅ Enabled | Apple audio framework |
| mpv HEAD | ✅ Working | May require the core-FFmpeg bridge during upgrades |
| OpenAPV library (`liboapv`) | ⏸ Not enabled | Stable-build API incompatibility; native APV entries are listed separately |
| DeckLink | ⏭ Not installed | Requires Blackmagic DeckLink SDK |
| FFmpeg stable-release recipe | Documented policy | Not the currently installed HEAD build |
| Homebrew FFmpeg linkage audit | ⚠ Exit 1 | Indirect-dependency findings; no missing libraries reported |

---

## 20. Design Principle

The installation policy can be summarized as:

> Enable everything the current Homebrew FFmpeg formula can reliably build, and maintain only a small explicit list of justified exceptions.

This is preferable to maintaining a historical list of individual codecs and libraries because the Homebrew formula, FFmpeg, and their dependencies evolve independently.
