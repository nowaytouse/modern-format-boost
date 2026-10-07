use std::borrow::Cow;
use std::path::{Path, PathBuf};

#[inline]
#[must_use]
pub fn safe_path_arg(path: &Path) -> Cow<'_, str> {
    let s = path.to_string_lossy();

    // Log warning if lossy conversion occurred (non-UTF-8 path)
    if matches!(s, Cow::Owned(_)) && path.to_str().is_none() {
        crate::media_conversion_gate::delivery_runtime_path_audit(
            "delivery_runtime",
            path,
            crate::infra::static_logs::messages::MSG_PATH_NON_UTF8
                .replace("{}", &path.display().to_string()),
        );
    }

    // ULTIMATE DEFENSE: If path contains shell metacharacters that could compromise
    // ImageMagick delegates or sub-shells, ensure it is treated strictly as a
    // relative literal by prepending './'.
    let has_meta = s.contains(|c: char| {
        matches!(
            c,
            ';' | '&'
                | '|'
                | '$'
                | '`'
                | '('
                | ')'
                | '{'
                | '}'
                | '['
                | ']'
                | '*'
                | '?'
                | '<'
                | '>'
                | '\\'
                | '\n'
                | '\r'
                | '\''
                | '\"'
        )
    });

    // Handle trailing spaces which can cause I/O misinterpretation
    let has_trailing_space = s.ends_with(' ');

    if (s.starts_with('-') && s != "-") || s.starts_with('@') || has_meta || has_trailing_space {
        let mut out = String::with_capacity(2 + s.len());
        if !s.starts_with("./") && !s.starts_with('/') {
            out.push_str("./");
        }
        out.push_str(&s);
        Cow::Owned(out)
    } else {
        s
    }
}

/// Specialized path argument escaping for format-interpreted strings.
/// (e.g. `ImageMagick`'s internal property interpretation).
#[inline]
#[must_use]
pub fn property_safe_path(path: &Path) -> Cow<'_, str> {
    let s = safe_path_arg(path);
    if s.contains('%') {
        Cow::Owned(s.replace('%', "%%"))
    } else {
        s
    }
}

/// `ImageMagick` specific path armor.
/// Prepends './' (protocol-less) to relative paths and doubles '%' to prevent
/// protocol injection and property expansion. Absolute paths use 'file:///'.
#[inline]
#[must_use]
pub fn magick_safe_path(path: &Path) -> Cow<'_, str> {
    // 1. Relativize first to bypass the '/Users' bug and avoid delegates
    let rel_string = crate::media_conversion_gate::path_magick_relativized_lossy(path);

    // 2. Perform property escaping (%%) on the chosen string
    let s_escaped = if rel_string.contains('%') {
        Cow::Owned(rel_string.replace('%', "%%"))
    } else {
        rel_string
    };

    // 3. Apply the Shell/Argfile shield (./) to the escaped string
    // This blocks metacharacter expansion and argfile hijacking
    if let Some(stripped) = s_escaped.strip_prefix('/') {
        // Absolute path fallback (triple-slash)
        let mut out = String::with_capacity(8 + s_escaped.len());
        out.push_str("file:///");
        out.push_str(stripped);
        Cow::Owned(out)
    } else if s_escaped.starts_with("file:")
        || s_escaped.starts_with("mp4:")
        || s_escaped.starts_with("gif:")
    {
        s_escaped
    } else {
        // ULTIMATE DEFENSE: Always prepend ./ to relative paths.
        // This bypasses many IM7 path-parsing bugs (like skip-first-two-chars).
        let mut out = String::with_capacity(2 + s_escaped.len());
        if !s_escaped.starts_with("./") {
            out.push_str("./");
        }
        out.push_str(&s_escaped);
        Cow::Owned(out)
    }
}

/// Fallback path helper for `ExifTool`.
#[inline]
#[must_use]
pub fn exiftool_path_arg(path: &Path) -> Cow<'_, str> {
    let s = path.to_string_lossy();

    if path.is_relative() {
        let mut out = String::with_capacity(2 + s.len());
        if !s.starts_with("./") {
            out.push_str("./");
        }
        out.push_str(&s);
        Cow::Owned(out)
    } else {
        s
    }
}

/// Returns a unique temporary path for search iterations, fully isolated from
/// user folders.
///
/// Ensures Ghost Mode (Zero Pollution) by using the central MFB tmp directory.
/// Create an isolated temporary path for search operations.
///
/// # Errors
/// Returns an error if the temporary file cannot be created.
pub fn isolated_temp_path_for_search(output_path: &Path) -> anyhow::Result<PathBuf> {
    let tmp_dir = crate::media_conversion_gate::delivery_scratch_temp_dir_or_system_temp(
        "path_safety isolated_temp_path_for_search",
    );
    let stem = crate::media_conversion_gate::path_search_temp_stem_or_output(output_path);
    let ext = crate::media_conversion_gate::path_search_temp_ext_or_tmp(output_path);

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
    fn test_safe_path_arg_prefixes() {
        // Test '-' prefix
        assert_eq!(safe_path_arg(Path::new("-test.jpg")), "./-test.jpg");
        // Test '@' prefix
        assert_eq!(safe_path_arg(Path::new("@test.jpg")), "./@test.jpg");
        // Test normal path
        assert_eq!(safe_path_arg(Path::new("normal.jpg")), "normal.jpg");
    }

    #[test]
    fn test_property_safe_path_doubling() {
        // Test single %
        assert_eq!(property_safe_path(Path::new("test%1.jpg")), "test%%1.jpg");
        // Test URL encoded %3A
        assert_eq!(property_safe_path(Path::new("http%3A.jpg")), "http%%3A.jpg");
    }

    #[test]
    fn test_magick_safe_path() {
        // Test relative
        assert_eq!(magick_safe_path(Path::new("img.jpg")), "./img.jpg");
        // Test with %
        assert_eq!(magick_safe_path(Path::new("img%1.jpg")), "./img%%1.jpg");
        // Test absolute
        assert_eq!(
            magick_safe_path(Path::new("/abs/img.jpg")),
            "file:///abs/img.jpg"
        );
        // Test already prepended (idempotency)
        assert_eq!(
            magick_safe_path(Path::new("file:./img.jpg")),
            "file:./img.jpg"
        );
    }

    #[test]
    fn test_exiftool_path_arg() {
        assert_eq!(exiftool_path_arg(Path::new("normal.png")), "./normal.png");
        // ExifTool path arg for main file SHOULD NOT have doubling (now)
        assert_eq!(exiftool_path_arg(Path::new("file%2f.png")), "./file%2f.png");
        // But it should have prefixing
        assert_eq!(exiftool_path_arg(Path::new("-dash%f.png")), "./-dash%f.png");
    }

    #[test]
    fn test_isolated_temp_path_for_search() {
        let output_path = Path::new("my_image.png");
        let temp_path = isolated_temp_path_for_search(output_path).unwrap();
        let temp_parent = temp_path
            .parent()
            .expect("isolated search temp path should have parent");

        let file_name = temp_path.file_name().unwrap().to_str().unwrap();
        assert!(file_name.starts_with("my_image.search."));
        assert!(
            temp_path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("png"))
        );

        // Ensure it ends up in an isolated absolute temp parent, not the current dir.
        // Do not call the scratch-dir resolver a second time here: other tests may
        // mutate MFB_HOME_ROOT in parallel, so a second global-env read can produce
        // a different valid scratch root under llvm-cov.
        assert!(temp_parent.is_absolute());
        assert_ne!(temp_parent, Path::new("."));
    }
}
