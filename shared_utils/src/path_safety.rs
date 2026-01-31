
use std::path::Path;
use std::borrow::Cow;

/// Sanitizes a file path for command-line usage, specifically for tools like FFmpeg
/// that do not support '--' as a delimiter.
///
/// Ensures the path starts with either '/' (absolute) or './' (relative),
/// preventing it from being misinterpreted as a flag if it starts with '-'.
pub fn safe_path_arg(path: &Path) -> Cow<'_, str> {
    let s = path.to_string_lossy();
    if s.starts_with('-') {
        // Prepend ./ to relative paths starting with -
        Cow::Owned(format!("./{}", s))
    } else {
        // Absolute paths (/) or safe relative paths (parent/, file.txt) are fine
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_safe_path_arg() {
        assert_eq!(safe_path_arg(Path::new("normal.mp4")), "normal.mp4");
        assert_eq!(safe_path_arg(Path::new("/abs/path.mp4")), "/abs/path.mp4");
        assert_eq!(safe_path_arg(Path::new("-dash.mp4")), "./-dash.mp4");
        assert_eq!(safe_path_arg(Path::new("-dir/file.mp4")), "./-dir/file.mp4");
    }
}
