//! 🔥 v6.7: Pure Media Compression Verifier
//!
//! Verification of compression using pure video stream size,
//! completely excluding the impact of container format and metadata.
//!
//! ## Core Logic
//! - Main Criterion: `output_video_stream_size < input_video_stream_size + DEFAULT_SIZE_TOLERANCE_BYTES`
//! - As long as the pure video stream shrinks or increases slightly (less than the standard tolerance), it's considered a success, regardless of total file size.

use crate::stream_size::Info;
#[cfg(feature = "high-precision")]
use rug::Rational;

#[inline]
#[must_use]
#[cfg(feature = "high-precision")]
fn size_ratio_or_one(numerator: u64, denominator: u64) -> Rational {
    if denominator == 0 {
        return Rational::from(1);
    }

    Rational::from((numerator, denominator))
}

#[inline]
#[must_use]
#[cfg(not(feature = "high-precision"))]
fn size_ratio_or_one(numerator: u64, denominator: u64) -> f64 {
    if denominator == 0 {
        return 1.0;
    }

    crate::numeric_cast::u64_to_f64(numerator) / crate::numeric_cast::u64_to_f64(denominator)
}

#[derive(Debug, Clone)]
pub struct PureMediaVerifyResult {
    pub video_compressed: bool,
    pub input_video_size: u64,
    pub output_video_size: u64,
    #[cfg(feature = "high-precision")]
    pub video_compression_ratio: Rational,
    #[cfg(not(feature = "high-precision"))]
    pub video_compression_ratio: f64,
    #[cfg(feature = "high-precision")]
    pub total_compression_ratio: Rational,
    #[cfg(not(feature = "high-precision"))]
    pub total_compression_ratio: f64,
    pub container_overhead_diff: i64,
    pub input_container_overhead: u64,
    pub output_container_overhead: u64,
}

impl PureMediaVerifyResult {
    #[must_use]
    pub fn video_size_change_percent(&self) -> f64 {
        #[cfg(feature = "high-precision")]
        {
            (self.video_compression_ratio.to_f64() - 1.0) * 100.0
        }
        #[cfg(not(feature = "high-precision"))]
        {
            (self.video_compression_ratio - 1.0) * 100.0
        }
    }

    #[must_use]
    pub fn total_size_change_percent(&self) -> f64 {
        #[cfg(feature = "high-precision")]
        {
            (self.total_compression_ratio.to_f64() - 1.0) * 100.0
        }
        #[cfg(not(feature = "high-precision"))]
        {
            (self.total_compression_ratio - 1.0) * 100.0
        }
    }

    #[must_use]
    pub fn is_container_overhead_issue(&self) -> bool {
        #[cfg(feature = "high-precision")]
        {
            self.video_compressed && self.total_compression_ratio >= 1
        }
        #[cfg(not(feature = "high-precision"))]
        {
            self.video_compressed && self.total_compression_ratio >= 1.0
        }
    }

    #[must_use]
    pub fn description(&self) -> String {
        if self.video_compressed {
            if self.is_container_overhead_issue() {
                format!(
                    "✅ Video compressed ({:+.1}%), but container overhead increased total size ({:+.1}%)",
                    self.video_size_change_percent(),
                    self.total_size_change_percent()
                )
            } else {
                format!(
                    "✅ Compression success: Video {:+.1}%, Total {:+.1}%",
                    self.video_size_change_percent(),
                    self.total_size_change_percent()
                )
            }
        } else {
            format!(
                "❌ Compression failed: Video {:+.1}% (Not smaller)",
                self.video_size_change_percent()
            )
        }
    }
}

#[must_use]
pub fn verify_pure_media_compression(
    input_info: &Info,
    output_info: &Info,
    allow_size_tolerance: bool,
) -> PureMediaVerifyResult {
    let input_video = input_info.video_stream_size;
    let output_video = output_info.video_stream_size;

    let video_compressed = if allow_size_tolerance {
        output_video < input_video.saturating_add(crate::constants::DEFAULT_SIZE_TOLERANCE_BYTES)
    } else {
        output_video < input_video
    };

    let video_compression_ratio = size_ratio_or_one(output_video, input_video);
    let total_compression_ratio =
        size_ratio_or_one(output_info.total_file_size, input_info.total_file_size);

    let container_overhead_diff =
        crate::numeric_cast::u64_to_i64_sat(output_info.container_overhead)
            - crate::numeric_cast::u64_to_i64_sat(input_info.container_overhead);

    PureMediaVerifyResult {
        video_compressed,
        input_video_size: input_video,
        output_video_size: output_video,
        video_compression_ratio,
        total_compression_ratio,
        container_overhead_diff,
        input_container_overhead: input_info.container_overhead,
        output_container_overhead: output_info.container_overhead,
    }
}

#[inline]
#[must_use]
pub const fn is_video_compressed(
    input_video_size: u64,
    output_video_size: u64,
    allow_size_tolerance: bool,
) -> bool {
    if allow_size_tolerance {
        output_video_size
            < input_video_size.saturating_add(crate::constants::DEFAULT_SIZE_TOLERANCE_BYTES)
    } else {
        output_video_size < input_video_size
    }
}

#[inline]
#[must_use]
#[cfg(feature = "high-precision")]
pub fn video_compression_ratio(input_video_size: u64, output_video_size: u64) -> Rational {
    size_ratio_or_one(output_video_size, input_video_size)
}

#[inline]
#[must_use]
#[cfg(not(feature = "high-precision"))]
pub fn video_compression_ratio(input_video_size: u64, output_video_size: u64) -> f64 {
    size_ratio_or_one(output_video_size, input_video_size)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stream_size::ExtractionMethod;

    fn make_stream_info(video: u64, audio: u64, overhead: u64) -> Info {
        Info {
            video_stream_size: video,
            audio_stream_size: audio,
            total_file_size: video + audio + overhead,
            container_overhead: overhead,
            extraction_method: ExtractionMethod::BitrateCalculation,
            duration_secs: 60.0,
            video_bitrate: None,
            audio_bitrate: None,
        }
    }

    #[test]
    fn test_video_compressed_success() {
        let input = make_stream_info(1000, 100, 50);
        let output = make_stream_info(800, 100, 50);

        let result = verify_pure_media_compression(&input, &output, false);

        assert!(result.video_compressed);
        #[cfg(feature = "high-precision")]
        assert!(result.video_compression_ratio < 1_i32);
        #[cfg(not(feature = "high-precision"))]
        assert!(result.video_compression_ratio < 1.0);
    }

    #[test]
    fn test_video_compressed_success_within_tolerance() {
        let input = make_stream_info(10_000_000, 100, 50);
        let output = make_stream_info(10_500_000, 100, 50); // 500,000 bytes larger

        let result = verify_pure_media_compression(&input, &output, true);

        assert!(result.video_compressed); // Accepts because < tolerance increase
        #[cfg(feature = "high-precision")]
        assert!(result.video_compression_ratio > 1_i32);
        #[cfg(not(feature = "high-precision"))]
        assert!(result.video_compression_ratio > 1.0);
    }

    #[test]
    fn test_video_not_compressed_exceeds_tolerance() {
        let input = make_stream_info(1000, 100, 50);
        let output = make_stream_info(
            1000 + crate::constants::DEFAULT_SIZE_TOLERANCE_BYTES + 1,
            100,
            50,
        );

        let result = verify_pure_media_compression(&input, &output, true);

        assert!(!result.video_compressed);
        #[cfg(feature = "high-precision")]
        assert!(result.video_compression_ratio > 1_i32);
        #[cfg(not(feature = "high-precision"))]
        assert!(result.video_compression_ratio > 1.0);
    }

    #[test]
    fn test_container_overhead_issue() {
        let input = make_stream_info(1000, 100, 50);
        let output = make_stream_info(900, 100, 200);

        let result = verify_pure_media_compression(&input, &output, false);

        assert!(result.video_compressed);
        assert!(result.is_container_overhead_issue());
        #[cfg(feature = "high-precision")]
        assert!(result.total_compression_ratio > 1_i32);
        #[cfg(not(feature = "high-precision"))]
        assert!(result.total_compression_ratio > 1.0);
    }

    #[test]
    fn test_is_video_compressed() {
        assert!(is_video_compressed(1000, 900, false));
        assert!(is_video_compressed(10_000_000, 10_001_000, true)); // Within tolerance
        assert!(!is_video_compressed(
            10_000,
            10_000 + crate::constants::DEFAULT_SIZE_TOLERANCE_BYTES + 1,
            true
        )); // Exceeds tolerance
    }

    #[test]
    fn test_video_compression_ratio() {
        #[cfg(feature = "high-precision")]
        {
            assert_eq!(
                video_compression_ratio(1000, 800),
                Rational::from((800_i32, 1_000_i32))
            );
            assert_eq!(video_compression_ratio(1000, 1000), Rational::from(1_i32));
            assert_eq!(
                video_compression_ratio(1000, 1200),
                Rational::from((1_200_i32, 1_000_i32))
            );
            assert_eq!(video_compression_ratio(0, 100), Rational::from(1_i32));
        }
        #[cfg(not(feature = "high-precision"))]
        {
            assert!((video_compression_ratio(1000, 800) - 0.8).abs() < 1e-6);
            assert!((video_compression_ratio(1000, 1000) - 1.0).abs() < 1e-6);
            assert!((video_compression_ratio(1000, 1200) - 1.2).abs() < 1e-6);
            assert!((video_compression_ratio(0, 100) - 1.0).abs() < 1e-6);
        }
    }

    #[test]
    fn test_large_sizes_use_integer_compression_decision() {
        let input = make_stream_info((1_u64 << 53) + 1, 0, 0);
        let output = make_stream_info(1_u64 << 53, 0, 0);

        let result = verify_pure_media_compression(&input, &output, false);

        assert!(result.video_compressed);
        assert_eq!(result.input_video_size - result.output_video_size, 1);
    }
}

#[cfg(test)]
mod prop_tests {
    use super::*;
    use crate::stream_size::ExtractionMethod;
    use proptest::prelude::*;

    fn make_stream_info(video: u64, audio: u64, overhead: u64) -> Info {
        Info {
            video_stream_size: video,
            audio_stream_size: audio,
            total_file_size: video + audio + overhead,
            container_overhead: overhead,
            extraction_method: ExtractionMethod::BitrateCalculation,
            duration_secs: 60.0,
            video_bitrate: None,
            audio_bitrate: None,
        }
    }

    proptest! {
        #[test]
        fn prop_compression_judgment_correct(
            input_video in 1000u64..1_000_000_000u64,
            output_video in 1u64..1_000_000_000u64,
            audio in 0u64..100_000_000u64,
            overhead in 0u64..100_000_000u64,
        ) {
            let input = make_stream_info(input_video, audio, overhead);
            let output = make_stream_info(output_video, audio, overhead);

            let result = verify_pure_media_compression(&input, &output, true);

            let expected_compressed = output_video < input_video.saturating_add(crate::constants::DEFAULT_SIZE_TOLERANCE_BYTES);
            prop_assert_eq!(result.video_compressed, expected_compressed,
                "When output {} {} input {}, video_compressed should be {}",
                output_video, if expected_compressed { "<" } else { ">=" },
                input_video, expected_compressed);
        }
    }

    proptest! {
        #[test]
        fn prop_compression_ratio_correct(
            input_video in 1u32..1_000_000_000u32,
            output_video in 1u32..1_000_000_000u32,
        ) {
            let ratio = video_compression_ratio(u64::from(input_video), u64::from(output_video));
            #[cfg(feature = "high-precision")]
            {
                let expected = Rational::from((output_video, input_video));
                prop_assert_eq!(ratio.clone(), expected.clone(),
                    "Compression ratio {} should be expected {}", ratio, expected);
            }
            #[cfg(not(feature = "high-precision"))]
            {
                let expected = crate::numeric_cast::u64_to_f64(u64::from(output_video)) / crate::numeric_cast::u64_to_f64(u64::from(input_video));
                prop_assert!((ratio - expected).abs() < 1e-6,
                    "Compression ratio {} should be close to expected {}", ratio, expected);
            }
        }
    }

    proptest! {
        #[test]
        fn prop_container_overhead_issue_detection(
            input_video in 1000u64..1_000_000_000u64,
            compression_percent in 1u64..50u64,
            input_overhead in 0u64..10_000_000u64,
            extra_overhead in 0u64..100_000_000u64,
        ) {
            let output_video = input_video * (100 - compression_percent) / 100;
            let output_overhead = input_overhead + extra_overhead;

            let input = make_stream_info(input_video, 0, input_overhead);
            let output = make_stream_info(output_video, 0, output_overhead);

            let result = verify_pure_media_compression(&input, &output, true);

            prop_assert!(result.video_compressed,
                "Video compressed from {} to {} should succeed", input_video, output_video);

            let input_total = input.total_file_size;
            let output_total = output.total_file_size;

            if output_total >= input_total {
                prop_assert!(result.is_container_overhead_issue(),
                    "When total file {} >= {} but video successfully compressed, a container overhead issue should be detected",
                    output_total, input_total);
            }
        }
    }
}
