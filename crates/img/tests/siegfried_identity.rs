//! Siegfried + PRONOM external identification regression (tool-gated).
//!
//! Follows the repo convention: skips with an explicit stderr note when `sf`
//! is unavailable. When present, verifies the identity layer end to end —
//! content-based identification, extension-mismatch diagnostics, zero-match
//! honesty, and the batch contract (one invocation for many paths).

use foundation::format_detect::FormatKind;
use foundation::format_identity::{
    DetectionSource, FormatIdentity, SupportLevel, resolve_format_identity, support_level,
};
use foundation::siegfried::{SiegfriedProbe, identify_paths, siegfried_available};
use std::process::{Command, Stdio};

const ONE_BY_ONE_RGBA_PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4,
    0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE,
    0x42, 0x60, 0x82,
];

fn sf_available() -> bool {
    if !siegfried_available() {
        return false;
    }
    matches!(
        Command::new("sf")
            .arg("-version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status(),
        Ok(status) if status.success()
    )
}

#[test]
fn content_wins_over_misleading_extensions() -> anyhow::Result<()> {
    if !sf_available() {
        eprintln!("Skipping siegfried identity test: sf is unavailable");
        return Ok(());
    }
    let dir = tempfile::tempdir()?;

    // PNG bytes behind a .jpg name: content identifies, mismatch recorded.
    let masquerade = dir.path().join("actually_png.jpg");
    std::fs::write(&masquerade, ONE_BY_ONE_RGBA_PNG)?;
    let identity = resolve_format_identity(&masquerade)?;
    assert_eq!(identity.family, FormatKind::Png, "content must win");
    assert!(identity.extension_mismatch);
    assert_eq!(identity.source, DetectionSource::Combined);
    let pronom = identity
        .pronom
        .first()
        .ok_or_else(|| anyhow::anyhow!("PRONOM evidence expected for mismatch case"))?;
    assert_eq!(pronom.puid, "fmt/11");
    assert_eq!(pronom.mime, "image/png");

    // No extension at all: still identified by content alone.
    let bare = dir.path().join("no_extension");
    std::fs::write(&bare, ONE_BY_ONE_RGBA_PNG)?;
    let identity = resolve_format_identity(&bare)?;
    assert_eq!(identity.family, FormatKind::Png);
    assert!(identity.extension_hint.is_none());
    assert!(!identity.extension_mismatch);

    Ok(())
}

#[test]
fn garbage_and_missing_entries_stay_honest() -> anyhow::Result<()> {
    if !sf_available() {
        eprintln!("Skipping siegfried identity test: sf is unavailable");
        return Ok(());
    }
    let dir = tempfile::tempdir()?;

    // Random-looking bytes: PRONOM's catch-all may label them "Binary File"
    // (fmt/208), which is an external identity — but the internal family
    // must stay Unknown and the file must never look processable.
    let garbage = dir.path().join("mystery.bin");
    std::fs::write(&garbage, [0xA5u8; 64])?;
    let identity = resolve_format_identity(&garbage)?;
    assert_eq!(identity.family, FormatKind::Unknown);
    assert!(
        matches!(
            support_level(&identity),
            SupportLevel::DetectOnly | SupportLevel::Unknown
        ),
        "garbage must never be FullySupported or Unsupported-video; got {:?}",
        support_level(&identity)
    );

    // A path sf cannot report on (nonexistent file) surfaces as a per-file
    // error entry, never as a silent drop or a panic.
    let missing = dir.path().join("does_not_exist.bin");
    let probe = identify_paths(std::slice::from_ref(&missing))?;
    let SiegfriedProbe::Identified { files, .. } = probe else {
        anyhow::bail!("identify_paths must succeed for a missing-file batch");
    };
    let entry = files
        .iter()
        .find(|file| file.filename == missing.to_string_lossy())
        .ok_or_else(|| anyhow::anyhow!("missing file must have a report entry"))?;
    anyhow::ensure!(!entry.errors.is_empty(), "missing file must carry an error");

    Ok(())
}

#[test]
fn batch_identification_preserves_every_file() -> anyhow::Result<()> {
    if !sf_available() {
        eprintln!("Skipping siegfried identity test: sf is unavailable");
        return Ok(());
    }
    let dir = tempfile::tempdir()?;
    let mut paths = Vec::new();
    for index in 0..3 {
        let path = dir.path().join(format!("copy{index}.png"));
        std::fs::write(&path, ONE_BY_ONE_RGBA_PNG)?;
        paths.push(path);
    }

    let SiegfriedProbe::Identified { files, .. } = identify_paths(&paths)? else {
        anyhow::bail!("identify_paths must succeed for the batch");
    };
    for path in &paths {
        let entry = files
            .iter()
            .find(|file| file.filename == path.to_string_lossy())
            .ok_or_else(|| anyhow::anyhow!("batch dropped {}", path.display()))?;
        anyhow::ensure!(
            entry.matches.iter().any(|m| m.id == "fmt/11"),
            "PNG batch member must identify as fmt/11"
        );
    }

    // Identity model stays consistent for a plain supported file.
    let identity: FormatIdentity = resolve_format_identity(&paths[0])?;
    assert_eq!(support_level(&identity), SupportLevel::FullySupported);
    Ok(())
}
