# 全链路质量决策审计 — 决定最终产出的「嫌疑代码」与负责文件

本文档审计所有**直接影响最终产出质量**的决策点：是否转换、是否保留输出、是否删除原文件、阈值与回退逻辑。每一处都对应具体文件和函数，便于审查与回归。

---

## 一、图像：是否转换 / 转成什么

### 1.1 质量判断 → 是否无损 → 分发分支

| 决策 | 负责文件 | 位置/函数 | 说明 |
|------|----------|-----------|------|
| 得到 `is_lossless` | `shared_utils/src/image_analyzer.rs` | `analyze_image` → `detect_lossless` / HEIC/JXL 内 `detect_compression` | 质量判断链路入口；PNG/TIFF/WebP/AVIF 走 `detect_compression`，HEIC/JXL 在各自 analyzer 内调用 |
| 无损/有损判定实现 | `shared_utils/src/image_detection.rs` | `detect_compression`, `analyze_png_quantization`, `detect_avif_compression`, `detect_heic_compression`, `detect_jxl_compression`, `detect_tiff_compression` 等 | 多维容器/码流解析；PNG 打分+灰区；不确定时 AVIF/HEIC/JXL 返回 Err |
| **转换分发**（转 JXL / 跳过 / 动图策略） | `img_hevc/src/main.rs` | `auto_convert_single_file`：`match (analysis.format, analysis.is_lossless, analysis.is_animated)` | 现代无损→JXL；现代有损→跳过；JPEG→JXL；其它无损→JXL；动图用 `is_lossless` 决定是否跳过现代有损动图 |
| 同上（img_av1） | `img_av1/src/main.rs` | `auto_convert_single_file`，同结构 | 与 img_hevc 一致，使用同一 `analyze_image` |
| 推荐文案（预期压缩率、质量描述） | `shared_utils/src/image_recommender.rs` | `get_recommendation`：`analysis.is_lossless` → expected_size_reduction / quality_preservation | 影响展示与用户预期，不改变文件 |

### 1.2 图像：跳过/收集策略

| 决策 | 负责文件 | 位置/函数 | 说明 |
|------|----------|-----------|------|
| 已为 JXL 则跳过 | `img_hevc/src/main.rs` | `auto_convert_single_file`：`if analysis.format.as_str() == "JXL"` | 避免 JXL→JXL 重复编码 |
| 收集哪些扩展名参与转换 | `shared_utils/src/file_copier.rs` | `IMAGE_EXTENSIONS_FOR_CONVERT`（不含 jxl） | 目录模式下不把 .jxl 列入待转换 |
| 小 PNG 跳过 | `img_hevc/src/lossless_converter.rs` | `convert_to_jxl`：`input_size < 500*1024` 且扩展 png | <500KB PNG 不转，避免无意义压缩 |

---

## 二、图像：输出是否接受（大小/compress）

### 2.1 统一大小容差与 compress 目标

| 决策 | 负责文件 | 位置/函数 | 说明 |
|------|----------|-----------|------|
| 输出大于输入（或 > input×tolerance）则丢弃输出、保留原文件 | `shared_utils/src/conversion.rs` | `check_size_tolerance`：`output_size > max_allowed_size` → 删输出、copy 原文件、`skipped_size_increase` | `allow_size_tolerance` 时 max = input×1.01，否则 1.0 |
| compress 模式：输出 ≥ 输入即视为未达标，删输出、保留原文件 | `shared_utils/src/conversion.rs` | `check_size_tolerance` 后段 + 各 converter 内 compress 分支；`img_hevc/src/conversion_api.rs` / `img_av1/src/conversion_api.rs`：`config.compress && out_size >= detection.file_size` → 删输出、copy 原文件 | 严格「必须更小」才接受 |
| 所有图片转换后是否通过 size 检查 | `img_hevc/src/lossless_converter.rs` | `convert_to_jxl`, `convert_jpeg_to_jxl`, `convert_to_avif` 等内部在 finalize 前调用 `check_size_tolerance` | 见文件头注释：JXL/AVIF/JPEG→JXL/quality-matched JXL 等路径均经过 |

---

## 三、视频：是否转换（编码跳过）

### 3.1 编码级跳过

| 决策 | 负责文件 | 位置/函数 | 说明 |
|------|----------|-----------|------|
| 普通模式：HEVC/AV1/VP9/VVC/AV2 是否跳过 | `shared_utils/src/quality_matcher.rs` | `should_skip_video_codec`：上述编码 → skip，不转 | 已是现代编码则跳过 |
| Apple 兼容模式：仅 HEVC 跳过，AV1/VP9/VVC/AV2 要转成 HEVC | `shared_utils/src/quality_matcher.rs` | `should_skip_video_codec_apple_compat`：仅 H.265 skip | 决定是否进入 HEVC 转换 |
| 调用处 | `vid_hevc/src/conversion_api.rs`, `vid_av1/src/conversion_api.rs` | 入口处根据 codec 调上述两函数 | 决定走 Skip 还是 HEVC/AV1 转换 |

---

## 四、视频：质量/大小校验与是否保留输出

### 4.1 探索阶段：quality_passed / 阈值

| 决策 | 负责文件 | 位置/函数 | 说明 |
|------|----------|-----------|------|
| 默认/可接受 SSIM 下限 | `shared_utils/src/video_explorer/precision.rs` | `DEFAULT_MIN_SSIM = 0.95`, `ACCEPTABLE_MIN_SSIM = 0.90` | 用于质量门限 |
| 默认/高要求 MS-SSIM | `shared_utils/src/video_explorer/precision.rs` | `DEFAULT_MIN_MS_SSIM = 0.90`, `HIGH_QUALITY_MIN_MS_SSIM = 0.95` | 融合/MS-SSIM 判定 |
| 单帧 SSIM 与 fusion（MS-SSIM + SSIM）是否通过 | `shared_utils/src/video_explorer/gpu_coarse_search.rs` | 多处：`ssim >= 0.95` / `0.90`；fusion = MS_SSIM_WEIGHT×ms + SSIM_ALL_WEIGHT×ss；GIF/长片等用 `actual_min_ssim.max(0.92)` 等 | 决定 `quality_passed`、`ms_ssim_passed` |
| 总文件更小 + SSIM 通过 → quality_passed | `shared_utils/src/video_explorer/gpu_coarse_search.rs` | `quality_passed = total_file_compressed && ssim_ok` | 最终是否算「质量过关」 |
| 日志中的 QualityCheck 文案 | `shared_utils/src/video_explorer/gpu_coarse_search.rs` | `match (result.ms_ssim_passed, result.quality_passed)` → PASSED/FAILED 等 | 仅展示，不改变文件 |

### 4.2 视频转换后：不通过则丢弃或保留（含 Apple 回退）

| 决策 | 负责文件 | 位置/函数 | 说明 |
|------|----------|-----------|------|
| 探索未过 quality（或 size）：是否丢弃输出、保留原文件 | `vid_hevc/src/conversion_api.rs` | 约 326–506 行：`!explore_result.quality_passed` 时构造 fail_reason/protect_msg；**若 Apple 兼容且源为 Apple 不兼容编码** 则走 fallback 保留 HEVC 输出，否则 `remove_file(temp_path)` + `copy_on_skip_or_fail`，返回 success: false | 决定「原文件保护」或「保留 best-effort HEVC」 |
| 允许保留 best-effort 的编码列表 | `shared_utils/src/quality_matcher.rs` | `should_keep_best_effort_output_on_failure`：仅 AV1/VP9/VVC/AV2；**不含 ProRes/DNxHD**（必须过 SSIM+体积） | 与 conversion_api 中的 `is_apple_incompatible_video_codec` 一致 |
| MS-SSIM 未过 0.90：是否仍保留输出 | `vid_hevc/src/conversion_api.rs` | 约 522–565 行：`ms_ssim_passed == Some(false)` 时同样按「Apple 兼容 + 不兼容编码」决定保留或丢弃 | 二次质量门 |
| 纯视频流更大则保护原文件 | `vid_hevc/src/conversion_api.rs` | 约 642–655 行：`output_video_stream_size >= input_video_stream_size` → 不 commit 输出，PROTECTED | 与「视频流必须更小」策略一致 |
| 动图→视频：quality 未过则丢弃输出、不保留 fallback | `vid_hevc/src/animated_image.rs` | 约 434–514 行：`!explore_result.quality_passed` 时删 temp、copy_on_skip_or_fail，无 Apple 回退 | GIF/动图不做 best-effort 保留 |

### 4.3 视频：大小容差

| 决策 | 负责文件 | 位置/函数 | 说明 |
|------|----------|-----------|------|
| 动图转视频的 size 容差（1% 等） | `vid_hevc/src/animated_image.rs` | `allow_size_tolerance` → tolerance_ratio 1.01 | 与图片类似，略放宽 |
| HEVC/AV1 转换中的「可压缩」判断 | `vid_hevc/src/conversion_api.rs`, `vid_av1/src/conversion_api.rs` | `can_compress` 等与 `allow_size_tolerance` 相关逻辑 | 影响是否接受略大的输出 |

---

## 五、删除原文件（安全门）

| 决策 | 负责文件 | 位置/函数 | 说明 |
|------|----------|-----------|------|
| 删除原文件前校验输出存在、非空、不小于 min | `shared_utils/src/checkpoint.rs` | `safe_delete_original` → `verify_output_integrity(output, min_output_size)` | 校验失败则不删原文件，打印 PROTECTED |
| 各工具在「成功」路径调用 safe_delete | `img_hevc/src/conversion_api.rs`, `img_av1`, `vid_hevc`, `vid_av1` 等 | `options.delete_original` 时调 `safe_delete_original` | 只有输出合规才可能删原文件 |

---

## 六、图像推荐/路由（仅建议，不直接改文件）

| 决策 | 负责文件 | 位置/函数 | 说明 |
|------|----------|-----------|------|
| 无损/有损/动图 → 推荐 JXL / AVIF / HEVC MP4 / 跳过 | `shared_utils/src/image_quality_core.rs` | `generate_recommendation(format, is_lossless, is_animated, …)` | HEIC 有损推荐跳过；其它决定推荐命令与目标格式，不写盘 |

---

## 七、质量匹配与 CRF（视频编码参数）

| 决策 | 负责文件 | 位置/函数 | 说明 |
|------|----------|-----------|------|
| 根据输入 bpp/编码效率等算目标 CRF | `shared_utils/src/quality_matcher.rs` | CRF 公式、SAFE_BPP_MIN/MAX、encoder 范围 clamp | 决定「目标质量」对应的 CRF，进而影响输出质量与体积 |
| 探索策略、迭代上界 | `shared_utils/src/video_explorer/*.rs` | 步长、最大迭代、收敛条件 | 间接影响最终选中的 CRF 与是否通过验证 |

---

## 八、汇总表：按「是否直接决定最终产出」分类

| 类别 | 负责模块/文件 | 直接决定 |
|------|----------------|----------|
| 图像：转不转、转成啥 | image_analyzer, image_detection, img_hevc/main, img_av1/main | ✅ 是 |
| 图像：接受/丢弃输出 | conversion.rs `check_size_tolerance`, img_hevc/img_av1 conversion_api compress 分支, lossless_converter 各路径 | ✅ 是 |
| 视频：是否跳过编码 | quality_matcher `should_skip_*`, vid_hevc/vid_av1 conversion_api 入口 | ✅ 是 |
| 视频：接受/丢弃/保留 best-effort | vid_hevc conversion_api（quality_passed/ms_ssim/stream size）, quality_matcher `should_keep_best_effort_*`, vid_hevc animated_image | ✅ 是 |
| 视频：质量阈值与 fusion | video_explorer/precision, video_explorer/gpu_coarse_search | ✅ 是 |
| 删原文件 | checkpoint `safe_delete_original` | ✅ 是 |
| 推荐/文案 | image_recommender, image_quality_core | ❌ 仅展示 |

---

## 九、建议审查顺序（按链路）

1. **图像**  
   `image_detection.rs`（质量判断）→ `image_analyzer.rs`（is_lossless）→ `img_hevc/main.rs`（分发）→ `lossless_converter.rs` / `conversion_api.rs`（check_size_tolerance、compress）→ `conversion.rs`（check_size_tolerance、finalize）→ `checkpoint.rs`（safe_delete_original）。

2. **视频**  
   `quality_matcher.rs`（should_skip_*, should_keep_best_effort_*）→ `vid_hevc/conversion_api.rs`（quality_passed、MS-SSIM、stream size、fallback）→ `video_explorer/gpu_coarse_search.rs`（quality_passed、fusion）→ `video_explorer/precision.rs`（阈值常量）→ `vid_hevc/animated_image.rs`（动图质量/丢弃）→ `checkpoint.rs`（safe_delete_original）。

以上为全链路质量相关决策的审计清单与对应负责文件，便于逐处审查与回归测试。

---

## 十、设计问题与边界（扩展审计）

详见 **`docs/AUDIT_DESIGN_ISSUES.md`**。此处仅列要点：

| 编号 | 问题 | 影响 | 负责位置 |
|------|------|------|----------|
| P0-1 | Apple 回退在「质量未过」时可能保留**视频流更大**的输出 | 与「必须更小」策略不一致；仅探索路径未再校验 video_stream | vid_hevc/conversion_api.rs 探索失败分支 |
| P0-2 | compress 边界：`>=` 与「严格更小」语义需在文档中统一写明 | 避免误读为「等于可接受」 | conversion.rs、各 conversion_api |
| D1 | `allow_size_tolerance` 默认 true：视频用 ratio&lt;1.01 视为可接受 | 默认略宽松 | conversion_types.rs、vid_hevc 637–641 行 |
| D2 | `safe_delete_original` 的 min 不一致：图 100、视频 1000 | 视频要求输出更大才删原文件 | img_hevc/av1 conversion_api 用 100；vid_hevc/av1 用 1000 |
| D3 | 探索失败与 require_compression 两处 Apple 回退逻辑重复且条件略不同 | 维护成本；行为需对齐 | vid_hevc/conversion_api.rs 两段 fallback |
| D4 | input_size/ output_size == 0 的防护分散在多处 | 需保证无路径在 output 为空时 commit 或删原文件 | conversion.rs、各 conversion_api、checkpoint |
