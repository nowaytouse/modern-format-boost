use std::path::Path;
use shared_utils::path_safety::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sers_truncation_defense() {
        // Poison Pill: absolute path that triggers V7 pointer offset bug
        let path = Path::new("/Users/nyamiiko/Downloads/GitHub/modern_format_boost/crates/dev/edge/poison_pill_grayscale_icc.jpg");
        let safe_path = magick_safe_path(path);
        
        // With Relativization Shield, it should be a safe relative path
        assert!(safe_path.ends_with("poison_pill_grayscale_icc.jpg"), "Incorrect path: {safe_path}");
        assert!(!safe_path.starts_with("file:"), "Should avoid file: protocol: {safe_path}");
        assert!(!safe_path.starts_with("sers/"), "Found illegal path truncation: {safe_path}");
    }

    #[test]
    fn test_format_expansion_prevention() {
        // Poison Pill: filename with internal property expansion triggers (%)
        let path = Path::new("/Users/nyamiiko/Downloads/GitHub/modern_format_boost/crates/dev/edge/poison_pill_format_expansion.jpg");
        let safe_path = magick_safe_path(path);
        
        // Should use relative path and double-percent locking
        assert!(safe_path.contains("poison_pill_format_expansion.jpg"), "Failed to relativize: {safe_path}");
    }

    #[test]
    fn test_shell_metacharacter_defense() {
        // Poison Pill: Filename with shell metacharacters
        let path = Path::new("crates/dev/edge/poison_pill_shell_injection;test.jpg");
        let safe_path = magick_safe_path(path);
        
        // Character scanner should trigger prepending of ./ to protect ImageMagick delegates
        assert!(safe_path.starts_with("./"), "Failed to prepend ./ for metacharacter safety: {safe_path}");
        assert!(safe_path.contains(";"), "Lost metacharacter: {safe_path}");
    }

    #[test]
    fn test_trailing_space_handling() {
        // Poison Pill: Filename with trailing space
        let path = Path::new("crates/dev/edge/poison_pill_trailing_space.jpg ");
        let safe_path = magick_safe_path(path);
        
        assert!(safe_path.starts_with("./"), "Failed to prepend ./ for trailing space safety: {safe_path}");
        assert!(safe_path.ends_with(" "), "Lost trailing space: {safe_path}");
    }

    #[test]
    fn test_exiftool_argument_injection_defense() {
        // Poison Pill: Filename that looks like an exiftool argument
        let path = Path::new("crates/dev/edge/poison_pill_exiftool_-execute.jpg");
        let safe_path = exiftool_path_arg(path);
        
        // Should prepend ./ to prevent exiftool from executing the command
        assert!(safe_path.starts_with("./"), "ExifTool path failed to prepend ./ for injection safety: {safe_path}");
        assert!(safe_path.contains("-execute"), "Lost injection payload: {safe_path}");
    }
}
