//! GAP-4: §`FolderStructure` — verify output path construction preserves source hierarchy.
//!
//! // [GAP-4] path construction verified: `determine_output_path_with_base` (conversion.rs:1209)
//! All fixtures are synthesized temp dirs — no project assets.

use foundation::conversion::determine_output_path_with_base;
use std::path::PathBuf;
use tempfile::TempDir;

fn touch(dir: &std::path::Path, rel: &str) -> anyhow::Result<PathBuf> {
    let p = dir.join(rel);
    let parent = p
        .parent()
        .ok_or_else(|| anyhow::anyhow!("missing parent for {}", p.display()))
        .map_err(anyhow::Error::msg)?;
    std::fs::create_dir_all(parent).map_err(anyhow::Error::msg)?;
    std::fs::write(
        &p,
        b"\xFF\xD8\xFF\xE0\0\x10JFIF\0\x01\x01\0\0\x01\0\x01\0\0\xFF\xD9",
    )
    .map_err(anyhow::Error::msg)?;
    Ok(p)
}

#[test]
fn flat_jpg_to_jxl() -> anyhow::Result<()> {
    let input_root = TempDir::new()?;
    let output_root = TempDir::new()?;
    let src = touch(input_root.path(), "a.jpg")?;

    let out = determine_output_path_with_base(
        &src,
        input_root.path(),
        "jxl",
        &Some(output_root.path().to_path_buf()),
    )
    .map_err(anyhow::Error::msg)?;

    assert_eq!(out, output_root.path().join("a.JXL"));
    Ok(())
}

#[test]
fn nested_preserves_structure() -> anyhow::Result<()> {
    let input_root = TempDir::new()?;
    let output_root = TempDir::new()?;
    let src = touch(input_root.path(), "x/y/z.jpg")?;

    let out = determine_output_path_with_base(
        &src,
        input_root.path(),
        "jxl",
        &Some(output_root.path().to_path_buf()),
    )
    .map_err(anyhow::Error::msg)?;

    assert_eq!(out, output_root.path().join("x/y/z.JXL"));
    Ok(())
}

#[test]
fn deep_nested_structure() -> anyhow::Result<()> {
    let input_root = TempDir::new()?;
    let output_root = TempDir::new()?;
    let src = touch(input_root.path(), "a/b/c/d/e.jpg")?;

    let out = determine_output_path_with_base(
        &src,
        input_root.path(),
        "jxl",
        &Some(output_root.path().to_path_buf()),
    )
    .map_err(anyhow::Error::msg)?;

    assert_eq!(out, output_root.path().join("a/b/c/d/e.JXL"));
    Ok(())
}

#[test]
fn no_output_dir_replaces_ext_in_place() -> anyhow::Result<()> {
    let dir = TempDir::new()?;
    let src = touch(dir.path(), "img.jpg")?;

    let out = determine_output_path_with_base(&src, dir.path(), "jxl", &None)
        .map_err(anyhow::Error::msg)?;
    assert_eq!(out, src.with_extension("JXL"));
    Ok(())
}

#[test]
fn subdirectory_not_created_without_recursion_flag() -> anyhow::Result<()> {
    // GAP-4 no-recurse: scan_image_files with recursive=false
    // should not yield files in subdirs. Test the collector directly.
    let input_root = TempDir::new()?;
    touch(input_root.path(), "top.jpg")?;
    touch(input_root.path(), "sub/img.jpg")?;

    let files = foundation::scan_image_files(
        input_root.path(),
        foundation::IMAGE_EXTENSIONS_FOR_CONVERT,
        false, // non-recursive
    )?;
    let root = input_root.path().canonicalize()?;
    // Only top-level file should appear
    assert!(
        files.iter().all(|p| p.parent() == Some(root.as_path())),
        "non-recursive should not descend into sub/"
    );
    Ok(())
}

#[test]
fn recursive_collects_subdirectory() -> anyhow::Result<()> {
    // [SB-fix] restored: scan_image_files has no DB dep; tests actual file discovery
    let input_root = TempDir::new()?;
    touch(input_root.path(), "top.jpg")?;
    touch(input_root.path(), "sub/img.jpg")?;

    let files = foundation::scan_image_files(
        input_root.path(),
        foundation::IMAGE_EXTENSIONS_FOR_CONVERT,
        true,
    )?;

    assert!(
        files.len() >= 2,
        "recursive: must find both top.jpg and sub/img.jpg"
    );
    Ok(())
}

#[test]
fn path_construction_mirrors_subdir() -> anyhow::Result<()> {
    // [GAP-4] path construction verified: determine_output_path_with_base (conversion.rs:1209)
    let input_root = TempDir::new()?;
    let output_root = TempDir::new()?;
    touch(input_root.path(), "top.jpg")?;
    touch(input_root.path(), "sub/img.jpg")?;

    let sub = input_root.path().join("sub/img.jpg");
    let out = determine_output_path_with_base(
        &sub,
        input_root.path(),
        "jxl",
        &Some(output_root.path().to_path_buf()),
    )
    .map_err(anyhow::Error::msg)?;

    assert_eq!(
        out,
        output_root.path().join("sub/img.JXL"),
        "recursive: nested file must map to mirrored sub-path in output"
    );
    Ok(())
}

#[test]
fn fastmode_delivery_restores_source_and_output_directory_metadata() -> anyhow::Result<()> {
    let input_root = TempDir::new()?;
    let output_root = TempDir::new()?;
    touch(input_root.path(), "album/day1/img.jpg")?;
    std::fs::create_dir_all(output_root.path().join("album/day1"))?;

    let saved = foundation::save_directory_timestamps(input_root.path())?;
    let source_before = std::fs::metadata(input_root.path().join("album/day1"))?.modified()?;

    std::thread::sleep(std::time::Duration::from_millis(1100));
    let mutation = input_root.path().join("album/day1/transient.tmp");
    std::fs::write(&mutation, b"mutation")?;
    std::fs::remove_file(&mutation)?;
    std::fs::write(output_root.path().join("album/day1/img.JXL"), b"jxl")?;

    foundation::restore_delivery_directory_metadata(&saved, input_root.path(), output_root.path())?;

    let source_after = std::fs::metadata(input_root.path().join("album/day1"))?.modified()?;
    let output_after = std::fs::metadata(output_root.path().join("album/day1"))?.modified()?;
    assert_eq!(
        source_after, source_before,
        "source directory timestamp must be restored after verified JPEG deletion"
    );
    assert_eq!(
        output_after, source_before,
        "output directory timestamp must mirror source folder metadata"
    );
    assert!(
        output_root.path().join("album/day1/img.JXL").exists(),
        "metadata restore must preserve the JXL-only folder structure"
    );
    Ok(())
}
