# shared_utils

Shared utilities library for modern_format_boost tools.

共享工具库，为 modern_format_boost 工具集提供通用功能。

## Features / 功能

### Quality Matching / 质量匹配
- **quality_matcher**: Unified CRF/distance calculation for AV1, HEVC, JXL encoders
- **image_quality_detector**: Image quality analysis for auto format routing
- **video_quality_detector**: Video quality analysis for auto format routing

### Video Explorer / 视频探索器 🔥 NEW
- **video_explorer**: Unified CRF exploration with three modes:
  - `--explore`: Size-only exploration (find smaller output)
  - `--match-quality`: Quality matching (single encode + SSIM validation)
  - `--explore --match-quality`: Precise quality match (binary search + SSIM judge)

### Media Analysis / 媒体分析
- **ffprobe**: FFprobe wrapper for video analysis
- **codecs**: Codec detection and classification
- **date_analysis**: Deep EXIF/XMP date extraction

### Processing / 处理
- **conversion**: Conversion utilities (ConversionResult, ConvertOptions, anti-duplicate)
- **batch**: Batch file processing with progress tracking
- **video**: Video dimension correction for YUV420 compatibility

### Utilities / 工具
- **progress**: Progress bar with ETA
- **safety**: Dangerous directory detection
- **report**: Summary reporting for batch operations
- **tools**: External tools detection
- **metadata**: EXIF/IPTC/xattr/timestamps/ACL preservation

## Test Coverage / 测试覆盖

**Total: 256 tests + 2 doc tests = 258 tests ✅**

| Module | Tests | Coverage |
|--------|-------|----------|
| quality_matcher | 53 | CRF calculation, BPP, GOP/chroma/HDR factors |
| video_quality_detector | 56 | Video analysis, codec detection, skip logic |
| image_quality_detector | 26 | Image analysis, content classification |
| codecs | 23 | Codec detection, modern/lossless/production |
| video_explorer | 22 | Explore modes, precision proof, judge validation |
| conversion | 22 | Size reduction, output paths, results |
| batch | 20 | Success rate, statistics |
| ffprobe | 17 | Frame rate parsing, bit depth detection |
| video | 11 | YUV420 compatibility, dimension correction |
| report | 9 | Summary reports, health reports |
| others | 6 | Safety, progress, tools |

## Quality Principles / 质量原则

1. **Content-Based Detection** - Detect actual file features via magic bytes, don't trust extensions
   
   **基于实际内容** - 通过魔数检测真实文件特征，不信任扩展名

2. **Fail Loudly** - No silent fallback, errors must be reported with context
   
   **失败即报错** - 无静默fallback，错误必须带上下文响亮报告

3. **Precision Validated** - All calculations verified by "裁判" (judge) tests
   
   **精度验证** - 所有计算由"裁判"测试验证

4. **Consistency Guaranteed** - Same input always produces same output
   
   **一致性保证** - 相同输入始终产生相同输出

## Precision Validation / 精度验证

### Mathematical Precision / 数学精度
- BPP calculation: `bitrate / (width * height * fps)`
- Size reduction: `(1 - output/input) * 100%`
- Success rate: `(succeeded / total) * 100%`
- Frame count: `fps * duration`

### Strict Tests / 严格测试
- NTSC frame rate precision (29.97, 23.976, 59.94)
- Bit depth detection (8/10/12/16-bit)
- Codec classification consistency
- Skip logic accuracy

## Usage / 使用

```rust
use shared_utils::{
    // Quality matching
    calculate_av1_crf, calculate_hevc_crf, calculate_jxl_distance,
    QualityAnalysis, VideoAnalysisBuilder,
    
    // Video explorer (NEW!)
    ExploreMode, ExploreConfig, ExploreResult,
    explore_hevc, explore_hevc_size_only, explore_hevc_quality_match,
    explore_av1, explore_av1_size_only, explore_av1_quality_match,
    
    // Image analysis
    analyze_image_quality, ImageQualityAnalysis,
    
    // Video analysis
    analyze_video_quality, VideoQualityAnalysis,
    
    // Conversion
    ConversionResult, ConvertOptions, calculate_size_reduction,
    
    // FFprobe
    probe_video, parse_frame_rate, detect_bit_depth,
    
    // Codecs
    DetectedCodec, get_codec_info,
    
    // Batch processing
    BatchResult, collect_files,
};
```

## Video Explorer Modes / 视频探索模式

```rust
use shared_utils::{explore_hevc, explore_hevc_size_only, explore_hevc_quality_match};

// Mode 1: --explore only (find smaller size, show SSIM hint)
let result = explore_hevc_size_only(input, output, vf_args, initial_crf)?;

// Mode 2: --match-quality only (single encode + SSIM validation)
let result = explore_hevc_quality_match(input, output, vf_args, predicted_crf)?;

// Mode 3: --explore + --match-quality (binary search + SSIM judge)
let result = explore_hevc(input, output, vf_args, initial_crf)?;
```

## Precision Specification / 精确度规范 🔬

### CRF Precision / CRF 精度
- **Binary search precision**: ±1 CRF (guaranteed within 8 iterations)
- **HEVC range [10, 28]**: needs 5 iterations for ±1 precision
- **AV1 range [10, 35]**: needs 5 iterations for ±1 precision
- **Worst case [0, 51]**: needs 6 iterations for ±1 precision

### SSIM Precision / SSIM 精度
- **Display precision**: 4 decimal places (0.0001)
- **Comparison epsilon**: 0.0001 (for floating point tolerance)

### Quality Grades / 质量等级

| SSIM Range | Grade | Description |
|------------|-------|-------------|
| >= 0.98 | Excellent | 几乎无法区分 |
| >= 0.95 | Good | 视觉无损 |
| >= 0.90 | Acceptable | 轻微差异 |
| >= 0.85 | Fair | 可见差异 |
| < 0.85 | Poor | 明显质量损失 |

### Mathematical Proof / 数学证明

Binary search reduces range by half each iteration:
```
Range [10, 28] = 18
- After 1 iter: 18 / 2 = 9
- After 2 iter: 9 / 2 = 4.5
- After 3 iter: 4.5 / 2 = 2.25
- After 4 iter: 2.25 / 2 = 1.125
- After 5 iter: 1.125 / 2 = 0.5625 < 1 ✅

∴ 5 iterations guarantee ±1 CRF precision
```

## License / 许可证

MIT License
