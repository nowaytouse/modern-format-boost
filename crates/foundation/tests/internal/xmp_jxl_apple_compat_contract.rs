// CONTRACT: JXL XMP merging appends a reconstruction-safe overlay and never
// routes a container through ExifTool's destructive metadata rewrite.

use super::is_jxl_container;

#[test]
fn contract_jxl_container_detection_uses_content_identity() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let container = temp.path().join("container.jpg");
    let codestream = temp.path().join("codestream.jxl");
    std::fs::write(&container, crate::constants::JXL_CONTAINER_MAGIC)?;
    std::fs::write(&codestream, [0xFF, 0x0A, 0x00])?;

    assert!(is_jxl_container(&container)?);
    assert!(!is_jxl_container(&codestream)?);
    Ok(())
}

#[test]
fn contract_jxl_xmp_overlay_is_idempotent() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let jxl = temp.path().join("photo.jxl");
    let xmp = temp.path().join("photo.xmp");
    let mut container = crate::constants::JXL_CONTAINER_MAGIC.to_vec();
    container.extend_from_slice(&10_u32.to_be_bytes());
    container.extend_from_slice(b"jxlc");
    container.extend_from_slice(&[0xFF, 0x0A]);
    std::fs::write(&jxl, container)?;
    std::fs::write(
        &xmp,
        br#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"/></x:xmpmeta>"#,
    )?;

    assert!(crate::metadata::append_xmp_overlay_to_jxl(&xmp, &jxl)?);
    let once = std::fs::read(&jxl)?;
    assert!(!crate::metadata::append_xmp_overlay_to_jxl(&xmp, &jxl)?);
    assert_eq!(std::fs::read(&jxl)?, once);

    let mut with_trailing_box = once;
    with_trailing_box.extend_from_slice(&8_u32.to_be_bytes());
    with_trailing_box.extend_from_slice(b"free");
    std::fs::write(&jxl, &with_trailing_box)?;
    assert!(!crate::metadata::append_xmp_overlay_to_jxl(&xmp, &jxl)?);
    assert_eq!(std::fs::read(&jxl)?, with_trailing_box);
    Ok(())
}

#[test]
fn contract_jxl_xmp_overlay_freezes_every_preexisting_box_byte() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let jxl = temp.path().join("archive.jxl");
    let xmp = temp.path().join("archive.xmp");
    let mut original = crate::constants::JXL_CONTAINER_MAGIC.to_vec();
    for (kind, payload) in [
        (*b"jbrd", b"reconstruction-owned".as_slice()),
        (*b"uuid", b"unknown-owned".as_slice()),
        (*b"jxlc", b"\xFF\x0A".as_slice()),
    ] {
        let size = u32::try_from(payload.len() + 8)?;
        original.extend_from_slice(&size.to_be_bytes());
        original.extend_from_slice(&kind);
        original.extend_from_slice(payload);
    }
    std::fs::write(&jxl, &original)?;
    std::fs::write(
        &xmp,
        br#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"/></x:xmpmeta>"#,
    )?;

    assert!(crate::metadata::append_xmp_overlay_to_jxl(&xmp, &jxl)?);
    let committed = std::fs::read(&jxl)?;
    assert_eq!(&committed[..original.len()], original.as_slice());
    assert!(committed.len() > original.len());
    Ok(())
}

#[test]
fn contract_jxl_xmp_overlay_rejects_ambiguous_duplicate_jbrd() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let jxl = temp.path().join("ambiguous.jxl");
    let xmp = temp.path().join("ambiguous.xmp");
    let mut container = crate::constants::JXL_CONTAINER_MAGIC.to_vec();
    for payload in [b"first".as_slice(), b"second".as_slice()] {
        let size = u32::try_from(payload.len() + 8)?;
        container.extend_from_slice(&size.to_be_bytes());
        container.extend_from_slice(b"jbrd");
        container.extend_from_slice(payload);
    }
    std::fs::write(&jxl, &container)?;
    std::fs::write(&xmp, b"<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"/>")?;

    let error = crate::metadata::append_xmp_overlay_to_jxl(&xmp, &jxl)
        .expect_err("duplicate jbrd must be rejected before mutation");
    assert!(error.to_string().contains("multiple top-level jbrd"));
    assert_eq!(std::fs::read(&jxl)?, container);
    Ok(())
}

#[test]
fn contract_jxl_xmp_overlay_rejects_truncated_container_before_mutation() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let jxl = temp.path().join("truncated.jxl");
    let xmp = temp.path().join("truncated.xmp");
    let mut original = crate::constants::JXL_CONTAINER_MAGIC.to_vec();
    original.extend_from_slice(&32_u32.to_be_bytes());
    original.extend_from_slice(b"jxlc");
    original.extend_from_slice(&[0xFF, 0x0A]);
    std::fs::write(&jxl, &original)?;
    std::fs::write(&xmp, b"<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"/>")?;

    let error = crate::metadata::append_xmp_overlay_to_jxl(&xmp, &jxl)
        .expect_err("truncated JXL box must fail before replacement");
    assert!(error.to_string().contains("invalid JXL box boundary"));
    assert_eq!(std::fs::read(&jxl)?, original);
    Ok(())
}

#[test]
fn contract_jxl_xmp_overlay_rejects_dtd_and_oversized_sidecars() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let jxl = temp.path().join("archive.jxl");
    let xmp = temp.path().join("archive.xmp");
    let mut original = crate::constants::JXL_CONTAINER_MAGIC.to_vec();
    original.extend_from_slice(&10_u32.to_be_bytes());
    original.extend_from_slice(b"jxlc");
    original.extend_from_slice(&[0xFF, 0x0A]);
    std::fs::write(&jxl, &original)?;
    std::fs::write(
        &xmp,
        b"<!DOCTYPE x [<!ENTITY unsafe SYSTEM \"file:///etc/passwd\">]><x/>",
    )?;
    let dtd_error = crate::metadata::append_xmp_overlay_to_jxl(&xmp, &jxl)
        .expect_err("DTD-bearing XMP must be rejected");
    assert!(dtd_error.to_string().contains("type declarations are forbidden"));
    assert_eq!(std::fs::read(&jxl)?, original);

    let oversized = std::fs::File::create(&xmp)?;
    oversized.set_len(crate::metadata::XMP_OVERLAY_MAX_BYTES + 1)?;
    let size_error = crate::metadata::append_xmp_overlay_to_jxl(&xmp, &jxl)
        .expect_err("oversized XMP must be rejected before copying");
    assert!(size_error.to_string().contains("archive safety limit"));
    assert_eq!(std::fs::read(&jxl)?, original);
    Ok(())
}
