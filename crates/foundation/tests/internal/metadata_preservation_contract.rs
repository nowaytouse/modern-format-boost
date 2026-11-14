// CONTRACT: M23 metadata preservation stack (metadata/mod.rs + platform modules).

use super::{
    AaeSidecarAction, MetadataLayerOutcome, XATTR_PRESERVE_SKIP_KEYS, aae_sidecar_destination,
    delivery_policy::{
        exiftool_combined_output_indicates_no_source_tags, is_metadata_delivery_soft_error,
        is_xattr_api_absence,
    },
    exiv2_xmp_fallback_allowed, find_aae_sidecar, find_xmp_sidecar, handle_aae_sidecar,
    is_xattr_preserve_skipped, merge_xmp_sidecar_into_dest, preserve_for_delivery,
    preserve_pro_delivery_layer_order, should_preserve_xattr, verify_exact_metadata_copy,
};
#[cfg(target_os = "macos")]
use super::{
    MetadataDeliveryReport, XATTR_MACOS_EXPLICIT_KEYS, XATTR_MACOS_METADATA_PREFIXES,
    reapply_macos_exact_copy_xattrs_for_delivery, should_copy_macos_extended_xattr,
};
use std::fs;
use tempfile::TempDir;

#[cfg(unix)]
#[test]
fn final_overlay_version_probe_is_uncached() -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    const CHILD: &str = "MFB_TEST_FINAL_OVERLAY_VERSION_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe()?)
            .args(["--exact", "metadata::metadata_preservation_contract::final_overlay_version_probe_is_uncached", "--test-threads=1"])
            .env(CHILD, "1")
            .output()?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        anyhow::ensure!(output.status.success() && stdout.contains("1 passed"),
            "final overlay version child failed: {stdout}\n{}", String::from_utf8_lossy(&output.stderr));
        return Ok(());
    }
    let root = TempDir::new()?;
    let tool = root.path().join("djxl");
    let version = root.path().join("djxl.value");
    fs::write(&tool, "#!/bin/sh\ncat \"$0.value\"\n")?;
    fs::set_permissions(&tool, fs::Permissions::from_mode(0o755))?;
    let _tool = crate::common_utils::EnvGuard::set("MFB_TOOL_DJXL", &tool.to_string_lossy());
    fs::write(&version, "audit-version-one\n")?;
    assert_eq!(super::djxl_version_for_overlay_audit(), "audit-version-one");
    fs::write(&version, "audit-version-two\n")?;
    assert_eq!(super::djxl_version_for_overlay_audit(), "audit-version-one");
    {
        let _fresh = crate::common_utils::UncachedVerificationScope::new()?;
        assert_eq!(super::djxl_version_for_overlay_audit(), "audit-version-two");
        fs::write(&version, "audit-version-three\n")?;
        assert_eq!(super::djxl_version_for_overlay_audit(), "audit-version-three");
    }
    assert_eq!(super::djxl_version_for_overlay_audit(), "audit-version-one",
        "final audit wrote a reusable version result");
    Ok(())
}

#[test]
fn contract_preserve_pro_timestamps_last_locked() {
    let layers = preserve_pro_delivery_layer_order();
    assert!(
        layers.last() == Some(&"timestamps"),
        "CONTRACT: timestamps must be the final preserve_pro layer"
    );
    let exif_pos = layers
        .iter()
        .position(|layer| *layer == "exif_internal")
        .expect("CONTRACT: preserve_pro must include exif_internal");
    let ts_pos = layers
        .iter()
        .position(|layer| *layer == "timestamps")
        .expect("CONTRACT: preserve_pro must include timestamps");
    assert!(
        exif_pos < ts_pos,
        "CONTRACT: exif_internal must run before timestamps (ExifTool rewrites file)"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn contract_preserve_pro_macos_includes_network_xattr_layer() {
    let layers = preserve_pro_delivery_layer_order();
    assert!(
        layers.contains(&"network_xattr"),
        "CONTRACT: macOS preserve_pro must include network xattr layer"
    );
    assert!(
        layers.contains(&"supplemental_xattr"),
        "CONTRACT: macOS preserve_pro must include supplemental xattr layer"
    );
    let net = layers
        .iter()
        .position(|l| *l == "network_xattr")
        .expect("network_xattr");
    let sup = layers
        .iter()
        .position(|l| *l == "supplemental_xattr")
        .expect("supplemental_xattr");
    assert!(net < sup, "CONTRACT: network before supplemental xattr");
}

#[cfg(target_os = "macos")]
#[test]
fn contract_preserve_pro_macos_reapplies_exact_copy_xattrs_after_mutations() {
    let layers = preserve_pro_delivery_layer_order();
    let exact_copy = layers
        .iter()
        .position(|l| *l == "exact_copy_xattr_reapply")
        .expect("CONTRACT: exact-copy xattrs must be replayed after file-mutating metadata steps");
    let spotlight_date = layers
        .iter()
        .position(|l| *l == "spotlight_content_creation_date")
        .expect("spotlight_content_creation_date");
    let ts = layers
        .iter()
        .position(|l| *l == "timestamps")
        .expect("timestamps");

    assert!(
        spotlight_date < exact_copy && exact_copy < ts,
        "CONTRACT: exact-copy xattr replay must happen after ExifTool/Spotlight mutations and before final timestamps"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn contract_preserve_pro_macos_includes_spotlight_content_creation_date_layer() {
    let layers = preserve_pro_delivery_layer_order();
    assert!(
        layers.contains(&"spotlight_content_creation_date"),
        "CONTRACT: macOS preserve_pro must expose the source content creation date for JXL Spotlight indexing"
    );
    let sup = layers
        .iter()
        .position(|l| *l == "supplemental_xattr")
        .expect("supplemental_xattr");
    let spotlight_date = layers
        .iter()
        .position(|l| *l == "spotlight_content_creation_date")
        .expect("spotlight_content_creation_date");
    let ts = layers
        .iter()
        .position(|l| *l == "timestamps")
        .expect("timestamps");
    assert!(
        sup < spotlight_date && spotlight_date < ts,
        "CONTRACT: source-resolved content date must override copied xattrs before final timestamp restoration"
    );
}

#[test]
fn contract_xattr_skip_list_locked() {
    assert!(is_xattr_preserve_skipped("com.apple.quarantine"));
    assert!(is_xattr_preserve_skipped("com.apple.decmpfs"));
    assert!(!should_preserve_xattr("com.apple.quarantine"));
    assert!(should_preserve_xattr("user.custom"));
    assert!(
        XATTR_PRESERVE_SKIP_KEYS.contains(&"com.apple.quarantine"),
        "CONTRACT: quarantine documented in skip list"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn contract_macos_spotlight_xattr_policy_locked() {
    assert!(
        should_copy_macos_extended_xattr("com.apple.metadata:kMDItemFinderComment"),
        "CONTRACT: Spotlight prefix keys must copy"
    );
    assert!(
        should_copy_macos_extended_xattr("com.apple.FinderInfo"),
        "CONTRACT: explicit FinderInfo must copy"
    );
    assert!(
        !should_copy_macos_extended_xattr("com.apple.quarantine"),
        "CONTRACT: quarantine must never copy"
    );
    assert!(
        !should_copy_macos_extended_xattr("user.namespace"),
        "CONTRACT: supplemental pass owns non-Spotlight keys"
    );
    assert_ne!(XATTR_MACOS_METADATA_PREFIXES.len(), 0);
    assert_ne!(XATTR_MACOS_EXPLICIT_KEYS.len(), 0);
}

#[cfg(target_os = "macos")]
#[test]
fn contract_network_xattr_quarantine_never_copied() {
    use super::{NETWORK_XATTR_PRIORITY_KEYS, XATTR_PRESERVE_SKIP_KEYS};
    assert!(
        !NETWORK_XATTR_PRIORITY_KEYS.contains(&"com.apple.quarantine"),
        "CONTRACT: quarantine must not be in priority list"
    );
    assert!(
        XATTR_PRESERVE_SKIP_KEYS.contains(&"com.apple.quarantine"),
        "CONTRACT: quarantine must be documented as skipped"
    );
}

#[test]
fn contract_find_xmp_sidecar_adjacent_extension_locked() {
    let temp = TempDir::new().expect("tempdir");
    let src = temp.path().join("photo.jpg");
    let sidecar = temp.path().join("photo.jpg.xmp");
    fs::write(&src, b"jpg").expect("write src");
    fs::write(&sidecar, b"xmp").expect("write sidecar");
    assert_eq!(
        find_xmp_sidecar(&src).unwrap().as_deref(),
        Some(sidecar.as_path()),
        "CONTRACT: must resolve adjacent .ext.xmp sidecar"
    );
}

#[test]
fn contract_find_xmp_sidecar_stem_only_locked() {
    let temp = TempDir::new().expect("tempdir");
    let src = temp.path().join("photo.heic");
    let sidecar = temp.path().join("photo.xmp");
    fs::write(&src, b"heic").expect("write src");
    fs::write(&sidecar, b"xmp").expect("write sidecar");
    assert_eq!(
        find_xmp_sidecar(&src).unwrap().as_deref(),
        Some(sidecar.as_path()),
        "CONTRACT: must resolve stem.xmp sidecar"
    );
}

#[test]
fn contract_find_xmp_sidecar_compound_stem_locked() {
    let temp = TempDir::new().expect("tempdir");
    let src = temp.path().join("IMG_0001.HEIC");
    let sidecar = temp.path().join("img_0001.heic.xmp");
    fs::write(&src, b"heic").expect("write src");
    fs::write(&sidecar, b"xmp").expect("write sidecar");
    let found = find_xmp_sidecar(&src)
        .unwrap()
        .expect("CONTRACT: compound stem sidecar must resolve");
    assert_eq!(
        found
            .file_name()
            .map(|n| n.to_string_lossy().to_lowercase()),
        sidecar
            .file_name()
            .map(|n| n.to_string_lossy().to_lowercase()),
        "CONTRACT: must resolve case-insensitive compound stem sidecar"
    );
}

#[test]
fn contract_find_xmp_sidecar_uppercase_xmp_extension_locked() {
    let temp = TempDir::new().expect("tempdir");
    let src = temp.path().join("photo.heic");
    let sidecar = temp.path().join("photo.XMP");
    fs::write(&src, b"heic").expect("write src");
    fs::write(&sidecar, b"xmp").expect("write sidecar");
    let found = find_xmp_sidecar(&src)
        .unwrap()
        .expect("CONTRACT: stem.XMP sidecar must resolve");
    assert_eq!(
        found
            .file_name()
            .map(|n| n.to_string_lossy().to_lowercase()),
        sidecar
            .file_name()
            .map(|n| n.to_string_lossy().to_lowercase()),
        "CONTRACT: must resolve stem.XMP sidecar"
    );
}

#[test]
fn contract_find_xmp_sidecar_dng_compound_extension_locked() {
    let temp = TempDir::new().expect("tempdir");
    let src = temp.path().join("RAW_0001.DNG");
    let sidecar = temp.path().join("RAW_0001.DNG.XMP");
    fs::write(&src, b"dng").expect("write src");
    fs::write(&sidecar, b"xmp").expect("write sidecar");

    assert_eq!(
        find_xmp_sidecar(&src).unwrap().as_deref(),
        Some(sidecar.as_path()),
        "CONTRACT: a unique DNG full-extension XMP sidecar must resolve"
    );
}

#[cfg(unix)]
#[test]
fn contract_find_xmp_sidecar_name_matching_does_not_guess_non_utf8() {
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    // Exercise the actual selector without requiring the filesystem to accept
    // invalid UTF-8 names; APFS rejects creating such files with EILSEQ.
    let invalid_name = Path::new(std::ffi::OsStr::from_bytes(b"photo\xff.xmp"));
    assert!(!super::xmp_candidate_matches_source(
        invalid_name,
        "photo",
        "photo.jpg"
    ));
    for name in ["PHOTO.XMP", "photo.jpg.xmp"] {
        assert!(super::xmp_candidate_matches_source(
            Path::new(name),
            "photo",
            "photo.jpg"
        ));
    }
    assert!(!super::xmp_candidate_matches_source(
        Path::new("unrelated.xmp"),
        "photo",
        "photo.jpg"
    ));
}

// Filesystem integration for Unix filesystems that can represent these names.
// The matching predicate above remains exercised on macOS.
#[cfg(all(unix, not(target_os = "macos")))]
#[test]
fn contract_find_xmp_sidecar_ignores_unrelated_non_utf8_names() {
    use std::os::unix::ffi::OsStrExt;

    let temp = TempDir::new().expect("tempdir");
    let src = temp.path().join("photo.jpg");
    let unrelated = temp
        .path()
        .join(std::ffi::OsStr::from_bytes(b"photo\xff.xmp"));
    fs::write(&src, b"source").expect("write source");
    fs::write(&unrelated, b"unrelated metadata").expect("write unrelated sidecar");
    assert!(find_xmp_sidecar(&src).unwrap().is_none());

    let sidecar = temp.path().join("photo.xmp");
    fs::write(&sidecar, b"matching metadata").expect("write matching sidecar");
    assert_eq!(
        find_xmp_sidecar(&src).unwrap().as_deref(),
        Some(sidecar.as_path())
    );

    let compound = temp.path().join("photo.jpg.xmp");
    fs::write(&compound, b"conflicting metadata").expect("write conflicting sidecar");
    let error = find_xmp_sidecar(&src).expect_err("matching ambiguity must still be refused");
    assert!(error.to_string().contains("ambiguous XMP"));
    assert_eq!(fs::read(src).unwrap(), b"source");
    assert_eq!(fs::read(unrelated).unwrap(), b"unrelated metadata");
    assert_eq!(fs::read(sidecar).unwrap(), b"matching metadata");
    assert_eq!(fs::read(compound).unwrap(), b"conflicting metadata");
}

#[test]
fn contract_find_aae_sidecar_case_insensitive_locked() {
    let temp = TempDir::new().expect("tempdir");
    let src = temp.path().join("IMG_0001.HEIC");
    let sidecar = temp.path().join("img_0001.aae");
    fs::write(&src, b"heic").expect("write src");
    fs::write(&sidecar, b"aae").expect("write sidecar");

    let found = find_aae_sidecar(&src).expect("AAE lookup must not fail");
    assert_eq!(found.as_deref(), Some(sidecar.as_path()));
}

#[test]
fn contract_aae_destination_tracks_output_stem_locked() {
    let temp = TempDir::new().expect("tempdir");
    let src = temp.path().join("IMG_0001.AAE");
    let dst = temp.path().join("IMG_0001_optimized.jxl");

    assert_eq!(
        aae_sidecar_destination(&src, &dst).expect("AAE destination"),
        temp.path().join("IMG_0001_optimized.AAE"),
        "CONTRACT: migrated AAE sidecar must remain adjacent to the converted output stem"
    );
}

#[test]
fn contract_handle_aae_copy_reports_action_and_preserves_source() {
    let temp = TempDir::new().expect("tempdir");
    let src = temp.path().join("IMG_0002.HEIC");
    let sidecar = temp.path().join("IMG_0002.AAE");
    let dst = temp.path().join("out").join("IMG_0002.jxl");
    fs::create_dir_all(dst.parent().expect("dst parent")).expect("create dst parent");
    fs::write(&src, b"heic").expect("write src");
    fs::write(&sidecar, b"aae-payload").expect("write sidecar");
    let source_mtime = filetime::FileTime::from_unix_time(1_700_000_000, 0);
    filetime::set_file_mtime(&sidecar, source_mtime).expect("set source AAE mtime");

    let action = handle_aae_sidecar(&src, &dst).expect("copy AAE");
    assert_eq!(
        action,
        AaeSidecarAction::Copied {
            source: sidecar.clone(),
            destination: dst.with_extension("AAE"),
        }
    );
    assert_eq!(
        fs::read(dst.with_extension("AAE")).expect("read copied AAE"),
        b"aae-payload"
    );
    assert!(sidecar.is_file(), "copy mode must not remove source AAE");
    assert_eq!(
        filetime::FileTime::from_last_modification_time(
            &fs::metadata(dst.with_extension("AAE")).expect("copied AAE metadata")
        ),
        source_mtime,
        "staged publication must retain AAE timestamp preservation"
    );
}

#[test]
fn contract_handle_aae_collision_preserves_both_files() {
    let temp = TempDir::new().expect("tempdir");
    let src = temp.path().join("IMG_0003.HEIC");
    let sidecar = temp.path().join("IMG_0003.AAE");
    let dst = temp.path().join("out").join("IMG_0003.jxl");
    let destination = dst.with_extension("AAE");
    fs::create_dir_all(dst.parent().expect("dst parent")).expect("create dst parent");
    fs::write(&src, b"heic").expect("write src");
    fs::write(&sidecar, b"source-aae").expect("write source AAE");
    fs::write(&destination, b"unrelated-existing-aae").expect("write destination AAE");

    let err = handle_aae_sidecar(&src, &dst)
        .expect_err("a different destination AAE must be a collision");

    assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
    assert_eq!(fs::read(&sidecar).expect("read source AAE"), b"source-aae");
    assert_eq!(
        fs::read(&destination).expect("read destination AAE"),
        b"unrelated-existing-aae"
    );
}

#[test]
fn contract_handle_aae_identical_destination_is_idempotent() {
    let temp = TempDir::new().expect("tempdir");
    let src = temp.path().join("IMG_0004.HEIC");
    let sidecar = temp.path().join("IMG_0004.AAE");
    let dst = temp.path().join("out").join("IMG_0004.jxl");
    let destination = dst.with_extension("AAE");
    fs::create_dir_all(dst.parent().expect("dst parent")).expect("create dst parent");
    fs::write(&src, b"heic").expect("write src");
    fs::write(&sidecar, b"same-aae").expect("write source AAE");
    fs::write(&destination, b"same-aae").expect("write destination AAE");

    let action = handle_aae_sidecar(&src, &dst).expect("identical AAE is idempotent");

    assert_eq!(
        action,
        AaeSidecarAction::AlreadyAdjacent {
            source: sidecar.clone(),
            destination: destination.clone(),
        }
    );
    assert_eq!(fs::read(&sidecar).expect("read source AAE"), b"same-aae");
    assert_eq!(
        fs::read(&destination).expect("read destination AAE"),
        b"same-aae"
    );
}

#[cfg(unix)]
#[test]
fn contract_handle_aae_symlink_collision_does_not_touch_target() {
    let temp = TempDir::new().expect("tempdir");
    let src = temp.path().join("IMG_0005.HEIC");
    let sidecar = temp.path().join("IMG_0005.AAE");
    let dst = temp.path().join("out").join("IMG_0005.jxl");
    let destination = dst.with_extension("AAE");
    let symlink_target = temp.path().join("unrelated.AAE");
    fs::create_dir_all(dst.parent().expect("dst parent")).expect("create dst parent");
    fs::write(&src, b"heic").expect("write src");
    fs::write(&sidecar, b"source-aae").expect("write source AAE");
    fs::write(&symlink_target, b"unrelated-aae").expect("write symlink target");
    std::os::unix::fs::symlink(&symlink_target, &destination).expect("create destination symlink");

    let err = handle_aae_sidecar(&src, &dst).expect_err("destination symlink must be a collision");

    assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
    assert!(
        fs::symlink_metadata(&destination)
            .expect("inspect destination")
            .file_type()
            .is_symlink()
    );
    assert_eq!(fs::read(&sidecar).expect("read source AAE"), b"source-aae");
    assert_eq!(
        fs::read(&symlink_target).expect("read symlink target"),
        b"unrelated-aae"
    );
}

#[cfg(unix)]
#[test]
fn contract_handle_aae_hardlink_alias_is_idempotent() {
    let temp = TempDir::new().expect("tempdir");
    let src = temp.path().join("IMG_0006.HEIC");
    let sidecar = temp.path().join("IMG_0006.AAE");
    let dst = temp.path().join("out").join("IMG_0006.jxl");
    let destination = dst.with_extension("AAE");
    fs::create_dir_all(dst.parent().expect("dst parent")).expect("create dst parent");
    fs::write(&src, b"heic").expect("write src");
    fs::write(&sidecar, b"same-inode-aae").expect("write source AAE");
    fs::hard_link(&sidecar, &destination).expect("hard-link destination to source AAE");

    let action = handle_aae_sidecar(&src, &dst).expect("hard-link alias is idempotent");

    assert_eq!(
        action,
        AaeSidecarAction::AlreadyAdjacent {
            source: sidecar.clone(),
            destination: destination.clone(),
        }
    );
    assert_eq!(
        fs::read(&sidecar).expect("read source AAE"),
        b"same-inode-aae"
    );
    assert_eq!(
        fs::read(&destination).expect("read destination AAE"),
        b"same-inode-aae"
    );
}

#[test]
fn contract_find_xmp_sidecar_missing_returns_none() {
    let temp = TempDir::new().expect("tempdir");
    let src = temp.path().join("lonely.png");
    fs::write(&src, b"png").expect("write src");
    assert_eq!(find_xmp_sidecar(&src).unwrap(), None);
}

#[test]
fn contract_xmp_relative_parent_uses_shared_path_rule() {
    use std::path::Path;

    assert_eq!(
        crate::media_conversion_gate::path_parent_or_dot(Path::new("photo.jpg")),
        Path::new(".")
    );
    assert_eq!(
        crate::media_conversion_gate::path_parent_or_dot(Path::new("album/photo.jpg")),
        Path::new("album")
    );
}

#[test]
fn contract_find_xmp_sidecar_refuses_ambiguous() {
    let temp = TempDir::new().expect("tempdir");
    let src = temp.path().join("photo.jpg");
    let full = temp.path().join("photo.jpg.xmp");
    let stem = temp.path().join("photo.xmp");
    fs::write(&src, b"source").expect("write source");
    fs::write(&full, b"<xmp>first</xmp>").expect("write full sidecar");
    fs::write(&stem, b"<xmp>second</xmp>").expect("write stem sidecar");
    assert!(
        find_xmp_sidecar(&src).is_err(),
        "conflicting sidecars must not be silently chosen by filename priority"
    );
    assert_eq!(fs::read(&full).unwrap(), b"<xmp>first</xmp>");
    assert_eq!(fs::read(&stem).unwrap(), b"<xmp>second</xmp>");
}

#[test]
fn contract_find_xmp_sidecar_refuses_non_files_and_incomplete_scan() {
    let temp = TempDir::new().unwrap();
    let src = temp.path().join("photo.jpg");
    fs::write(&src, b"source").unwrap();
    let candidate = temp.path().join("photo.xmp");
    fs::create_dir(&candidate).unwrap();
    assert!(find_xmp_sidecar(&src).is_err());
    fs::remove_dir(&candidate).unwrap();
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(temp.path().join("missing"), &candidate).unwrap();
        assert!(find_xmp_sidecar(&src).is_err());
        fs::remove_file(&candidate).unwrap();
    }
    assert!(find_xmp_sidecar(&src.join("unreadable-parent.jpg")).is_err());
    assert_eq!(fs::read(&src).unwrap(), b"source");
}

#[test]
fn contract_xmp_sidecar_merge_fails_closed_when_destination_missing() {
    let temp = TempDir::new().expect("tempdir");
    let src = temp.path().join("photo.jpg");
    let sidecar = temp.path().join("photo.xmp");
    let missing_dst = temp.path().join("missing.jxl");
    fs::write(&src, [0xFF, 0xD8, 0xFF, 0xD9]).expect("write src");
    fs::write(&sidecar, b"<x:xmpmeta/>").expect("write sidecar");

    let err = merge_xmp_sidecar_into_dest(&src, &missing_dst)
        .expect_err("sidecar exists but destination is missing; merge must fail closed");

    assert!(
        err.to_string().contains("Failed to merge XMP sidecar"),
        "unexpected merge error: {err}"
    );
}

#[test]
fn contract_jxl_xmp_never_uses_exiv2_fallback() {
    let temp = TempDir::new().expect("tempdir");
    let jxl = temp.path().join("archive.JXL");
    let unknown = temp.path().join("unclassified.bin");
    fs::write(&jxl, crate::constants::JXL_CONTAINER_MAGIC).expect("write JXL container magic");
    fs::write(&unknown, b"not a media container").expect("write unknown payload");

    assert!(
        !exiv2_xmp_fallback_allowed(&jxl).expect("detect JXL container"),
        "JXL XMP failure must retain the file rather than route through Exiv2"
    );
    assert!(
        !exiv2_xmp_fallback_allowed(&unknown).expect("classify unknown payload"),
        "unknown destination identity must not authorize an Exiv2 mutation"
    );
}

#[test]
fn contract_delivery_policy_exiftool_no_source_tags_locked() {
    assert!(exiftool_combined_output_indicates_no_source_tags(
        "0 image files updated"
    ));
    assert!(exiftool_combined_output_indicates_no_source_tags(
        "Warning: nothing to do"
    ));
    assert!(!exiftool_combined_output_indicates_no_source_tags(
        "Error: Not a valid JPEG"
    ));
}

#[test]
fn contract_delivery_policy_xattr_absence_locked() {
    let err = std::io::Error::new(std::io::ErrorKind::Unsupported, "xattr not supported");
    assert!(is_xattr_api_absence(&err));
    assert!(is_metadata_delivery_soft_error(&err));
}

#[test]
fn contract_preserve_for_delivery_missing_source_non_blocking() {
    let temp = TempDir::new().expect("tempdir");
    let missing = temp.path().join("no-such-source.jpg");
    let dst = temp.path().join("out.jxl");
    fs::write(&dst, b"jxl").expect("write dst");
    let report = preserve_for_delivery(&missing, &dst).expect("missing source must not Err");
    assert_eq!(
        report.exif,
        MetadataLayerOutcome::SkippedNoSourceMetadata,
        "CONTRACT: missing source skips exif without blocking"
    );
}

#[test]
fn contract_preserve_for_delivery_minimal_source_non_blocking() {
    let temp = TempDir::new().expect("tempdir");
    let src = temp.path().join("bare.png");
    let dst = temp.path().join("out.jxl");
    fs::write(&src, b"\x89PNG\r\n").expect("write bare png");
    fs::write(&dst, b"jxl").expect("write dst");
    preserve_for_delivery(&src, &dst)
        .expect("CONTRACT: minimal source without sidecar/xattr must not block delivery");
}

#[test]
fn contract_exact_metadata_copy_verifier_accepts_matching_metadata() {
    let temp = TempDir::new().expect("tempdir");
    let src = temp.path().join("src.jpg");
    let dst = temp.path().join("dst.jxl");
    fs::write(&src, b"source").expect("write src");
    fs::write(&dst, b"different output bytes").expect("write dst");
    let mtime = filetime::FileTime::from_unix_time(1_700_000_000, 0);
    filetime::set_file_mtime(&src, mtime).expect("set src mtime");
    filetime::set_file_mtime(&dst, mtime).expect("set dst mtime");
    let src_permissions = fs::metadata(&src).expect("src metadata").permissions();
    fs::set_permissions(&dst, src_permissions).expect("copy permissions");

    let check = verify_exact_metadata_copy(&src, &dst).expect("metadata copy matches");

    assert!(check.passed);
    assert_eq!(check.mismatches.len(), 0);
}

#[cfg(target_os = "macos")]
#[test]
fn contract_exact_metadata_copy_verifier_ignores_macos_runtime_xattr_noise() {
    let temp = TempDir::new().expect("tempdir");
    let src = temp.path().join("src.jpg");
    let dst = temp.path().join("dst.jxl");
    fs::write(&src, b"source").expect("write src");
    fs::write(&dst, b"different output bytes").expect("write dst");
    let mtime = filetime::FileTime::from_unix_time(1_700_000_000, 0);
    filetime::set_file_mtime(&src, mtime).expect("set src mtime");
    filetime::set_file_mtime(&dst, mtime).expect("set dst mtime");
    let src_permissions = fs::metadata(&src).expect("src metadata").permissions();
    fs::set_permissions(&dst, src_permissions).expect("copy permissions");

    xattr::set(&src, "com.apple.cscachefs", b"volatile source cache").expect("set cache");
    // com.apple.lastuseddate#PS is now COPIED to dst as asset history —
    // it is no longer silently skipped. Simulate what MFB copy does.
    xattr::set(
        &src,
        "com.apple.lastuseddate#PS",
        b"some photoshop timestamp",
    )
    .expect("set lastuseddate#PS");
    xattr::set(
        &dst,
        "com.apple.lastuseddate#PS",
        b"some photoshop timestamp",
    )
    .expect("copy lastuseddate#PS to dst");
    xattr::set(&src, "user.mfb_contract", b"source-owned metadata").expect("set src xattr");
    xattr::set(&dst, "user.mfb_contract", b"source-owned metadata").expect("set dst xattr");
    xattr::set(
        &dst,
        "com.apple.metadata:kMDItemContentCreationDate",
        b"generated spotlight date",
    )
    .expect("set generated content creation date");
    xattr::set(&dst, "com.apple.provenance", b"generated provenance")
        .expect("set generated provenance");

    let check = verify_exact_metadata_copy(&src, &dst)
        .expect("com.apple.cscachefs on source-only must not fail verify; lastuseddate copied");

    assert!(check.passed);
    assert_eq!(check.mismatches.len(), 0);
}

#[cfg(target_os = "macos")]
#[test]
fn contract_exact_metadata_copy_verifier_ignores_lastuseddate_app_variants() {
    // Regression: delivery was emitting ☢️ RARE ERROR for every file opened
    // in Photoshop because com.apple.lastuseddate#PS was present on JPEG source
    // but absent from JXL destination.
    //
    // Correct fix: copy com.apple.lastuseddate#App xattrs as asset history
    // (same rationale as EXIF DateTimeOriginal / kMDItemWhereFroms).
    // Verify then passes because the xattr IS on the destination.
    let temp = TempDir::new().expect("tempdir");
    let src = temp.path().join("src.jpg");
    let dst = temp.path().join("dst.jxl");
    fs::write(&src, b"source").expect("write src");
    fs::write(&dst, b"output").expect("write dst");
    let mtime = filetime::FileTime::from_unix_time(1_700_000_000, 0);
    filetime::set_file_mtime(&src, mtime).expect("set src mtime");
    filetime::set_file_mtime(&dst, mtime).expect("set dst mtime");
    let src_permissions = fs::metadata(&src).expect("src metadata").permissions();
    fs::set_permissions(&dst, src_permissions).expect("copy permissions");

    // Source has per-app last-used timestamps; destination has them copied.
    xattr::set(&src, "com.apple.lastuseddate#PS", b"photoshop ts").expect("set src #PS");
    xattr::set(&src, "com.apple.lastuseddate#Safari", b"safari ts").expect("set src #Safari");
    // Simulate what the copy path now does: copy these to destination
    xattr::set(&dst, "com.apple.lastuseddate#PS", b"photoshop ts").expect("set dst #PS");
    xattr::set(&dst, "com.apple.lastuseddate#Safari", b"safari ts").expect("set dst #Safari");

    let check = verify_exact_metadata_copy(&src, &dst)
        .expect("com.apple.lastuseddate#* must not fail exact-copy verification");

    assert!(
        check.passed,
        "lastuseddate#App copied to dst must pass verify: {:?}",
        check.mismatches
    );
    assert_eq!(check.mismatches.len(), 0);
}

#[cfg(target_os = "macos")]
#[test]
fn contract_preserve_for_delivery_copies_lastuseddate_variants_end_to_end() {
    let temp = TempDir::new().expect("tempdir");
    let src = temp.path().join("src.jpg");
    let dst = temp.path().join("dst.jxl");
    fs::write(&src, b"source").expect("write src");
    fs::write(&dst, b"output").expect("write dst");

    xattr::set(&src, "com.apple.lastuseddate#PS", b"photoshop ts").expect("set src #PS");
    xattr::set(&src, "com.apple.lastuseddate#Safari", b"safari ts").expect("set src #Safari");

    let report = preserve_for_delivery(&src, &dst).expect("delivery metadata copy");
    assert!(
        !matches!(report.xattr, MetadataLayerOutcome::PartialAudit),
        "lastuseddate copy must not be a partial xattr audit: {:?}",
        report.xattr
    );
    assert_eq!(
        xattr::get(&dst, "com.apple.lastuseddate#PS").expect("read dst #PS"),
        Some(b"photoshop ts".to_vec())
    );
    assert_eq!(
        xattr::get(&dst, "com.apple.lastuseddate#Safari").expect("read dst #Safari"),
        Some(b"safari ts".to_vec())
    );
}

#[cfg(target_os = "macos")]
#[test]
fn contract_preserve_for_delivery_copies_icloud_cpl_variants_end_to_end() {
    let temp = TempDir::new().expect("tempdir");
    let src = temp.path().join("src.jpg");
    let dst = temp.path().join("dst.jxl");
    fs::write(&src, b"source").expect("write src");
    fs::write(&dst, b"output").expect("write dst");

    xattr::set(&src, "com.apple.cpl.original", b"icloud original").expect("set src cpl original");
    xattr::set(&src, "com.apple.cpl.delete", b"icloud delete").expect("set src cpl delete");

    let report = preserve_for_delivery(&src, &dst).expect("delivery metadata copy");
    assert!(
        !matches!(report.xattr, MetadataLayerOutcome::PartialAudit),
        "iCloud CPL copy must not be a partial xattr audit: {:?}",
        report.xattr
    );
    assert_eq!(
        xattr::get(&dst, "com.apple.cpl.original").expect("read dst cpl original"),
        Some(b"icloud original".to_vec())
    );
    assert_eq!(
        xattr::get(&dst, "com.apple.cpl.delete").expect("read dst cpl delete"),
        Some(b"icloud delete".to_vec())
    );
}

#[cfg(target_os = "macos")]
#[test]
fn contract_exact_copy_xattr_reapply_restores_icloud_cpl_after_file_mutation() {
    let temp = TempDir::new().expect("tempdir");
    let src = temp.path().join("src.jpg");
    let dst = temp.path().join("dst.jxl");
    fs::write(&src, b"source").expect("write src");
    fs::write(&dst, b"output").expect("write dst");

    xattr::set(&src, "com.apple.cpl.original", b"icloud original").expect("set src cpl original");
    xattr::set(&src, "com.apple.cpl.delete", b"icloud delete").expect("set src cpl delete");

    let mut report = MetadataDeliveryReport::default();
    reapply_macos_exact_copy_xattrs_for_delivery(&src, &dst, &mut report);

    assert!(
        !matches!(report.xattr, MetadataLayerOutcome::PartialAudit),
        "CPL replay must not be a partial xattr audit: {:?}",
        report.xattr
    );
    assert_eq!(
        xattr::get(&dst, "com.apple.cpl.original").expect("read dst cpl original"),
        Some(b"icloud original".to_vec())
    );
    assert_eq!(
        xattr::get(&dst, "com.apple.cpl.delete").expect("read dst cpl delete"),
        Some(b"icloud delete".to_vec())
    );
}

#[cfg(target_os = "macos")]
#[test]
fn contract_exact_metadata_copy_verifier_rejects_source_owned_xattr_mismatch() {
    let temp = TempDir::new().expect("tempdir");
    let src = temp.path().join("src.jpg");
    let dst = temp.path().join("dst.jxl");
    fs::write(&src, b"source").expect("write src");
    fs::write(&dst, b"different output bytes").expect("write dst");
    let mtime = filetime::FileTime::from_unix_time(1_700_000_000, 0);
    filetime::set_file_mtime(&src, mtime).expect("set src mtime");
    filetime::set_file_mtime(&dst, mtime).expect("set dst mtime");
    let src_permissions = fs::metadata(&src).expect("src metadata").permissions();
    fs::set_permissions(&dst, src_permissions).expect("copy permissions");

    xattr::set(&src, "user.mfb_contract", b"source metadata").expect("set src xattr");
    xattr::set(&dst, "user.mfb_contract", b"wrong metadata").expect("set dst xattr");

    let err = verify_exact_metadata_copy(&src, &dst)
        .expect_err("source-owned xattr mismatch must fail closed");

    assert!(
        err.to_string().contains("xattrs"),
        "mismatch detail must include xattrs: {err}"
    );
}

#[test]
fn contract_exact_metadata_copy_verifier_rejects_mismatched_metadata() {
    let temp = TempDir::new().expect("tempdir");
    let src = temp.path().join("src.jpg");
    let dst = temp.path().join("dst.jxl");
    fs::write(&src, b"source").expect("write src");
    fs::write(&dst, b"output").expect("write dst");
    filetime::set_file_mtime(&src, filetime::FileTime::from_unix_time(1_700_000_000, 0))
        .expect("set src mtime");
    filetime::set_file_mtime(&dst, filetime::FileTime::from_unix_time(1_700_000_123, 0))
        .expect("set dst mtime");

    let err =
        verify_exact_metadata_copy(&src, &dst).expect_err("metadata mismatch must fail closed");

    assert!(
        err.to_string().contains("Exact metadata copy mismatch"),
        "unexpected exact metadata copy error: {err}"
    );
    assert!(
        err.to_string().contains("modified"),
        "mismatch detail must include modified time: {err}"
    );
}

#[test]
fn contract_directory_metadata_entry_points_reject_file_sources() {
    let temp = TempDir::new().expect("tempdir");
    let file_source = temp.path().join("not-a-dir.jpg");
    let dst_dir = temp.path().join("dst");
    fs::write(&file_source, b"jpeg").expect("write file source");
    fs::create_dir(&dst_dir).expect("create dst");

    let preserve_err = super::preserve_directory(&file_source, &dst_dir)
        .expect_err("directory metadata preservation must reject file source paths");
    assert!(
        preserve_err.to_string().contains("not a directory"),
        "unexpected preserve_directory error: {preserve_err}"
    );

    let snapshot_err = super::save_directory_timestamps(&file_source)
        .expect_err("directory timestamp snapshot must reject file source paths");
    assert!(
        snapshot_err.to_string().contains("not a directory"),
        "unexpected save_directory_timestamps error: {snapshot_err}"
    );

    let restore_err = super::restore_timestamps_from_source_to_output(&file_source, &dst_dir)
        .expect_err("directory timestamp restore must reject file source paths");
    assert!(
        restore_err.to_string().contains("not a directory"),
        "unexpected restore_timestamps_from_source_to_output error: {restore_err}"
    );
}

#[cfg(unix)]
#[test]
fn contract_adjacent_timestamp_sync_preserves_source_and_skips_output_links() -> anyhow::Result<()> {
    use std::os::unix::fs::{MetadataExt, symlink};

    let temp = TempDir::new()?;
    let source = temp.path().join("source");
    let output = temp.path().join("output");
    let source_child = source.join("nested");
    let output_child = output.join("nested");
    fs::create_dir_all(&source_child)?;
    fs::create_dir_all(&output_child)?;
    let original = source_child.join("photo.jpg");
    let converted = output_child.join("photo.jxl");
    let root_original = source.join("cover.jpg");
    let root_converted = output.join("cover.jxl");
    let link_target = temp.path().join("untouched");
    fs::write(&original, b"synthetic original")?;
    fs::write(&converted, b"synthetic conversion")?;
    fs::write(&root_original, b"root-level original")?;
    fs::write(&root_converted, b"root-level conversion")?;
    fs::write(&link_target, b"outside the output")?;
    symlink(&link_target, output_child.join("photo.alias"))?;
    xattr::set(&source_child, "user.mfb_contract", b"source metadata")?;

    let source_time = filetime::FileTime::from_unix_time(1_600_000_000, 123_456_789);
    let output_time = filetime::FileTime::from_unix_time(1_700_000_000, 0);
    for path in [&source, &source_child, &original, &root_original] {
        filetime::set_file_times(path, source_time, source_time)?;
    }
    for path in [&output, &output_child, &converted, &root_converted, &link_target] {
        filetime::set_file_times(path, output_time, output_time)?;
    }
    let observation = |path: &std::path::Path| -> std::io::Result<_> {
        let meta = fs::symlink_metadata(path)?;
        Ok((
            meta.dev(),
            meta.ino(),
            meta.ctime(),
            meta.ctime_nsec(),
            filetime::FileTime::from_last_modification_time(&meta),
        ))
    };
    let unchanged = [&source, &source_child, &original, &root_original, &link_target];
    let before = unchanged
        .iter()
        .map(|path| observation(path))
        .collect::<std::io::Result<Vec<_>>>()?;

    super::restore_timestamps_from_source_to_output(&source, &output)?;

    for (path, expected) in unchanged.into_iter().zip(before) {
        assert_eq!(observation(path)?, expected, "{} changed", path.display());
    }
    for path in [&output, &output_child, &converted, &root_converted] {
        assert_eq!(
            filetime::FileTime::from_last_modification_time(&fs::metadata(path)?),
            source_time,
        );
    }
    assert_eq!(
        xattr::get(&source_child, "user.mfb_contract")?.as_deref(),
        Some(&b"source metadata"[..]),
    );
    assert_eq!(fs::read_link(output_child.join("photo.alias"))?, link_target);
    assert!(super::restore_timestamps_from_source_to_output(&source, &source_child)
        .unwrap_err().to_string().contains("overlap"));
    assert_eq!(xattr::get(&source_child, "user.mfb_contract")?.as_deref(),
        Some(&b"source metadata"[..]));
    fs::remove_file(&root_converted)?;
    fs::hard_link(&link_target, &root_converted)?;
    let linked_before = observation(&link_target)?;
    let error = super::restore_timestamps_from_source_to_output(&source, &output).unwrap_err();
    assert!(error.to_string().contains("standalone file"));
    assert_eq!(observation(&link_target)?, linked_before);
    Ok(())
}

#[test]
fn contract_directory_mirror_finishes_parent_timestamps_after_children() {
    let temp = TempDir::new().expect("tempdir");
    let src = temp.path().join("src");
    let dst = temp.path().join("dst");
    let mut directories = vec![src.clone()];
    for branch in 0..16 {
        let child = src.join(format!("branch-{branch}/child/grandchild"));
        fs::create_dir_all(&child).expect("source tree");
        directories.extend(child.ancestors().take(3).map(std::path::Path::to_path_buf));
    }
    let timestamp = filetime::FileTime::from_unix_time(1_600_000_000, 123_456_789);
    for directory in &directories {
        filetime::set_file_times(directory, timestamp, timestamp).expect("source timestamps");
    }
    super::preserve_directory(&src, &dst).expect("directory metadata mirror");
    super::preserve_directory(&src, &src)
        .expect("in-place directory metadata must remain supported");
    for directory in &directories {
        let target = dst.join(directory.strip_prefix(&src).expect("source-relative path"));
        let actual = fs::metadata(target).expect("mirrored metadata");
        assert_eq!(
            filetime::FileTime::from_last_modification_time(&actual),
            timestamp
        );
        assert_eq!(
            filetime::FileTime::from_last_access_time(&actual),
            timestamp
        );
    }
}

#[cfg(unix)]
#[test]
fn contract_scoped_delivery_metadata_touches_only_recorded_ancestors() -> anyhow::Result<()> {
    use std::collections::BTreeSet;
    use std::os::unix::fs::symlink;
    use std::path::PathBuf;

    let temp = TempDir::new()?;
    let source = temp.path().join("source");
    let output = temp.path().join("output");
    let source_delivery = source.join("recorded/nested");
    let output_delivery = output.join("recorded/nested");
    let unrelated = output.join("unrelated");
    for directory in [
        source_delivery.as_path(), &source.join("ignored/empty"), &source.join("unrelated"),
        &output_delivery, &unrelated,
    ] {
        fs::create_dir_all(directory)?;
    }
    let saved_time = filetime::FileTime::from_unix_time(1_600_000_000, 123_456_789);
    let other_time = filetime::FileTime::from_unix_time(1_700_000_000, 0);
    filetime::set_file_times(&source_delivery, saved_time, saved_time)?;
    filetime::set_file_times(&unrelated, other_time, other_time)?;
    xattr::set(&source_delivery, "user.mfb_contract", b"recorded metadata")?;
    xattr::set(&source.join("unrelated"), "user.mfb_contract", b"unrelated source metadata")?;
    let saved = super::save_directory_timestamps(&source)?;
    let ancestors = BTreeSet::from([PathBuf::new(), "recorded".into(), "recorded/nested".into()]);

    let strict = super::ScopedDeliveryDirectoryMetadata::default();
    super::apply_scoped_delivery_directory_metadata(&saved, &source, &output, &ancestors, strict)?;

    assert_eq!(filetime::FileTime::from_last_modification_time(&fs::metadata(&output_delivery)?), saved_time);
    assert_eq!(xattr::get(&output_delivery, "user.mfb_contract")?.as_deref(), Some(&b"recorded metadata"[..]));
    assert!(!output.join("ignored").exists());
    assert_eq!(filetime::FileTime::from_last_modification_time(&fs::metadata(&unrelated)?), other_time);
    assert_eq!(xattr::get(&unrelated, "user.mfb_contract")?, None);

    fs::remove_dir(&output_delivery)?;
    let before_gate = super::ScopedDeliveryDirectoryMetadata { defer_missing_output_to_gate: true, ..strict };
    super::apply_scoped_delivery_directory_metadata(&saved, &source, &output, &ancestors, before_gate)?;
    assert!(!output_delivery.exists());
    assert!(super::apply_scoped_delivery_directory_metadata(&saved, &source, &output, &ancestors, strict).is_err());
    symlink(&unrelated, &output_delivery)?;
    assert!(super::apply_scoped_delivery_directory_metadata(&saved, &source, &output, &ancestors, before_gate).is_err());
    assert_eq!(xattr::get(&unrelated, "user.mfb_contract")?, None);

    fs::remove_file(&output_delivery)?;
    fs::create_dir(&output_delivery)?;
    filetime::set_file_times(&output_delivery, other_time, other_time)?;
    fs::remove_dir(&source_delivery)?;
    let cleanup_resume = super::ScopedDeliveryDirectoryMetadata { allow_removed_source: true, ..strict };
    assert!(super::apply_scoped_delivery_directory_metadata(&saved, &source, &output, &ancestors, cleanup_resume).is_err());
    let resume_snapshot = super::save_directory_timestamps(&source)?;
    assert!(super::apply_scoped_delivery_directory_metadata(&resume_snapshot, &source, &output, &ancestors, strict).is_err());
    super::apply_scoped_delivery_directory_metadata(&resume_snapshot, &source, &output, &ancestors, cleanup_resume)?;
    assert_eq!(filetime::FileTime::from_last_modification_time(&fs::metadata(&output_delivery)?), other_time);
    Ok(())
}

#[cfg(unix)]
#[test]
fn contract_scoped_directory_retry_copies_xattrs_after_restoring_owner_write() -> anyhow::Result<()> {
    use std::collections::BTreeSet;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};

    let temp = TempDir::new()?;
    let source = temp.path().join("source");
    let output = temp.path().join("output");
    let source_dir = source.join("nested");
    let output_dir = output.join("nested");
    for directory in [&source_dir, &output_dir] {
        fs::create_dir_all(directory)?;
    }
    let key = "user.mfb_metadata_retry";
    xattr::set(&source_dir, key, b"original")?;
    fs::set_permissions(&source_dir, fs::Permissions::from_mode(0o500))?;
    let ancestors = BTreeSet::from([PathBuf::new(), PathBuf::from("nested")]);
    let copy = || {
        let saved = super::save_directory_timestamps(&source)?;
        super::apply_scoped_delivery_directory_metadata(
            &saved,
            &source,
            &output,
            &ancestors,
            super::ScopedDeliveryDirectoryMetadata::default(),
        )
    };
    copy()?;
    let mode = |path: &Path| fs::metadata(path).map(|metadata| metadata.permissions().mode() & 0o777);
    assert_eq!(mode(&output_dir)?, 0o500);

    fs::set_permissions(&source_dir, fs::Permissions::from_mode(0o700))?;
    xattr::set(&source_dir, key, b"after permission recovery")?;
    copy()?;
    assert_eq!(mode(&source_dir)?, 0o700);
    assert_eq!(mode(&output_dir)?, 0o700);
    assert_eq!(
        xattr::get(&output_dir, key)?.as_deref(),
        Some(&b"after permission recovery"[..]),
    );

    let unexpected_key = "user.mfb_metadata_unexpected";
    xattr::set(&output_dir, unexpected_key, b"must not be discarded")?;
    fs::set_permissions(&output_dir, fs::Permissions::from_mode(0o500))?;
    assert!(copy().unwrap_err().to_string().contains("unexpected actual_only"));
    assert_eq!(mode(&output_dir)?, 0o500);
    assert_eq!(mode(&source_dir)?, 0o700);
    assert_eq!(
        xattr::get(&output_dir, unexpected_key)?.as_deref(),
        Some(&b"must not be discarded"[..]),
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn contract_directory_mirror_rejects_links_without_touching_target_metadata() {
    use std::os::unix::fs::symlink;
    let temp = TempDir::new().expect("tempdir");
    let src = temp.path().join("src");
    let dst = temp.path().join("dst");
    let outside = temp.path().join("outside");
    fs::create_dir_all(src.join("child")).expect("source tree");
    fs::create_dir(&outside).expect("outside directory");
    fs::create_dir(&dst).expect("destination root");
    symlink(&outside, dst.join("child")).expect("destination link");
    let timestamp = filetime::FileTime::from_unix_time(1_600_000_100, 0);
    filetime::set_file_times(&outside, timestamp, timestamp).expect("outside timestamps");
    super::preserve_directory(&src, &dst).expect_err("destination link must be rejected");
    assert_eq!(
        filetime::FileTime::from_last_modification_time(&fs::metadata(&outside).expect("outside")),
        timestamp
    );
    fs::remove_file(dst.join("child")).expect("remove destination link");
    symlink(&outside, src.join("linked")).expect("source link");
    super::preserve_directory(&src, &dst).expect_err("source directory link must be rejected");
    assert!(
        !dst.join("linked").exists(),
        "linked trees cannot be mirrored"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn contract_directory_mirror_preserves_acl_without_copying_media() {
    let temp = TempDir::new().expect("tempdir");
    let src = temp.path().join("src");
    let dst = temp.path().join("dst");
    fs::create_dir(&src).expect("source");
    fs::write(src.join("private-media"), b"synthetic").expect("source media");
    let status = std::process::Command::new("/bin/chmod")
        .args(["+a", "everyone deny writeattr,writeextattr"])
        .arg(&src)
        .status()
        .expect("set synthetic directory ACL");
    assert!(status.success(), "synthetic ACL fixture must be supported");
    super::preserve_directory(&src, &dst).expect("directory metadata mirror");
    let listing = |path: &std::path::Path| {
        std::process::Command::new("/bin/ls")
            .args(["-lde", "--"])
            .arg(path)
            .output()
            .expect("read synthetic ACL")
    };
    let source = listing(&src);
    let target = listing(&dst);
    assert!(source.status.success() && target.status.success());
    let entries = |bytes: &[u8]| {
        String::from_utf8_lossy(bytes)
            .lines()
            .skip(1)
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    assert!(
        !entries(&source.stdout).is_empty(),
        "fixture must contain an ACL"
    );
    assert_eq!(entries(&source.stdout), entries(&target.stdout));
    assert!(
        !dst.join("private-media").exists(),
        "metadata mirroring cannot copy media"
    );
    let cleared = std::process::Command::new("/bin/chmod")
        .arg("-N")
        .arg(&src)
        .status()
        .expect("remove the synthetic source ACL");
    assert!(cleared.success());
    let target_reset = std::process::Command::new("/bin/chmod")
        .arg("-N")
        .arg(&dst)
        .status()
        .expect("reset the restrictive destination fixture ACL");
    assert!(target_reset.success());
    let target_acl = std::process::Command::new("/bin/chmod")
        .args(["+a", "everyone allow readattr"])
        .arg(&dst)
        .status()
        .expect("add a nonrestrictive destination-only ACL");
    assert!(target_acl.success());
    super::preserve_directory(&src, &dst).expect("absence of an ACL must also be preserved");
    assert_eq!(entries(&listing(&dst).stdout), Vec::<String>::new());
}

#[test]
fn contract_saved_directory_timestamps_reject_missing_destination_mirror() {
    let temp = TempDir::new().expect("tempdir");
    let src_dir = temp.path().join("src");
    let child_dir = src_dir.join("nested");
    let dst_dir = temp.path().join("dst");
    fs::create_dir_all(&child_dir).expect("create source tree");
    fs::create_dir(&dst_dir).expect("create destination root without nested mirror");

    let saved = super::save_directory_timestamps(&src_dir).expect("snapshot source dirs");
    let err = super::apply_saved_timestamps_to_dst(&saved, &src_dir, &dst_dir)
        .expect_err("missing destination directory mirror must not be silently skipped");

    assert!(
        err.to_string()
            .contains("missing destination directory mirror"),
        "unexpected apply_saved_timestamps_to_dst error: {err}"
    );
}

#[cfg(unix)]
#[test]
fn contract_saved_directory_timestamps_refuse_linked_intermediate_destination() -> anyhow::Result<()> {
    let temp = TempDir::new()?;
    let source = temp.path().join("source");
    let output = temp.path().join("output");
    let outside = temp.path().join("outside");
    let external_child = outside.join("child");
    fs::create_dir_all(source.join("nested/child"))?;
    fs::create_dir_all(&external_child)?;
    fs::create_dir(&output)?;
    std::os::unix::fs::symlink(&outside, output.join("nested"))?;
    let before = fs::metadata(&external_child)?.modified()?;
    let saved = super::save_directory_timestamps(&source)?;
    let error = super::apply_saved_timestamps_to_dst(&saved, &source, &output).unwrap_err();
    assert!(error.to_string().contains("missing destination directory mirror"));
    assert_eq!(fs::metadata(&external_child)?.modified()?, before);
    Ok(())
}

#[cfg(target_os = "macos")]
#[test]
fn contract_macos_timestamp_update_remains_bound_to_open_file() -> anyhow::Result<()> {
    use std::time::{Duration, UNIX_EPOCH};

    let temp = TempDir::new()?;
    let source = temp.path().join("source.jpg");
    let target = temp.path().join("target.jxl");
    let moved = temp.path().join("moved.jxl");
    fs::write(&source, b"source")?;
    fs::write(&target, b"target")?;
    let created = UNIX_EPOCH + Duration::new(1_600_000_000, 12_345_678);
    let added = UNIX_EPOCH + Duration::new(1_650_000_000, 34_567_890);
    let modified = UNIX_EPOCH + Duration::new(1_700_000_000, 56_789_012);
    let modified = filetime::FileTime::from_system_time(modified);
    filetime::set_file_times(&source, modified, modified)?;
    super::macos::set_creation_time(&source, created)?;
    super::macos::set_added_time(&source, added)?;
    let source_file = fs::File::open(&source)?;
    let target_file = fs::File::open(&target)?;
    fs::rename(&target, &moved)?;
    fs::write(&target, b"replacement")?;
    let replacement = fs::metadata(&target)?;

    super::apply_file_timestamps_inner(&source, &target, Some((&source_file, &target_file)))?;

    let bound = fs::metadata(&moved)?;
    assert_eq!(bound.created()?, created);
    assert_eq!(super::macos::get_added_time(&moved)?, added);
    assert_eq!(filetime::FileTime::from_last_modification_time(&bound), modified);
    let current = fs::metadata(&target)?;
    assert_eq!(current.created()?, replacement.created()?);
    assert_eq!(current.modified()?, replacement.modified()?);
    assert_eq!(fs::read(&target)?, b"replacement");
    Ok(())
}

#[test]
fn contract_commit_delivery_runs_exact_metadata_verification_after_timestamps() {
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let source = fs::read_to_string(manifest_dir.join("src/convert/conversion.rs"))
        .expect("conversion source must be readable");
    let commit_start = source
        .find("fn commit_temp_to_output_with_metadata_inner(")
        .expect("commit implementation must exist");
    let commit_end = source[commit_start..]
        .find("fn repair_corrupt_jxl_brotli_exif_for_delivery(")
        .map(|offset| commit_start + offset)
        .expect("commit implementation boundary must exist");
    let commit_source = &source[commit_start..commit_end];
    let timestamp_pos = commit_source
        .find("apply_file_timestamps_for_delivery(src, output, &mut ts_report)?;")
        .expect("timestamp restoration must exist in commit path");
    let verify_pos = commit_source
        .find("verify_exact_metadata_copy(src, output)")
        .expect("delivery metadata verification must exist in commit path");
    let embedded_pos = commit_source
        .find("verify_output_embedded_metadata(")
        .expect("embedded output metadata verification must exist in commit path");
    assert!(
        timestamp_pos < verify_pos,
        "CONTRACT: exact metadata verification must run after final timestamp restore"
    );
    assert!(
        verify_pos < embedded_pos,
        "CONTRACT: embedded identity audit must run after filesystem exact-copy verify"
    );
    let policy_start = source
        .find("fn metadata_output_policy_for_delivery(")
        .expect("metadata delivery policy helper must exist");
    let policy_end = source[policy_start..]
        .find("fn commit_temp_to_output_with_metadata_inner(")
        .map(|offset| policy_start + offset)
        .expect("metadata delivery policy helper boundary must exist");
    let policy_source = &source[policy_start..policy_end];
    assert!(
        policy_source.contains("MetadataOutputPolicy::Preserve"),
        "CONTRACT: same-format delivery must use Preserve policy"
    );
    assert!(
        commit_source.contains("metadata_output_policy_for_delivery(src, output)?"),
        "CONTRACT: commit path must apply the centralized embedded-metadata policy"
    );
}

#[test]
fn contract_meme_finalize_requires_cleared_embedded_metadata() {
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let source = fs::read_to_string(manifest_dir.join("../img/src/lossless_converter.rs"))
        .expect("lossless_converter source must be readable");
    let finalize = source
        .split("pub fn finalize_meme_avif_probe(")
        .nth(1)
        .and_then(|rest| rest.split("pub fn ").next())
        .expect("finalize_meme_avif_probe body");
    assert!(
        finalize.contains("metadata_policy: runtime_config::AvifMetadataPolicy")
            && finalize.contains("metadata_policy.output_policy()"),
        "CONTRACT: meme finalize must enforce the resolved typed AVIF metadata policy"
    );
    assert!(
        finalize.contains("verify_output_embedded_metadata("),
        "CONTRACT: meme finalize must run per-file cleared metadata verification"
    );
    let policy = crate::infra::runtime_config::AvifMetadataPolicy::Clear;
    assert_eq!(policy.output_policy(), crate::metadata::MetadataOutputPolicy::Clear);
    assert_eq!(
        policy.version(), 4,
        "CONTRACT: existing AVIF output reuse requires the progressive Clear contract"
    );
}

#[test]
fn contract_animated_meme_finalize_requires_cleared_embedded_metadata() {
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let source = fs::read_to_string(manifest_dir.join("../vid/src/animated_image.rs"))
        .expect("animated_image source must be readable");
    let convert = source
        .split("pub fn convert_to_avif_meme(")
        .nth(1)
        .and_then(|rest| rest.split("pub fn ").next())
        .expect("convert_to_avif_meme body");
    let commit_pos = convert
        .find("commit_temp_to_output_with_metadata(")
        .expect("animated meme commit");
    let clear_pos = convert
        .find("MetadataOutputPolicy::Clear")
        .expect("animated meme cleared metadata audit");
    assert!(
        commit_pos < clear_pos && convert[commit_pos..clear_pos].contains("None,"),
        "CONTRACT: animated meme commit must not copy source metadata before its Clear audit"
    );
    assert!(
        convert.contains("verify_output_embedded_metadata(")
            && convert.contains("cleared-metadata mismatch output cleanup"),
        "CONTRACT: animated meme output must fail closed and remove residual-metadata output"
    );
}

#[test]
fn contract_metadata_directory_and_xmp_probe_errors_are_not_silent() {
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let metadata_sources = [
        manifest_dir.join("src/metadata/mod.rs"),
        manifest_dir.join("src/metadata/linux.rs"),
        manifest_dir.join("src/metadata/windows.rs"),
        manifest_dir.join("src/metadata/exif.rs"),
    ];
    let mut combined_sources = String::new();
    for source in metadata_sources {
        combined_sources
            .push_str(&fs::read_to_string(&source).expect("metadata source must be readable"));
        combined_sources.push('\n');
    }
    for forbidden in [
        ["if let ", "Ok", "(created) = metadata.created", "()"].concat(),
        ["if let ", "Ok", "(added) = macos::get_added_time(src_path)"].concat(),
        ["out.as_ref().", "is_ok_and", "(|o| o.status.success())"].concat(),
        ["if let ", "Ok", "(out) = &out"].concat(),
        ["if let ", "Ok", "(out) = output"].concat(),
        ["if let ", "Ok", "(meta) = std::fs::metadata(src)"].concat(),
        ["if let ", "Ok", "(r) = result"].concat(),
        ["if let ", "Ok", "(metadata) = std::fs::metadata(src)"].concat(),
        ["&& let ", "Ok", "(mtime) = metadata.modified", "()"].concat(),
    ] {
        assert!(
            !combined_sources.contains(&forbidden),
            "metadata preservation must handle probe/read results with explicit error branches, found: {forbidden}"
        );
    }
}
#[cfg(target_os = "macos")]
#[test]
fn contract_macos_delivery_copies_preservable_xattrs_without_quarantine() -> anyhow::Result<()> {
    let scratch = tempfile::tempdir()?;
    let source = scratch.path().join("source");
    let output = scratch.path().join("generated");
    std::fs::write(&source, b"synthetic source")?;
    std::fs::write(&output, b"synthetic output")?;
    let quarantine = "com.apple.quarantine";
    let custom = "user.mfb_contract";
    xattr::set(&source, quarantine, b"0081;0;MFB-test;")?;
    xattr::set(&source, custom, b"keep this metadata")?;

    let mut report = super::MetadataDeliveryReport::default();
    super::preserve_pro_for_delivery(&source, &output, &mut report, false)?;
    assert!(xattr::get(&source, quarantine)?.is_some());
    assert_eq!(xattr::get(&output, quarantine)?, None);
    assert_eq!(xattr::get(&output, custom)?.as_deref(), Some(b"keep this metadata".as_slice()));
    assert_eq!(std::fs::read(&output)?, b"synthetic output");
    let previous = b"0083;1;MFB-existing;";
    xattr::set(&output, quarantine, previous)?;
    super::preserve_pro_for_delivery(&source, &output, &mut report, false)?;
    assert_eq!(xattr::get(&output, quarantine)?.as_deref(), Some(previous.as_slice()));
    Ok(())
}
