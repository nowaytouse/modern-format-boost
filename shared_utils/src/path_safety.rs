use std::borrow::Cow;
use std::path::Path;

/// Sanitizes a file path for command-line usage, specifically for tools like FFmpeg
/// that do not support '--' as a delimiter.
///
/// Ensures the path starts with either '/' (absolute) or './' (relative),
/// preventing it from being misinterpreted as a flag if it starts with '-'.
#[inline]
pub fn safe_path_arg(path: &Path) -> Cow<'_, str> {
    let s = path.to_string_lossy();
    if s.starts_with('-') {
        let mut out = String::with_capacity(2 + s.len());
        out.push_str("./");
        out.push_str(&s);
        Cow::Owned(out)
    } else {
        s
    }
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
}
