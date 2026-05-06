// Silent numeric fallback detection unit tests
//
// Specifically detects the presence of silent numeric fallback behavior in code, particularly:
// - unwrap_or(0) - Integer default value fallback
// - unwrap_or(0.0) - Float default value fallback
// - unwrap_or(1) - Counter default value fallback
// - Other silent numeric fallback patterns

// Data structure for simulated test scenarios
#[derive(Debug, Clone)]
struct TestMediaInfo {
    width: Option<u32>,
    height: Option<u32>,
    frame_count: Option<u64>,
    duration: Option<f64>,
    fps: Option<f64>,
    bitrate: Option<u64>,
    quality_score: Option<f64>,
}

impl TestMediaInfo {
    const fn new() -> Self {
        Self {
            width: None,
            height: None,
            frame_count: None,
            duration: None,
            fps: None,
            bitrate: None,
            quality_score: None,
        }
    }

    // ❌ Error example: silent fallback to 0
    fn get_width_silent(&self) -> u32 {
        self.width.unwrap_or(0) // Silent fallback to 0
    }

    // ❌ Error example: silent fallback to 0.0
    fn get_fps_silent(&self) -> f64 {
        self.fps.unwrap_or(0.0) // Silent fallback to 0.0
    }

    // ❌ Error example: silent fallback to 1
    fn get_frame_count_silent(&self) -> u64 {
        self.frame_count.unwrap_or(1) // Silent fallback to 1
    }

    // ✅ Correct example: explicit handling
    const fn get_width_explicit(&self) -> Option<u32> {
        self.width // Maintain None state
    }

    // ✅ Correct example: error propagation
    fn get_quality_result(&self) -> Result<f64, String> {
        self.quality_score
            .ok_or_else(|| "Quality score missing".to_string()) // Explicit error
    }
}

// Simulated configuration structure
#[derive(Debug)]
struct TestConfig {
    max_width: Option<u32>,
    target_fps: Option<f64>,
    compression_level: Option<u8>,
}

impl TestConfig {
    const fn new() -> Self {
        Self {
            max_width: None,
            target_fps: None,
            compression_level: None,
        }
    }

    // ❌ Error example: silent fallback
    fn get_max_width_silent(&self) -> u32 {
        self.max_width.unwrap_or(1920) // Silent fallback to default value
    }

    // ✅ Correct example: explicit handling
    fn get_max_width_explicit(&self) -> Result<u32, String> {
        self.max_width
            .ok_or_else(|| "Max width not configured".to_string())
    }
}

// Test helper functions
const fn create_test_media_with_missing_data() -> TestMediaInfo {
    TestMediaInfo::new() // All fields are None
}

const fn create_test_media_with_partial_data() -> TestMediaInfo {
    TestMediaInfo {
        width: Some(1920),
        height: Some(1080),
        frame_count: None,
        duration: None,
        fps: None,
        bitrate: None,
        quality_score: None,
    }
}

const fn create_test_config_missing() -> TestConfig {
    TestConfig::new() // All fields are None
}

fn test_detect_silent_integer_fallback_to_zero() {
    let info = create_test_media_with_missing_data();

    // ❌ Detect silent fallback to 0 behavior
    let width = info.get_width_silent();
    assert_eq!(width, 0, "Silent fallback should return 0");

    // ✅ Verify explicit handling
    let width_explicit = info.get_width_explicit();
    assert_eq!(
        width_explicit, None,
        "Explicit handling should maintain None"
    );
}

fn test_detect_silent_float_fallback_to_zero() {
    let info = create_test_media_with_missing_data();

    // ❌ Detect silent fallback to 0.0 behavior
    let fps = info.get_fps_silent();
    assert!(
        fps.abs() < f64::EPSILON,
        "Silent fallback should return 0.0"
    );
}

fn test_detect_silent_counter_fallback_to_one() {
    let info = create_test_media_with_missing_data();

    // ❌ Detect silent fallback to 1 behavior
    let frame_count = info.get_frame_count_silent();
    assert_eq!(frame_count, 1, "Silent fallback should return 1");
}

fn test_detect_silent_config_fallback() {
    let config = create_test_config_missing();

    // ❌ Detect silent fallback in configuration
    let max_width = config.get_max_width_silent();
    assert_eq!(
        max_width, 1920,
        "Config silent fallback should return default value"
    );

    // ✅ Verify explicit handling
    let max_width_explicit = config.get_max_width_explicit();
    assert!(
        max_width_explicit.is_err(),
        "Explicit handling should return an error"
    );
}

fn test_detect_all_silent_fallbacks() {
    let info = create_test_media_with_partial_data();

    // Verify partial data
    assert_eq!(info.width, Some(1920));
    assert_eq!(info.height, Some(1080));
    assert_eq!(info.frame_count, None);

    // Verify get_quality_result
    let quality_res = info.get_quality_result();
    assert!(quality_res.is_err(), "Quality score should be missing");

    let config = TestConfig {
        max_width: Some(3840),
        target_fps: Some(60.0),
        compression_level: Some(9),
    };

    assert_eq!(config.target_fps, Some(60.0));
    assert_eq!(config.compression_level, Some(9));

    let duration = info.duration;
    assert!(duration.is_none());
    let bitrate = info.bitrate;
    assert!(bitrate.is_none());
}

fn main() {
    println!("Running silent numeric fallback detection tests...");

    test_detect_silent_integer_fallback_to_zero();
    test_detect_silent_float_fallback_to_zero();
    test_detect_silent_counter_fallback_to_one();
    test_detect_silent_config_fallback();
    test_detect_all_silent_fallbacks();

    println!("✅ Silent numeric fallback detection tests passed!");
}
