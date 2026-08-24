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
