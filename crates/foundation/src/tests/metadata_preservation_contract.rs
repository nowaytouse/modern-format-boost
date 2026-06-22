// CONTRACT: M23 metadata preservation stack (metadata/mod.rs + platform modules).

use super::{
    MetadataLayerOutcome, XATTR_PRESERVE_SKIP_KEYS,
    delivery_policy::{
        exiftool_combined_output_indicates_no_source_tags, is_metadata_delivery_soft_error,
        is_xattr_api_absence,
    },
    aae_sidecar_destination, find_aae_sidecar, find_xmp_sidecar, handle_aae_sidecar,
    is_xattr_preserve_skipped, merge_xmp_sidecar_into_dest, preserve_for_delivery,
    preserve_pro_delivery_layer_order, should_preserve_xattr, verify_exact_metadata_copy,
    AaeSidecarAction,
};
#[cfg(target_os = "macos")]
use super::{
    MetadataDeliveryReport, XATTR_MACOS_EXPLICIT_KEYS, XATTR_MACOS_METADATA_PREFIXES,
    reapply_macos_exact_copy_xattrs_for_delivery, should_copy_macos_extended_xattr,
};
use std::fs;
use tempfile::TempDir;

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
    assert!(!XATTR_MACOS_METADATA_PREFIXES.is_empty());
    assert!(!XATTR_MACOS_EXPLICIT_KEYS.is_empty());
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
        find_xmp_sidecar(&src).as_deref(),
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
        find_xmp_sidecar(&src).as_deref(),
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
    let found = find_xmp_sidecar(&src).expect("CONTRACT: compound stem sidecar must resolve");
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
    let found = find_xmp_sidecar(&src).expect("CONTRACT: stem.XMP sidecar must resolve");
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
        find_xmp_sidecar(&src).as_deref(),
        Some(sidecar.as_path()),
        "CONTRACT: DNG full-extension XMP sidecars must resolve before stem-only fallbacks"
    );
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

    let action = handle_aae_sidecar(&src, &dst, true).expect("copy AAE");
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
}

#[test]
fn contract_find_xmp_sidecar_missing_returns_none() {
    let temp = TempDir::new().expect("tempdir");
    let src = temp.path().join("lonely.png");
    fs::write(&src, b"png").expect("write src");
    assert_eq!(find_xmp_sidecar(&src), None);
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
    assert!(check.mismatches.is_empty());
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
    assert!(check.mismatches.is_empty());
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
    assert!(check.mismatches.is_empty());

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
    xattr::set(&src, "com.apple.lastuseddate#Safari", b"safari ts")
        .expect("set src #Safari");

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

    xattr::set(&src, "com.apple.cpl.original", b"icloud original")
        .expect("set src cpl original");
    xattr::set(&src, "com.apple.cpl.delete", b"icloud delete")
        .expect("set src cpl delete");

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

    xattr::set(&src, "com.apple.cpl.original", b"icloud original")
        .expect("set src cpl original");
    xattr::set(&src, "com.apple.cpl.delete", b"icloud delete")
        .expect("set src cpl delete");

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

#[cfg(target_os = "macos")]
#[test]
fn contract_macos_file_added_time_is_reapplied_and_verified_after_filetime() {
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let source =
        fs::read_to_string(manifest_dir.join("src/metadata/mod.rs")).expect("metadata source");
    let filetime_pos = source
        .find("filetime::set_file_times(dst, atime, mtime)")
        .expect("file timestamp setter must exist");
    let after_filetime = &source[filetime_pos..];

    assert!(
        after_filetime.contains("macos::set_added_time(dst, added)"),
        "macOS file Date Added must be re-applied after filetime::set_file_times"
    );
    assert!(
        after_filetime.contains("Verifying Finder added time integrity"),
        "macOS file Date Added must be verified after re-application"
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
