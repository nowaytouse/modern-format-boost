use proptest::prelude::*;
use shared_utils::float_compare::approx_eq_f64;
use shared_utils::image_detection::PrecisionMetadata;
use serde_json;

proptest! {
    #[test]
    fn test_float_approx_eq_identity(val in -1000.0..1000.0f64) {
        prop_assert!(approx_eq_f64(val, val));
    }

    #[test]
    fn test_float_approx_eq_symmetry(a in -1000.0..1000.0f64, b in -1000.0..1000.0f64) {
        prop_assert_eq!(approx_eq_f64(a, b), approx_eq_f64(b, a));
    }

    #[test]
    fn test_precision_metadata_roundtrip(
        bit_depth in proptest::option::of(0..=16u8),
        palette_size in proptest::option::of(0..=1024usize),
        color_type in proptest::option::of(0..=255u8),
        is_lossless in proptest::bool::ANY,
        quality_estimate in proptest::option::of(0..=100u8),
        chroma_subsampling in proptest::option::of("4:2:0|4:2:2|4:4:4")
    ) {
        let meta = PrecisionMetadata {
            bit_depth,
            palette_size,
            color_type,
            is_lossless_deterministic: is_lossless,
            quality_estimate,
            chroma_subsampling: chroma_subsampling.map(|s| s.to_string()),
        };

        let serialized = serde_json::to_string(&meta).unwrap();
        let deserialized: PrecisionMetadata = serde_json::from_str(&serialized).unwrap();

        prop_assert_eq!(meta.bit_depth, deserialized.bit_depth);
        prop_assert_eq!(meta.palette_size, deserialized.palette_size);
        prop_assert_eq!(meta.color_type, deserialized.color_type);
        prop_assert_eq!(meta.is_lossless_deterministic, deserialized.is_lossless_deterministic);
        prop_assert_eq!(meta.quality_estimate, deserialized.quality_estimate);
        prop_assert_eq!(meta.chroma_subsampling, deserialized.chroma_subsampling);
    }
}
