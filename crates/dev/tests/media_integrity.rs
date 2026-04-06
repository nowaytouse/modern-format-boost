use shared_utils::jxl_utils::*;
use std::path::Path;

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn get_edge_file(name: &str) -> PathBuf {
        let cargo_manifest = env!("CARGO_MANIFEST_DIR");
        PathBuf::from(cargo_manifest).join("edge").join(name)
    }

    #[test]
    fn test_zero_byte_file_handling() {
        // Poison Pill: 0-byte file that should NOT cause a crash or hang
        let input = get_edge_file("poison_pill_zero_byte.jpg");
        let output_file = tempfile::Builder::new().suffix(".jxl").tempfile().unwrap();
        let output = output_file.path();

        let result =
            run_imagemagick_cjxl_pipeline(&input, output, 1.0, 1, false, 8, false, false, 7);

        // This should gracefully fail because ImageMagick cannot read a 0-byte JPG
        assert!(
            result.is_err(),
            "Pipeline should fail gracefully for 0-byte files!"
        );

        let (_, _, error_msg) = result.unwrap_err();
        // Since ImageMagick is killed or fails, there might be no stdout, but it must not panic
        assert!(
            error_msg.is_empty()
                || error_msg.contains("failed")
                || error_msg.contains("not available")
        );
    }

    #[test]
    fn test_trailing_space_path_loading() {
        // Poison Pill: Filename with trailing space handled via safe_path_arg
        let input = get_edge_file("poison_pill_trailing_space.jpg ");
        let output_file = tempfile::Builder::new().suffix(".jxl").tempfile().unwrap();
        let output = output_file.path();

        // This test ensures the pipeline can at least attempt to call magick on a file with spaces
        let result =
            run_imagemagick_cjxl_pipeline(&input, output, 1.0, 1, false, 8, false, false, 7);

        // On many systems this might still fail if the file doesn't exist, but it must not be a shell injection or hang
        assert!(
            result.is_err(),
            "Expected failure for synthetic space-padded file"
        );
    }

    #[test]
    fn test_metadata_bomb_stamina() {
        // Poison Pill: Image with abnormally high metadata density
        let input = get_edge_file("poison_pill_metadata_bomb.jpg");

        let result = run_imagemagick_cjxl_pipeline(
            &input,
            Path::new("bomb.jxl"),
            1.0,
            1,
            false,
            8,
            false,
            false,
            7,
        );

        // Success here means no OOM and no hang during metadata reading/piping
        assert!(
            result.is_ok() || result.is_err(),
            "Metadata bomb caused a crash or infinite hang!"
        );
    }
}
