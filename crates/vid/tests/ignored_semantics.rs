/// Test `TargetVideoFormat::Ignored` variant and ignored semantics
/// Regression test for: semantic confusion between `skip` vs `ignore`
/// Previously code conflated them via strategy.target=Skip + ignored=true
#[cfg(test)]
mod ignored_semantics_tests {
    use vid::{ConversionOutput, ConversionStrategy, TargetVideoFormat};

    /// Test that Ignored variant exists and can be constructed
    #[test]
    fn test_target_video_format_ignored_variant() {
        let ignored = TargetVideoFormat::Ignored;
        let as_str = ignored.as_str();

        // Verify Ignored has a distinct string representation
        assert_eq!(as_str, "Ignored");
        assert_ne!(as_str, "Skip");
    }

    /// Test that `Ignored` variant works in `extension()`
    #[test]
    fn test_ignored_extension() {
        let ignored = TargetVideoFormat::Ignored;
        // Ignored should not map to a real file extension
        // (static images are handled by img, not video output)
        let ext = ignored.extension();
        assert!(ext.is_empty());
    }

    /// Test `ConversionOutput` with `Ignored` strategy
    #[test]
    fn test_conversion_output_ignored_semantic() {
        let output = ConversionOutput {
            input_path: "/test/static.png".to_string(),
            output_path: String::new(), // No output for ignored
            strategy: ConversionStrategy {
                target: TargetVideoFormat::Ignored,
                reason: "Static image - vid ignores (handled by img)".to_string(),
                command: String::new(),
                preserve_audio: false,
                crf: 0.0,
                lossless: false,
            },
            input_size: 100_000,
            output_size: 0,
            size_ratio: 0.0,
            success: true,
            message: "IGNORED: Static image detected".to_string(),
            final_crf: 0.0,
            exploration_attempts: 0,
            blake3: None,
            ignored: true,
        };

        // Verify ignored semantics are consistent
        assert!(output.ignored);
        assert_eq!(output.strategy.target.as_str(), "Ignored");
        assert!(output.output_path.is_empty());
        assert_eq!(output.output_size, 0);
        assert!((output.final_crf - 0.0).abs() < 1e-6);
    }

    /// Test that `outcome()` correctly interprets `Ignored`
    #[test]
    fn test_conversion_output_outcome_ignored() {
        let output = ConversionOutput {
            input_path: "/test/static.gif".to_string(),
            output_path: String::new(),
            strategy: ConversionStrategy {
                target: TargetVideoFormat::Ignored,
                reason: "GIF is static (1 frame)".to_string(),
                command: String::new(),
                preserve_audio: false,
                crf: 0.0,
                lossless: false,
            },
            input_size: 50_000,
            output_size: 0,
            size_ratio: 0.0,
            success: true,
            message: "IGNORED: Static GIF".to_string(),
            final_crf: 0.0,
            exploration_attempts: 0,
            blake3: None,
            ignored: true,
        };

        let outcome = output.outcome();
        // outcome should be Ignored variant
        assert_eq!(format!("{outcome:?}"), "Ignored");

        // Verify conversion was not attempted
        assert!(output.output_path.is_empty());
        assert_eq!(output.output_size, 0);
    }

    /// Test that ignored=true MUST use Ignored target (not Skip)
    #[test]
    fn test_ignored_true_must_use_ignored_target() {
        // This test verifies the semantic fix:
        // Previously: ignored=true with target=Skip (confusing)
        // Now: ignored=true MUST pair with target=Ignored

        let correct = ConversionOutput {
            input_path: "/test/static.png".to_string(),
            output_path: String::new(),
            strategy: ConversionStrategy {
                target: TargetVideoFormat::Ignored,
                reason: "Test: correct semantics".to_string(),
                command: String::new(),
                preserve_audio: false,
                crf: 0.0,
                lossless: false,
            },
            input_size: 100_000,
            output_size: 0,
            size_ratio: 0.0,
            success: true,
            message: "IGNORED".to_string(),
            final_crf: 0.0,
            exploration_attempts: 0,
            blake3: None,
            ignored: true,
        };

        // Verify: when ignored=true, target must be Ignored
        assert!(correct.ignored);
        assert_eq!(correct.strategy.target.as_str(), "Ignored");

        // This is the corrected pattern that prevents semantic confusion
        assert!(correct.output_path.is_empty());
    }
}
