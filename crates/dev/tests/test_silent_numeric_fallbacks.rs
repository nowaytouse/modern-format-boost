// 静默数值回退检测单元测试
//
// 专门检测代码中是否存在静默的数值回退行为，特别是：
// - unwrap_or(0) - 整数默认值回退
// - unwrap_or(0.0) - 浮点数默认值回退
// - unwrap_or(1) - 计数器默认值回退
// - 其他静默数值回退模式

// 模拟测试场景的数据结构
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

    // ❌ 错误示例：静默回退到0
    fn get_width_silent(&self) -> u32 {
        self.width.unwrap_or(0) // 静默回退到0
    }

    // ❌ 错误示例：静默回退到0.0
    fn get_fps_silent(&self) -> f64 {
        self.fps.unwrap_or(0.0) // 静默回退到0.0
    }

    // ❌ 错误示例：静默回退到1
    fn get_frame_count_silent(&self) -> u64 {
        self.frame_count.unwrap_or(1) // 静默回退到1
    }

    // ✅ 正确示例：显式处理
    const fn get_width_explicit(&self) -> Option<u32> {
        self.width // 保持None状态
    }

    // ✅ 正确示例：错误传播
    fn get_quality_result(&self) -> Result<f64, String> {
        self.quality_score
            .ok_or_else(|| "Quality score missing".to_string()) // 明确错误
    }
}

// 模拟配置结构
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

    // ❌ 错误示例：静默回退
    fn get_max_width_silent(&self) -> u32 {
        self.max_width.unwrap_or(1920) // 静默回退到默认值
    }

    // ✅ 正确示例：显式处理
    fn get_max_width_explicit(&self) -> Result<u32, String> {
        self.max_width
            .ok_or_else(|| "Max width not configured".to_string())
    }
}

// 测试辅助函数
const fn create_test_media_with_missing_data() -> TestMediaInfo {
    TestMediaInfo::new() // 所有字段都是None
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
    TestConfig::new() // 所有字段都是None
}

fn test_detect_silent_integer_fallback_to_zero() {
    let info = create_test_media_with_missing_data();

    // ❌ 检测静默回退到0的行为
    let width = info.get_width_silent();
    assert_eq!(width, 0, "静默回退应该返回0");

    // ✅ 验证显式处理
    let width_explicit = info.get_width_explicit();
    assert_eq!(width_explicit, None, "显式处理应该保持None");
}

fn test_detect_silent_float_fallback_to_zero() {
    let info = create_test_media_with_missing_data();

    // ❌ 检测静默回退到0.0的行为
    let fps = info.get_fps_silent();
    assert!(fps.abs() < f64::EPSILON, "静默回退应该返回0.0");
}

fn test_detect_silent_counter_fallback_to_one() {
    let info = create_test_media_with_missing_data();

    // ❌ 检测静默回退到1的行为
    let frame_count = info.get_frame_count_silent();
    assert_eq!(frame_count, 1, "静默回退应该返回1");
}

fn test_detect_silent_config_fallback() {
    let config = create_test_config_missing();

    // ❌ 检测配置中的静默回退
    let max_width = config.get_max_width_silent();
    assert_eq!(max_width, 1920, "配置静默回退应该返回默认值");

    // ✅ 验证显式处理
    let max_width_explicit = config.get_max_width_explicit();
    assert!(max_width_explicit.is_err(), "显式处理应该返回错误");
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
    println!("运行静默数值回退检测测试...");

    test_detect_silent_integer_fallback_to_zero();
    test_detect_silent_float_fallback_to_zero();
    test_detect_silent_counter_fallback_to_one();
    test_detect_silent_config_fallback();
    test_detect_all_silent_fallbacks();

    println!("✅ 静默数值回退检测测试通过！");
}
