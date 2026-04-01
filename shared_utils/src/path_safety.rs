use std::borrow::Cow;
use std::path::{Path, PathBuf};

#[inline]
#[must_use]
pub fn safe_path_arg(path: &Path) -> Cow<'_, str> {
    let s = path.to_string_lossy();

    // Log warning if lossy conversion occurred (non-UTF-8 path)
    if matches!(s, Cow::Owned(_)) && path.to_str().is_none() {
        eprintln!(
            "Warning: Non-UTF-8 path encountered, using lossy conversion: {}",
            path.display()
        );
    }

    if s.starts_with('-') {
        let mut out = String::with_capacity(2 + s.len());
        out.push_str("./");
        out.push_str(&s);
        Cow::Owned(out)
    } else {
        s
    }
}

/// Specialized path argument escaping for `ExifTool`.
///
/// `ExifTool` interprets `%` characters as format strings (e.g., `%d`, `%f`).
/// To use a literal path containing `%`, it must be escaped to `%%`.
#[inline]
#[must_use]
pub fn exiftool_path_arg(path: &Path) -> Cow<'_, str> {
    let s = safe_path_arg(path);
    if s.contains('%') {
        Cow::Owned(s.replace('%', "%%"))
    } else {
        s
    }
}

/// Returns a unique temporary path for search iterations, fully isolated from user folders.
///
/// Ensures Ghost Mode (Zero Pollution) by using the central MFB tmp directory.
pub fn isolated_temp_path_for_search(output_path: &Path) -> anyhow::Result<PathBuf> {
    let tmp_dir = crate::process_lock::get_mfb_tmp_dir()?;
    let stem = output_path.file_stem().map_or_else(
        || std::borrow::Cow::Borrowed("output"),
        |s| s.to_string_lossy(),
    );
    let ext = output_path.extension().map_or_else(
        || std::borrow::Cow::Borrowed("tmp"),
        |e| e.to_string_lossy(),
    );

    let random_id = crate::conversion::next_temp_output_suffix();

    Ok(tmp_dir.join(format!("{stem}.search.{random_id}.{ext}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn test_safe_path_arg() {
        assert_eq!(safe_path_arg(Path::new("normal.mp4")), "normal.mp4");
        assert_eq!(safe_path_arg(Path::new("/abs/path.mp4")), "/abs/path.mp4");
        assert_eq!(safe_path_arg(Path::new("-dash.mp4")), "./-dash.mp4");
        assert_eq!(safe_path_arg(Path::new("-dir/file.mp4")), "./-dir/file.mp4");
    }

    #[test]
    fn test_exiftool_path_arg() {
        assert_eq!(exiftool_path_arg(Path::new("normal.png")), "normal.png");
        assert_eq!(
            exiftool_path_arg(Path::new("file%2f.png")),
            "file%%2f.png"
        );
        assert_eq!(
            exiftool_path_arg(Path::new("-dash%f.png")),
            "./-dash%%f.png"
        );
    }
}
