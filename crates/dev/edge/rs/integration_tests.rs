// Integration tests for video_explorer module
// Uses synthetic test media files in test/videos/

#[cfg(test)]
mod integration_tests {
    use std::path::PathBuf;

    fn get_test_media_dir() -> PathBuf {
        // Navigate from Cargo manifest directory to edge/
        let cargo_manifest = env!("CARGO_MANIFEST_DIR");
        PathBuf::from(cargo_manifest).join("edge")
    }

    fn video_file(name: &str) -> PathBuf {
        get_test_media_dir().join("videos").join(name)
    }

    #[test]
    fn test_video_media_files_generated() {
        let files = vec![
            "test_h264_10s.mp4",
            "test_vp9_5s.webm",
            "test_hevc_8s.mp4",
            "test_av1_6s.mkv",
            "test_hq_source_15s.mp4",
            "test_lq_source_12s.mp4",
            "test_short_2s.mp4",
        ];

        for file in files {
            let path = video_file(file);
            assert!(
                path.exists(),
                "Test video '{}' should exist at {}",
                file,
                path.display()
            );
        }
    }

    #[test]
    fn test_gif_media_files_generated() {
        let gifs = vec!["test_simple.gif", "test_pattern.gif"];
        let media_dir = get_test_media_dir();

        for gif in gifs {
            let path = media_dir.join("gifs").join(gif);
            assert!(
                path.exists(),
                "Test GIF '{}' should exist at {}",
                gif,
                path.display()
            );
        }
    }

    #[test]
    fn test_media_manifest_documentation() {
        let manifest = get_test_media_dir().join("MEDIA_MANIFEST.md");
        assert!(
            manifest.exists(),
            "Media manifest should exist at {}",
            manifest.display()
        );

        let content = std::fs::read_to_string(&manifest).expect("Should read manifest");

        // Verify manifest contains specifications
        assert!(content.contains("H.264"), "Manifest should document H.264");
        assert!(content.contains("AV1"), "Manifest should document AV1");
        assert!(content.contains("CRF"), "Manifest should document CRF");
        assert!(content.contains("SSIM"), "Manifest should document SSIM");
    }

    #[test]
    fn test_all_major_codecs_represented() {
        let codecs = vec![
            ("H.264/AVC", "test_h264_10s.mp4"),
            ("VP9", "test_vp9_5s.webm"),
            ("HEVC/H.265", "test_hevc_8s.mp4"),
            ("AV1", "test_av1_6s.mkv"),
        ];

        for (codec_name, filename) in codecs {
            assert!(
                video_file(filename).exists(),
                "{codec_name} test video not found: {filename}"
            );
        }
    }

    #[test]
    fn test_media_files_are_readable() {
        let test_files = vec![
            "videos/test_h264_10s.mp4",
            "videos/test_av1_6s.mkv",
            "videos/test_hq_source_15s.mp4",
            "gifs/test_simple.gif",
        ];

        for file in test_files {
            let path = get_test_media_dir().join(file);
            let metadata = std::fs::metadata(&path)
                .unwrap_or_else(|_| panic!("Should read metadata for {file}"));

            assert!(metadata.len() > 0, "File {file} should have content");
        }
    }

    #[test]
    fn test_media_directory_structure() {
        let test_dir = get_test_media_dir();
        assert!(
            test_dir.join("videos").is_dir(),
            "videos/ directory required"
        );
        assert!(test_dir.join("gifs").is_dir(), "gifs/ directory required");
        assert!(
            test_dir.join("MEDIA_MANIFEST.md").is_file(),
            "MEDIA_MANIFEST.md required"
        );
    }

    #[test]
    fn test_crf_precision_video_coverage() {
        // CRF tests need H.264 baseline
        assert!(video_file("test_h264_10s.mp4").exists(), "H.264 baseline");
        // AV1 specific tests
        assert!(video_file("test_av1_6s.mkv").exists(), "AV1 codec");
        // Wide range tests
        assert!(video_file("test_hq_source_15s.mp4").exists(), "HQ source");
    }

    #[test]
    fn test_ssim_quality_grades_video_coverage() {
        // Need high and low quality sources
        assert!(
            video_file("test_hq_source_15s.mp4").exists(),
            "High quality source"
        );
        assert!(
            video_file("test_lq_source_12s.mp4").exists(),
            "Low quality source"
        );
    }

    #[test]
    fn test_zero_gains_duration_coverage() {
        // Various durations for zero-gains tests
        assert!(video_file("test_short_2s.mp4").exists(), "2s duration");
        assert!(video_file("test_h264_10s.mp4").exists(), "10s duration");
        assert!(
            video_file("test_hq_source_15s.mp4").exists(),
            "15s duration"
        );
    }
}
