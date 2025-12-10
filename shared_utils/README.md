# shared_utils

Shared utilities library for modern_format_boost tools.

共享工具库，为 modern_format_boost 工具集提供通用功能。

## Features / 功能

### Quality Matching / 质量匹配
- **quality_matcher**: Unified CRF/distance calculation for AV1, HEVC, JXL encoders
- **image_quality_detector**: Image quality analysis for auto format routing
- **video_quality_detector**: Video quality analysis for auto format routing

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

**Total: 234 tests + 2 doc tests = 236 tests ✅**

| Module | Tests | Coverage |
|--------|-------|----------|
| quality_matcher | 53 | CRF calculation, BPP, GOP/chroma/HDR factors |
| video_quality_detector | 56 | Video analysis, codec detection, skip logic |
| image_quality_detector | 26 | Image analysis, content classification |
| codecs | 23 | Codec detection, modern/lossless/production |
| conversion | 22 | Size reduction, output paths, results |
| batch | 20 | Success rate, statistics |
| ffprobe | 17 | Frame rate parsing, bit depth detection |
| video | 11 | YUV420 compatibility, dimension correction |
| report | 9 | Summary reports, health reports |
| others | 6 | Safety, progress, tools |

## Quality Manifesto / 质量宣言

This library follows the **Pixly Filter Mode Quality Standard**:

本库遵循 **Pixly 滤镜模式质量规范**：

1. **AI-Driven Decisions** - All optimization parameters from AI prediction, no hardcoding
   
   **AI驱动决策** - 所有优化参数由AI预测，禁止硬编码

2. **Content-Based Detection** - Detect actual file features, don't trust extensions
   
   **基于实际内容** - 检测真实文件特征，不信任扩展名

3. **Fail Loudly** - No silent fallback, errors must be reported
   
   **失败即报错** - 无静默fallback，错误必须响亮

4. **Precision Validated** - All calculations verified by "裁判" (judge) tests
   
   **精度验证** - 所有计算由"裁判"测试验证

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

## License / 许可证

MIT License
