# 代码审计报告 / Code Audit Report

**日期 / Date**: 2026-02-23  
**范围 / Scope**: modern_format_boost 仓库（重点 shared_utils、video_explorer、ffprobe、image_analyzer、progress_mode、ssim_calculator、stream_analysis、precision）

---

## 1. 已修复问题 / Fixes Applied

### 1.1 路径注入与安全参数 (Path / Argument Safety)

- **image_analyzer.rs** `try_get_frame_count()`: 原先使用 `path.to_str().unwrap_or("")` 传给 ffprobe，非 UTF-8 路径会传空字符串。已改为使用 `crate::safe_path_arg(path).as_ref()`，与仓库内其他 Command 调用一致。
- **ffprobe.rs**:
  - `probe_video()`: 原先用 `path.to_str().ok_or(...)` 再传给 Command；以 `-` 开头的路径可能被解析为选项。已改为 `crate::safe_path_arg(path).as_ref()`，路径会以 `./` 前缀保护。
  - `get_duration()` / `get_frame_count()`: 原先用 `path.to_str()?`，非 UTF-8 会返回 None。已改为 `safe_path_arg(path)`（to_string_lossy），保证始终有合法参数字符串并避免 `-` 注入。

**结论**: 所有通过 `Command::new("ffprobe")` / `ffmpeg` / `magick` / `identify` 等传入的路径，应统一使用 `safe_path_arg(path).as_ref()`。当前仓库内主要调用点已使用；若新增外部命令调用，请沿用同一方式。

#### 第二轮（vid_hevc / vid_av1 conversion_api）
- **vid_hevc/src/conversion_api.rs** `execute_hevc_conversion`、`execute_hevc_lossless`：原先将 `detection.file_path.clone()` 与 `output.display().to_string()` 直接放入 ffmpeg 的 `args`，路径以 `-` 开头时可能被解析为选项。已改为使用 `shared_utils::safe_path_arg(Path::new(&detection.file_path)).as_ref().to_string()` 与 `shared_utils::safe_path_arg(output).as_ref().to_string()`。
- **vid_av1/src/conversion_api.rs** `execute_ffv1_lossless`、`execute_av1_lossless`：同上，输入/输出路径均已改为经 `safe_path_arg` 再传入 ffmpeg。

---

## 2. 并发与锁 (Concurrency & Locking) — 已核查

- **progress_mode.rs** `LOG_FILE_WRITER`: 使用 `if let Ok(mut guard) = LOG_FILE_WRITER.lock()` 与 `has_log_file()` 中的 `.lock().map(...).unwrap_or(false)`，**poison 时**写日志被跳过或返回 false，不会 panic，行为可接受。
- **conversion.rs** `PROCESSED_FILES`: 使用 `lock().unwrap_or_else(|e| e.into_inner())`，**poison 恢复**，符合当前设计。
- **gpu_accel.rs**: 已使用 `lock().unwrap_or_else(|e| e.into_inner())` 或 `if let Ok(...) = ...lock()`，poison 不传播。
- **progress.rs**: `main_bar` / `sub_bar` 的 `.unwrap()` 已改为 `.expect("...")`，便于排查“未初始化即使用”的调用错误。

---

## 3. 除零与数值 (Division & Numerics) — 已修复

- **stream_analysis.rs**: `container_overhead_percent()` 已对 `total_file_size == 0` 做保护，返回 0.0。
- **gpu_coarse_search.rs**: 已引入 `stream_size_change_pct(output_size, input_size)`，内部使用 `input_size.max(1)` 作为分母，所有“视频流大小变化百分比”均经此函数计算，**不再出现 `inf%`**。另对 `prev_size == 0` 时的 `size_increase_pct` 做了分支保护，返回 0.0。
- **quality_matcher.rs**: `(0.5 - megapixels) / 0.5` 等为常量分母，无除零风险。
- **progress.rs**: `main_bar` / `sub_bar` 已改为 `.expect("...")`，见上文。

---

## 4. unwrap / expect / panic 使用

- **数量**: 仓库内 `unwrap()` / `expect()` 使用较多，多集中在**测试**、CLI 输出路径、或“此处失败即不可恢复”的逻辑中。
- **已核查**: **conversion.rs**、**file_sorter.rs**、**xmp_merger.rs** 中提及的 `unwrap()` 均在 `#[test]` 或测试辅助函数内，属测试代码，保留可接受。
- **img_hevc/img_av1 lossless_converter**: `get_output_path(...).unwrap()` 仅出现在测试中；生产路径中 `get_output_path` 返回 `Result` 并由调用方处理。
- **测试与断言**: `image_quality_core.rs` 等处的 `assert!(rec.command.as_ref().unwrap()...)` 仅用于测试，保留。

**建议**: 新增“用户可控输入 → 文件/路径”的代码时，优先使用 `Result` + `?` 并附带路径等上下文。

---

## 5. unsafe 使用 — 已补充注释

- **gpu_accel.rs**: 两处 `unsafe { libc::kill(...) }` 已添加 `// SAFETY:` 注释，说明 `child_pid` 为本进程 spawn 的子进程 PID，有权向其发送信号。
- **metadata/macos.rs**: 模块头已说明“仅用于 FFI 调用系统 C API；CString 与指针在调用期间有效”。`copyfile`、`getattrlist`、`setattrlist` 三处 `unsafe` 块均已添加 `// SAFETY:` 注释（指针有效、同步调用、不保留指针）。

后续新增 `unsafe` 请继续附带简短注释说明安全假设。

---

## 6. 外部命令与依赖

- 所有已知的 `Command::new("ffprobe")` / `ffmpeg` / `magick` / `identify` 等调用，路径参数已通过本次审计改为或确认为使用 `safe_path_arg`，避免路径以 `-` 开头被解析为选项。
- 参数构建多为字面量或受控类型（数字、枚举），未发现直接拼接用户字符串到 shell；若将来需要执行复杂命令行，建议继续使用 `Command::args()` 而非 `bash -c` 一类方式。

---

## 7. 与本次改动相关的模块

- **progress_mode.rs**: 线程本地 log 前缀、UTF-8 安全截断、pad_tag、format_log_line；无新增锁，逻辑清晰。
- **video_explorer/precheck.rs**: ImageMagick 时长回退；仅读原子变量与调用 image_analyzer，无共享可变状态问题。
- **video_explorer/stream_analysis.rs**: `calculate_ssim_all` 多级回退、`run_ssim_all_filter`；仅解析 ffmpeg stderr 并做数值校验，未引入新的命令注入面。
- **video_explorer/gpu_coarse_search.rs**: `quality_verification_skipped_for_format` 仅在本函数内使用，无数据竞争。

---

## 8. 审计小结与优先级

| 类别           | 状态       | 说明 |
|----------------|------------|------|
| 路径/参数安全  | ✅ 已修复 3 处 | image_analyzer、ffprobe 已用 `safe_path_arg`；新加 Command 调用请沿用 |
| Mutex poison   | ✅ 已核查   | progress_mode / conversion / gpu_accel 已 poison 安全；progress 已用 expect |
| 除零/数值      | ✅ 已修复   | gpu_coarse_search 使用 `stream_size_change_pct` 及 prev_size 分支，无 inf% |
| unwrap 使用    | ✅ 已核查   | 高风险处均在测试代码中，生产路径可接受 |
| unsafe         | ✅ 已注释   | gpu_accel、metadata/macos 已加 SAFETY 与模块头说明 |
| 正确性/健壮性  | ✅ 已修复 3 处 | GIF 解析越界防护（image_formats）、rsync 用 which 查找（thread_manager）、processed list 文件锁 Unix flock（conversion）；详见 §30 |

**本轮修复**: 路径安全（前轮 + vid_hevc/vid_av1 conversion_api）+ 除零保护（stream_size_change_pct、prev_size）+ progress expect + unsafe 注释 + **§30 正确性/健壮性（GIF 越界、rsync which、processed list 文件锁）** + 审计文档更新。

---

## 9. 系统审计 / Systematic Audit（后续轮次）

### 9.1 路径安全 — 已补全

- **video_explorer.rs**（SSIM/PSNR/MS-SSIM 等 ffmpeg 调用）: 四处 `Command::new("ffmpeg")` 原先直接 `.arg(&self.input_path)` / `.arg(&self.output_path)`，已全部改为 `crate::safe_path_arg(self.input_path.as_path()).as_ref()` 与 `crate::safe_path_arg(self.output_path.as_path()).as_ref()`（约 2505、2631、2674、2753 行附近）。
- **video_explorer/dynamic_mapping.rs**: 校准阶段将 `temp_gpu` / `temp_cpu` 作为 ffmpeg 输出路径传入 Command 的三处（约 168、217、355 行），已改为 `crate::safe_path_arg(...).as_ref()`，与仓库内其余 Command 调用一致。

### 9.2 unwrap / expect（生产代码）

- **progress / unified_progress**: 模板字符串与进度条构建中仍有 `.expect(...)`，用于“未初始化即使用”的可追溯 panic，已接受。
- **gpu_accel**: 固定偏移等处的 expect 已核查，属可控逻辑。
- **测试与 IO**: 测试代码中的 `.unwrap()` 仅限测试；生产 IO 路径已优先使用 `Result` + `?`。

### 9.3 除零与数值

- 审计中列出的潜在除零点（precheck、report、conversion、video_explorer、dynamic_mapping、calibration、quality_matcher、msssim_progress 等）多数已有上下文保护或仅在有效数据下调用；高优先级处（如 gpu_coarse_search、stream_analysis）已在 §3 中修复。其余可按需在后续迭代中逐处加防护。

### 9.4 Mutex 与锁

- 未使用 `.lock().unwrap()`；Mutex 使用方式为 poison-safe（`lock().unwrap_or_else(|e| e.into_inner())` 或 `if let Ok(...) = ...lock()`），见 §2。

---

## 10. img_av1 / img_hevc 审计与修复

### 10.1 路径安全

- **img_hevc lossless_converter.rs**: 一处 `cjxl` 调用（约 629 行）原先 `.arg(input).arg(&output)`，已改为 `shared_utils::safe_path_arg(input).as_ref()` 与 `shared_utils::safe_path_arg(&output).as_ref()`。
- **img_hevc conversion_api.rs**:  
  - `convert_to_jxl` / `convert_to_avif` 改为通过 `safe_path_arg` 传路径给 cjxl/avifenc，不再使用 `path_to_str` 直接入参。  
  - `convert_to_hevc_mp4` 输出路径 `cmd.arg(&output_abs)` 改为 `cmd.arg(shared_utils::safe_path_arg(&output_abs).as_ref())`。  
  - `preserve_timestamps`（touch）：源/目标路径改为 `safe_path_arg(source).as_ref()` 与 `safe_path_arg(dest).as_ref()`。
- **img_av1 conversion_api.rs**:  
  - `convert_to_jxl`、`convert_to_jxl_lossless`、`convert_to_avif` 均改为用 `shared_utils::safe_path_arg` 传路径给 cjxl/avifenc。  
  - `convert_to_av1_mp4` 输出路径改为 `safe_path_arg(output).as_ref()`。
- **img_av1 main.rs**: djxl 解码时临时文件路径改为 `safe_path_arg(temp_path).as_ref()`，与仓库其余 Command 一致。

### 10.2 除零

- **img_av1 conversion_api.rs**: `size_reduction` 计算两处（execute_conversion 与 execute_animated_conversion）在 `detection.file_size == 0` 时返回 0.0，避免除零。
- **img_hevc conversion_api.rs**: 同上，两处 `size_reduction` 在 `detection.file_size == 0` 时返回 0.0。
- **img_hevc lossless_converter.rs**: JXL 输出大于输入时的 `size_increase_pct` 在 `input_size == 0` 时使用 0.0，避免除零。

### 10.3 unwrap / 测试

- **img_av1 / img_hevc lossless_converter.rs**: `get_output_path(...).unwrap()` 仅出现在 `#[test]` 中，生产路径使用 `Result` + `?`，已接受。

---

## 11. 逻辑 / 数学 / 顺序 核心能力审计 (Logic, Math, Ordering)

### 11.1 除零与数值稳定性

- **gpu_accel.rs** `is_quality_better`: `old_score.combined_score <= 0.0` 时不再做除法，直接返回 `new_score.combined_score > 0.0`，避免除零与 NaN。
- **video_explorer/dynamic_mapping.rs** `add_anchor`: `gpu_size == 0` 时直接 return，不计算 `size_ratio`，避免除零；调用方虽已有 `gpu_size > 0` 条件，此处做防御性保护。
- **conversion.rs**:  
  - `skipped_size_increase`、`success`、`format_size_change`、`calculate_size_reduction`、`check_size_tolerance` 中所有以 `input_size` 为分母的式子，在 `input_size == 0` 时均返回 0.0 或跳过除法，避免除零。
- **video_explorer.rs** 二分搜索阶段: `calc_window_variance` 在 `input_size == 0` 时直接返回 `f64::MAX`，不参与方差计算，避免 0 作分母与无效比例。
- **video_explorer/gpu_coarse_search.rs**:  
  - `size_change_pct`、`total_file_pct` 在 `input_size == 0` 时设为 0.0。  
  - `margin_safety` 在 `target > 0 && final_full_size < target` 时才用 `target` 作分母，避免 `target == 0` 除零。
- **image_metrics.rs** `calculate_ssim_simple`: `pixel_count < 1.0` 时返回 `None`；`denominator < 1e-10` 时返回 `Some(1.0)`（常数图像 SSIM 视为 1），避免除零与数值不稳定。

### 11.2 顺序与前置条件

- 上述除零防护均保证“先判分母再除”的顺序；`dynamic_mapping` 的 `add_anchor` 在更新 `anchors` 前先校验 `gpu_size > 0`，与校准调用处的 `gpu_size > 0 && cpu_size > 0` 一致。

---

## 12. image_metrics.rs 审计修复 (SSIM / MS-SSIM)

### 12.1 P1 — 正确性

- **calculate_ssim_simple**: 方差/协方差改为无偏估计 `(n-1)` 分母，与 Wang et al. 及主路径 `calculate_window_ssim`（高斯加权）一致；小图（< 11×11）SSIM 不再系统性偏高。`n < 2` 时返回 `None`。
- **calculate_ms_ssim**: 按实际参与 scale 的权重和 `used_weight_sum` 做归一化：`ms_ssim.powf(1.0 / used_weight_sum)`，提前 break 时结果仍落在 [0,1]；无任何有效 scale 时（`used_weight_sum < 1e-10`）返回 `None`。

### 12.2 P2 — 性能

- **calculate_window_ssim**: 先将 11×11 窗口一次性读入 `buf_x`/`buf_y`，再基于缓冲区计算均值与方差，将 get_pixel 从 242 次降为 121 次，更利于 cache。
- **calculate_ssim_simple**: 单遍遍历，用 `sum_x, sum_y, sum_xx, sum_yy, sum_xy` 计算均值与无偏方差/协方差，不再分配 `Vec<f64>`，也无三次遍历。

### 12.3 P3 — 代码质量与测试

- 删除未使用变量 `_half_win`；去掉 `#![allow(clippy::needless_range_loop)]`，高斯窗口用 `iter_mut().enumerate()` 写法；为 C1/C2 增加注释（Wang et al. 稳定常数）。
- 新增测试：不同尺寸返回 `None`、小图走 simple 路径、常数图像 SSIM=1、MS-SSIM 同图、过小图 MS-SSIM 返回 `None`。

---

## 13. image_quality_core.rs 审计修复

### 13.1 P1 — 正确性 / 安全性

- **analyze_quality**: `todo!()` 改为返回 `Err(ImgQualityError::NotImplemented(...))`，不再在运行时 panic；`img_errors` 新增 `NotImplemented(String)` 变体。
- **check_avif_lossless**: 保持返回 `false`，增加文档说明“未实现，调用方不得依赖此结果判断是否无损”；保留 TODO 提示后续解析 av1C/sequence_header_obu。
- **generate_recommendation**: 所有拼接到 command 的路径均经 `shell_escape_single_quoted`（单引号内 `'` → `'\''`），避免路径含单引号时 shell 注入；并改为返回 `Result<ConversionRecommendation>`，当 `file_stem()` 无法得到有效 base 时返回 `Err`，避免静默使用 `"output"` 导致文件名冲突。

### 13.2 P2 — 逻辑 / 设计

- **is_format_lossless**: 从“无损”列表中移除 `ImageFormat::Gif`（GIF 为 256 色调色板，全彩转 GIF 为有损）；文档注明仅对真正无损格式返回 true。
- **generate_recommendation**: 使用 `format` 参数：当 `format` 为 HEIC/HEIF 且 `is_lossless == false` 时，推荐 `should_convert: false`，理由为“避免二代有损”。
- **analyze_gif_quality**: GIF 无质量参数概念，改为返回 `estimated_quality: None`、`confidence: 0.0`，仅保留 bit_depth/color_type/compression_method 等元数据；删除启发式质量分数与虚高置信度。

### 13.3 P3 — 代码质量与测试

- **output_base**: 不再使用 `unwrap_or("output")`，改为 `ok_or_else(|| ImgQualityError::AnalysisError(...))?`，调用方需处理 `Result`。
- **calculate_entropy**: 当宽或高 > 256 时先 `thumbnail(256, 256)` 再转 luma 计算熵，减少大图全量转换；并防护 `total < 1.0` 时除零。当前仅被删除的 GIF 启发式使用，保留函数并加 `#[allow(dead_code)]` 以备复用。
- **测试**: 新增/调整：路径含单引号时 command 正确转义、HEIC 有损跳过、无法解析路径时返回 Err、GIF 质量返回 `estimated_quality: None` 与 `confidence: 0.0`；`test_format_lossless` 不再断言 GIF 为无损。

---

## 14. img_av1 conversion_api.rs 审计修复

### 14.1 P1 — 正确性

- **determine_strategy 展示命令与实际执行一致**: 展示用 `command` 与 `convert_to_jxl`/`convert_to_av1_mp4` 实际参数对齐（JPEG: `--lossless_jpeg=1 --`；静态无损: `-d 0.0 -e 7 --`；动图: 含 `-y -pix_fmt yuv420p`）；并注明为 illustrative，NoConversion 时改为 `command: Option<String>` 的 `None`。
- **simple_convert 策略与 smart_convert 一致**: 按 `(image_type, compression)` 分支：Static+Lossy / Animated+Lossy 均跳过并返回 skipped，避免有损图被当作无损压成 JXL 造成二代损耗与体积膨胀；仅 Static+Lossless → JXL、Animated+Lossless → MP4 执行转换。
- **转换失败后清理残留**: `execute_conversion` 与 `simple_convert` 在 `result.is_err()` 时调用 `std::fs::remove_file(&output_path)`，避免不完整输出导致后续“已存在”跳过。

### 14.2 P2 — 设计

- **ConversionStrategy::command**: 类型改为 `Option<String>`，NoConversion 为 `None`，调用方用 `command.as_deref().unwrap_or("")` 或 `command.as_ref().map(|s| s.as_str())` 展示。
- **负数 size_reduction 消息**: 成功时根据 `size_reduction >= 0` 显示 “size reduced X%” 或 “size increased X%”，避免 “size reduced -12%” 的语义错误。
- **convert_to_jxl vs convert_to_jxl_lossless**: 增加注释说明前者供 execute_conversion（effort 7），后者供 simple_convert（effort 9、--modular），避免误用。

### 14.3 P3 — 逻辑与测试

- **NoConversion 只在一处处理**: 扩展名 match 中 NoConversion 改为 `unreachable!("handled above")`，避免重复分支。
- **测试**: 新增 NoConversion 时 `command.is_none()`、execute_conversion 在输出已存在时返回 skipped、simple_convert 对 Static+Lossy 跳过行为与消息的断言。

---

## 15. img_av1 lossless_converter.rs 审计修复

### 15.1 P1 — 正确性

- **convert_to_jxl 变量遮蔽**: 将第一次 `result` 重命名为 `cmd_result`，fallback 分支中 `Err(_) => result` 改为 `Err(_) => cmd_result`，避免遮蔽导致误读。
- **convert_to_av1_mp4_lossless**: 移除 `-crf 0`，仅保留 `-svtav1-params lossless=1:lp=...`；SVT-AV1 中 `lossless=1` 才是数学无损，`-crf 0` 与 lossless 并存存在冲突风险。
- **calculate_matched_crf_for_animation 调用处**: 删除多余的 `as f32`（函数已返回 f32）。

### 15.2 P2 — 设计

- **create_dir_all 一致**: 在 convert_jpeg_to_jxl、convert_to_avif、convert_to_avif_lossless、convert_to_av1_mp4、convert_to_av1_mp4_lossless、convert_to_av1_mp4_matched 中，在写入 output 前对 `output.parent()` 调用 `fs::create_dir_all(parent)`。
- **失败后清理残留**: convert_jpeg_to_jxl、convert_to_avif、convert_to_avif_lossless、convert_to_av1_mp4、convert_to_av1_mp4_lossless 在“进程成功但业务失败”或“进程失败”分支中调用 `fs::remove_file(&output)`；convert_to_av1_mp4_matched 在 explore 返回 Err 时同样清理 output。

### 15.3 P3 — 代码质量

- **distance 精度**: convert_to_jxl 的 distance 格式由 `{:.1}` 改为 `{:.2}`，与 convert_to_jxl_matched 一致。
- **prepare_input_for_cjxl JPEG**: 为 SOI 两字节检查添加注释，说明此处仅为 SOI 校验，detect_real_extension 可能已做更完整的魔法字节检测。

---

## 16. video_explorer.rs 审计修复

### 16.1 P1 — 算法与正确性

- **Phase 2 搜索命名**: 原称 “Golden section search”，实现为每轮单点评估（`mid = low + (high - low) * PHI`），非标准 GSS 的双内点。已改为 “Phi-based single-point search” 并注明非完整 golden-section，避免误导。
- **为何不用完整黄金分割搜索**: 完整 GSS 维护两个内点并在缩区间时复用其中一个，因此也是每轮 1 次评估（首轮 2 次），总评估次数理论最优。当前实现每轮从区间左端按 PHI 取单点，实现简单、无需维护“保留哪一侧内点”的状态，每轮同样 1 次编码；整段 Phase 2 可能多 1～2 次编码，换得代码更简单、易维护。若将来需要极致减少编码次数，可再实现完整 GSS。
- **SSIM 计算时机**: 在 `explore_size_only` 中，`calculate_ssim()` 基于 `self.output_path`；已加注释说明必须在当前编码输出仍在原位时调用，避免隐式状态被破坏。

### 16.2 P2 — 设计

- **三个构造函数**: 抽取私有 `build(input, output, encoder, vf_args, config, use_gpu: Option<bool>, preset, max_threads)`，`new` / `new_with_gpu` / `new_with_preset` 均调用 `build`，消除约 30 行重复初始化。
- **LONG_VIDEO_THRESHOLD 重复**: 删除重复常量 `LONG_VIDEO_THRESHOLD`，统一使用 `LONG_VIDEO_THRESHOLD_SECS`。
- **Confidence**: 在 `ConfidenceBreakdown` 上增加注释：当前 explore 结果使用按模式固定的 confidence 值，breakdown 未填充。
- **explore_compress_with_quality**: SSIM 达标与不达标分支合并为单次 `best_result = Some(...)`，仅用 if/else 区分日志；不达标时注明“接受当前最佳，无更低 CRF 重试”。

### 16.3 P3 — 代码质量

- **`_best_crf_so_far`**: 重命名为 `best_crf_so_far`（去掉 `_` 前缀），因其被读写，非“有意未使用”。
- **prop_duration_fallback_calculation**: 原断言 `(expected_duration - (frame_count/fps)).abs() < 0.0001` 恒真；改为校验 `duration > 0` 且 `(duration * fps).round() - frame_count` 在 1 以内，具有实际覆盖意义。
- **macro shadowing**: 模块级 `progress_line!` / `progress_done!` 与各 `explore_*` 内重定义并存；已保留现状，必要时可后续抽成方法替代宏遮蔽。

---

## 17. AVIF/AV1 健康检查

- **shared_utils/avif_av1_health.rs**: 新增模块，提供 `verify_avif_health(path)` 与 `verify_av1_mp4_health(path)`，当前做最小长度校验（文件存在、大小 ≥ 32 字节）。后续可按需增加 ffprobe 或更严格的最小长度/关键帧检查。
- **img_av1 lossless_converter**: 在 `convert_to_avif`、`convert_to_avif_lossless`、`convert_to_av1_mp4`、`convert_to_av1_mp4_lossless`、`convert_to_av1_mp4_matched` 成功编码后、`finalize_conversion` 前调用对应健康检查；失败时删除输出并返回错误。

---

## 18. gpu_accel.rs 审计修复

### 18.1 P1 — 正确性

- **startup_handle 超时**: 在 kill 前先 `stop_clone.store(true, Ordering::Relaxed)`，保证停止状态一致，避免后续逻辑误判。
- **absolute_timeout**: 原 12 小时检查在 `child.wait()` 之后，无法中断已阻塞进程；已删除该段死代码，长时卡死由 HeartbeatMonitor（5 分钟）处理。
- **QualityCeilingDetector**: 粗搜索从 min_crf 向 max_crf 递增，PSNR 随 CRF 升而降，`last - prev` 常为负。原逻辑 `improvement < threshold` 几乎恒真导致误判。改为用 `change = (last - prev).abs()`，仅当 `change < plateau_threshold` 时计高原，否则重置 `plateau_count`。

### 18.2 P2 — 设计

- **skip_parallel**: 原三分支均为 `true`，`encode_parallel` 从未被调用。已将“Normal file”分支改为 `(GPU_SAMPLE_DURATION, false)`，正常文件走并行编码；日志改为 “Parallel mode”。
- **detect_internal**: `try_nvenc` / `try_qsv` 在 macOS 上也会执行，可能覆盖 VideoToolbox。已用 `#[cfg(not(target_os = "macos"))]` 包裹，仅在非 macOS 上尝试 NVENC/QSV；macOS 仅用 VideoToolbox。
- **derive_gpu_temp_extension**: 新增内部 `temp_extension_for(output, suffix)`，`derive_gpu_temp_extension` 调用 `temp_extension_for(_, "gpu_temp")`；warmup 编码使用 `temp_extension_for(output, "warmup")`，公开 API 与内部逻辑统一。

### 18.3 P3 — 代码质量

- **_VARIANCE_THRESHOLD / _calc_window_variance**: 删除未使用的 `_VARIANCE_THRESHOLD`；保留 `_calc_window_variance` 并加注释“Reserved for future variance-based early exit”，参数 `input_size` 改为 `_input_size` 避免未使用警告。
- **beijing_time_now**: 格式化输出增加 “ (UTC+8)” 后缀，避免国际用户误解为本地时间。
- **get_extra_args**: 改为 `extra_args(&self) -> &[&'static str]`，避免每次分配 `Vec`；video_explorer 等处调用改为 `extra_args()`，CPU 分支使用 `&[] as &[&str]` 保持类型一致。

---

## 19. explore_strategy.rs 审计修复

### 19.1 P1 — 正确性

- **binary_search_quality**: 阈值由硬编码 0.99 改为 `ctx.config.quality_thresholds.min_ssim`；搜索目标统一为「满足 min_ssim 的最大 CRF」（最佳压缩）。当 `result.value >= min_ssim` 时 `low = mid` 并仅在 `mid > best_crf` 时更新 best；否则 `high = mid`。返回的 best 为满足阈值下 CRF 最大的点。
- **binary_search_compress**: 返回类型改为 `Result<Option<(f32, u64, u32)>>`，当整个范围内无压缩点时返回 `None`。CompressOnlyStrategy、PreciseQualityMatchWithCompressionStrategy、CompressWithQualityStrategy 在 `None` 时用 max_crf 编码并返回 `quality_passed: false`（或等效），避免 `best_size == u64::MAX` 进入 `size_change_pct` / `output_size`。
- **PSNR→SSIM**: 在 `ssim_mapping.rs` 中新增 `psnr_to_ssim_estimate(psnr_db)` 统一公式，`explore_strategy::do_calculate_ssim` 与相关 prop test 改为调用该函数，与 gpu_accel 等 fallback 路径可共用同一估算逻辑。

### 19.2 P2 — 设计

- **prop_crf_cache_key_uniqueness**: 唯一性断言使用 `CACHE_CRF_RESOLUTION = 1.0 / CRF_CACHE_MULTIPLIER`，与缓存分辨率一致，避免误报。`prop_crf_cache_equivalence` 中 HashMap key 由 `crf * 40.0` 改为 `crf * CRF_CACHE_MULTIPLIER` 与实现一致。
- **SSIM 缓存**: 在 `do_calculate_ssim` 上增加注释：SSIM 依赖当前磁盘上的 output_path，缓存 key 为 CRF，仅在 encode(crf) 后立即调用且未覆盖 output 时有效。
- **SsimCalculationResult / SsimDataSource**: 增加 `#[deprecated]`，建议直接使用 `SsimResult` / `SsimSource`。
- **ExploreContext::new**: 增加文档注释，说明 9 参数构造，建议后续可考虑 builder。

### 19.3 P3 — 代码质量

- **confidence_detail**: `build_result` 中 `ConfidenceBreakdown::default()` 旁加注释「not filled; confidence is the fixed value above」。
- **strategy_name**: 删除自由函数 `strategy_name(mode)`，从 lib 导出中移除；测试改为仅断言 `strategy.name().is_empty() == false`。
- **description()**: 六种 Strategy 的 `description()` 返回值由中文改为英文，与代码风格一致。

---

## 20. video_quality_detector.rs 审计修复

### 20.1 P1 — 正确性

- **estimate_content_type**: 增加 `fps` 参数；新增 Gaming（fps ≥ 50、1080p/720p、bpp 0.08–0.5）、LiveAction（bpp 0.05–0.6、非 Intermediate），典型电影/剧集不再全部落为 Unknown。
- **to_quality_analysis**: GOP 无数据时用帧率相关默认值 `(fps * 2.5).clamp(12, 250)` 替代硬编码 60；色彩空间无数据时按分辨率区分：`height ≤ 576` 用 `bt601`，否则 `bt709`，符合 SD/HD 约定。
- **ChromaSubsampling::from_pix_fmt**: 在宽泛的 `yuv` 兜底前显式处理 `411`（→ Yuv422）、`410`（→ Yuv420），避免 yuv411p/yuv410p 被误判为 Yuv420；并增加 yuv411p/yuv410p 的单元测试。

### 20.2 P2 — 设计

- **VideoRoutingDecision**: 删除与顶层重复的 `should_skip`、`skip_reason`，调用方统一从 `VideoQualityAnalysis.should_skip` / `skip_reason` 读取；本模块内测试改为使用 `result.should_skip`。
- **calculate_quality_score**: 使用 `bpp` 在 Standard/HighQuality 档内做小幅调整（bpp_tweak），同档内不同 bpp 质量分数有区分。
- **b_frame_count**: 在结构体字段上注明「Estimated from has_b_frames only (0 or 2); not from actual ffprobe B-frame count」。
- **make_video_routing_decision**: 删除未使用的 `_is_modern` 参数。
- **estimate_crf_from_bpp**: 对超高 BPP 增加下界：`adjusted_bpp > 5` 时返回 CRF 14，`> 1` 时 18，避免极高 bpp 与 1.x 混同。

### 20.3 P3 — 代码质量

- **_pixels**: 删除未使用的 `_pixels` 局部变量。
- **test_strict_legacy_never_skip**: 注释与断言文案改为「Non-modern (Legacy or Inefficient) codec」，变量名改为 `non_modern_codecs`，避免 mjpeg（Inefficient）被误称为 Legacy。
- **analyze_video_quality**: 增加文档注释，建议参数过多时考虑使用结构体（如 VideoQualityInput）以降低参数顺序错误风险。

---

## 21. 深挖审计：正确性与设计潜在问题

以下为对 shared_utils 及关联路径的深挖结论：除已在前述章节修复的内容外，发现的潜在问题与已确认安全点如下。

### 21.1 正确性 — 潜在问题

- **image_quality_detector 测试辅助函数**  
  - `create_checkerboard`：`block_size == 0` 会除零。已改为 `let block_size = block_size.max(1)`，避免误用导致 panic。  
  - `create_gradient`：`width` 或 `height` 为 0 会除零。已改为使用 `w = width.max(1)`、`h = height.max(1)` 参与除法。

- **dynamic_mapping 校准后使用 `anchors[0]`**  
  仅在 `gpu_size > 0 && cpu_size > 0` 时 `add_anchor`，故 `calibration_success == true` 时 `anchors[0].gpu_size` 必为正，当前逻辑安全。若将来放宽 `add_anchor` 条件，需在 412–421 行使用 ratio 前再次保证 `gpu_size > 0`，避免除零。

- **image_metrics / quality_matcher / precheck / ffprobe**  
  - PSNR/MSE、SSIM 分母（pixel_count、n、denominator、used_weight_sum）均有零值检查或常数分母。  
  - `parse_frame_rate`、precheck 中 fps/duration 解析均对 `den > 0` 或等效条件做了防护。  
  - `video_bitrate()` 中 `pixels = w*h`、`fps > 0` 已校验。  
  **结论**：当前实现下除零风险已覆盖。

### 21.2 设计 — 观察与建议

- **video_explorer / gpu_accel**  
  - `probe_crfs` 固定为 3 元组，`probe_crfs[0/1/2]` 访问安全。  
  - 失败路径上 vid_hevc/vid_av1 conversion_api 与 animated_image 均存在「删除输出 + copy_on_skip_or_fail」的清理逻辑，行为一致。

- **conversion.rs `get_file_dimensions`**  
  使用 `parts.len() >= 2` 后再访问 `parts[0]`、`parts[1]`，解析失败不 panic，设计合理。

- **checkpoint**  
  `normalize_path` 使用 `canonicalize().ok().and_then(...).unwrap_or_else(...)`，非 UTF-8 或不可 canonicalize 的路径会回退到 `path.display().to_string()`，不会因路径编码导致 panic。

- **metadata/exif preserve_internal_metadata_fallback**  
  失败时对 temp 文件有恢复或清理（rename 回退、emergency copy、remove_file），逻辑完整。

### 21.3 代码质量 — 建议

- **unwrap/expect 分布**  
  video_quality_detector 中数量较多（多为测试或内部构造）；quality_matcher、xmp_merger、checkpoint 等也有使用。建议：对「用户可控输入或外部文件」路径继续优先使用 `Result` + `?`，并在错误中附带路径/上下文。

- **类型转换**  
  大量 `as usize`/`as u32`/`as f64` 用于索引或数值计算。当前未发现明显溢出或截断导致的逻辑错误；若后续引入极端分辨率或超大 duration，建议对「宽高/帧数/时长 → 整数」的转换做范围检查或 saturating 语义。

- **浮点比较**  
  已有不少 `> 0.0`、`< 1e-10` 等阈值判断，未发现直接用 `==` 比较未约束浮点数的关键逻辑；保持现状即可。

### 21.4 优先级汇总（本节）

| 优先级 | 项目 | 建议 |
|--------|------|------|
| 已修复 | create_checkerboard / create_gradient | 已用 max(1) 防护 block_size/width/height 为 0 的除零 |
| 信息 | 其余除零/索引/清理路径 | 已核查，当前实现安全 |
| 信息 | unwrap/expect、as 转换 | 新代码继续保守处理；极端输入时补充范围检查 |

---

## 22. progress.rs — 正确性与设计审计（非安全）

**范围**: `shared_utils/src/progress.rs`，仅审查正确性、除零、API 一致性、设计重复与边界情况。

### 22.1 已修复

- **DetailedCoarseProgressBar SSIM 0.0 歧义**  
  原先用 `current_ssim == 0` 表示“未设置”，但 `0.0f64.to_bits() == 0`，导致真实 SSIM 0.0 被当成 None。已改为增加 `has_ssim: AtomicBool`，仅在有 `Some(ssim)` 时置 true 并写入 `current_ssim`；读取时用 `has_ssim` 区分“未设置”与“0.0”。

- **truncate_filename 边界**  
  当 `max_len <= 3` 时，`(max_len - 3) / 2` 在 `max_len < 3` 时在 `usize` 下会下溢。已改为：`filename.len() <= max_len` 直接返回；`max_len <= 3` 时返回 `".".repeat(max_len)`；否则再按“前半...后半”截断。

### 22.2 正确性 — 已核查

- **除零**  
  `CoarseProgressBar::render` 使用 `total.max(1)`；`DetailedCoarseProgressBar::render` 使用 `total_iterations.max(1)`；`ExploreLogger::calc_change` 与 `BatchProgress::stats` 对 `input_size`/`input_bytes` 为 0 有分支，无除零。

- **ETA**  
  `CoarseProgressBar` 在 `current > 0 && current < total` 时计算 ETA；`SmartProgressBar` 对 `recent_times` 仅在 `len() > 0` 时做平均，逻辑正确。

- **finish 幂等**  
  `CoarseProgressBar::finish`、`DetailedCoarseProgressBar::finish` 等通过 `is_finished.swap(true)` 实现“只执行一次”，行为正确。

### 22.3 设计 / API 说明

- **CoarseProgressBar::set_message(_msg)**  
  仅调用 `self.render()`，未把 `_msg` 写入任何字段，因此“设置的消息”不会在进度条上显示。若需求是“显示自定义消息”，需在结构体增加 message 字段并在 render 中输出；当前为未完成 API。

- **ExploreLogger::finish()**  
  使用 `best_size` 计算 `size_change`；若从未调用 `new_best()`，`best_size` 为 0，会得到“-100%”等显示。建议：要么保证调用方至少调用一次 `new_best`，要么在 `finish` 中对“从未更新过 best”做单独提示。

- **GlobalProgressManager**  
  `create_main` / `create_sub` 用 `Option` 存 bar，返回 `as_ref().expect(...)`。下一次 `create_main` 会替换掉之前的 bar，之前返回的引用即失效。调用方需注意：不要持有旧引用在替换后继续使用。

- **进度条类型较多**  
  Coarse / DetailedCoarse / FixedBottom / Explore / ExploreLogger / Batch / SmartProgressBar / GlobalProgressManager 等职责有重叠，存在重复的“完成时恢复光标”等逻辑；长期可考虑收敛为 fewer 抽象，非必须。

### 22.4 小结

- 已修复：SSIM 0.0 与 None 的歧义（has_ssim）、truncate_filename 的 max_len 边界。
- 正确性：除零与 finish 幂等已核查，无问题。
- 设计：set_message 未真正展示消息、ExploreLogger 未更新 best 时的 finish 语义、GlobalProgressManager 引用生命周期，已在文档中说明，可按需后续改进。

---

## 23. quality_verifier_enhanced 实现与 heartbeat_manager 修复

### 23.1 quality_verifier_enhanced

- **已实现**：原 0 字节占位文件已完整实现为“编码后增强质量校验”模块。
- **功能**：`verify_output_file(path, min_size)` 基础文件健康检查；`verify_after_encode(input, output, options)` 文件完整性 + 可选时长匹配、视频流存在性（ffprobe）；`VerifyOptions` / `EnhancedVerifyResult` 可配置与可扩展。
- **依赖**：复用 [crate::checkpoint::verify_output_integrity]、[crate::ffprobe::probe_video]，无循环依赖。
- **测试**：单元测试覆盖 options 默认值、不存在路径、空/小文件、result.passed() 逻辑。

### 23.2 heartbeat_manager 原子下溢修复

- **问题**：`unregister_progress_bar()` / `unregister_heartbeat()` 使用 `fetch_sub(1)`，在“注销次数多于注册次数”（如测试清理）时会发生原子下溢，`active_progress_count()` 返回 `usize::MAX`，导致 `test_progress_bar_guard` 失败。
- **修复**：`unregister_progress_bar` 与 `unregister_heartbeat` 改为仅在 `count > 0` 时用 `compare_exchange` 递减，避免下溢。
- **验证**：`cargo test -p shared_utils` 全部通过（含 heartbeat_manager 与 progress 相关测试）。

### 23.3 实际使用与代码质量（quality_verifier_enhanced）

- **实际使用**：在 `video_explorer/gpu_coarse_search.rs` 中，在最终编码完成后（输出 stream 信息打印之后）调用 `quality_verifier_enhanced::verify_after_encode(input, output, &VerifyOptions::strict_video())`，并以 `verbose_eprintln` 输出 `summary()` 与 `details`，实现编码后增强校验（文件完整性 + 时长匹配 + 视频流存在性）。
- **API 暴露**：`lib.rs` 中通过 `pub use quality_verifier_enhanced::{ verify_after_encode, verify_output_file, EnhancedVerifyResult, VerifyOptions, DEFAULT_MIN_FILE_SIZE }` 对外提供，便于 vid_hevc、img_av1 等 crate 复用。
- **代码质量**：`quality_verifier_enhanced.rs` 已加 `#[must_use]` 于 `VerifyOptions`、`EnhancedVerifyResult`；`verify_output_file` 直接返回 `verify_output_integrity` 的 `Result<(), String>`，无多余转换；语法与风格与仓库一致，clippy 对该文件无额外告警。

---

## 24. metadata/network.rs 说明

- **用途**：网络/云相关元数据**校验**（不负责拷贝）。在 `preserve_pro` / `preserve_metadata` 中，拷贝完成后调用 `verify_network_metadata(src, dst)`，检查源文件上存在的关键 xattr 在目标上是否也存在；若缺失则打印警告。
- **检查的 xattr（macOS）**：`com.apple.metadata:kMDItemWhereFroms`（下载来源）、`com.apple.metadata:kMDItemUserTags`（用户标签）、`com.apple.quarantine`（ quarantine 不参与“缺失”警告，因通常故意不拷贝）。
- **跨平台**：xattr 键为 macOS 专用，在非 macOS 上 `xattr::get` 对未知键会返回无数据，逻辑等效于不检查，不会报错。

### 24.1 依赖更新（本次）

- 已执行 `cargo update`，Cargo.lock 更新至当前 workspace 版本约束内最新（如 chrono 0.4.43 → 0.4.44）。
- workspace 中声明的版本（anyhow 1.0、thiserror 2.0、clap 4.5、indicatif 0.18、tempfile 3.25、proptest 1.10 等）保持不变，与现有测试兼容。

---

## 25. gpu_coarse_search.rs 审计修复（P1/P2）

依据审计报告对 `shared_utils/src/video_explorer/gpu_coarse_search.rs` 做了以下修正。

### 25.1 P1 正确性

- **GpuAccel::detect() 重复调用与变量遮蔽**：删除第一次 `detect()` 与 `print_detection_info()`，在 `input_size` 之后保留单次 `let gpu = GpuAccel::detect(); gpu.print_detection_info();`。
- **伪 fallback**：边界校验失败时不再“Retrying with CPU encoding”并重复调用同一 `encode_cached`；改为一次失败即返回清晰错误（`Boundary verification failed at CRF ...`）。
- **quality_wall_hit 分支**：`domain_wall_hit` 与 `quality_wall_hit` 后处理逻辑一致，合并为 `if domain_wall_hit || quality_wall_hit { ... }`，保留两变量赋值以便区分日志含义。
- **Phase 3 向下搜索步长**：由固定 0.25 改为与主路径一致的 **0.1**（`PHASE3_DOWNWARD_STEP`）；向上搜索仍用 `step_size_upward = 0.25`，向下用 0.1 以对齐主路径 fine-tune 精度。

### 25.2 P2 设计

- **H.264 CrfMapping**：两处 `VideoEncoder::H264 => CrfMapping::hevc(...)` 增加注释，说明 H.264 CRF 范围与 HEVC 一致，有意复用 HEVC 映射。
- **min_ssim 在 GIF/长视频/无时长路径生效**：GIF、长视频、无时长三条 SSIM 校验路径由硬编码 0.92 改为 `result.actual_min_ssim.max(0.92)`，使调用方传入的 `min_ssim` 生效。
- **ffprobe 只执行一次**：在 `explore_with_gpu_coarse_search` 顶层统一获取 `duration`（一次 ffprobe），传入 `cpu_fine_tune_from_gpu_boundary(..., duration)`；`cpu_fine_tune_from_gpu_boundary` 内删除重复的 ffprobe 调用。
- **stderr_temp 的 else 死分支**：删除 `if let Some(ref temp) = stderr_temp { ... } else { File::create(...) }` 中永远不执行的 else 分支。

### 25.3 未改动（P3）

- `#[allow(unused_assignments)]` / 未使用变量、以及将 `cpu_fine_tune_from_gpu_boundary` 拆分为多函数等 P3 项暂未改动，可按后续重构计划处理。

---

## 26. precheck.rs / dynamic_mapping.rs 审计修复（P1/P2）

依据审计报告对 `precheck.rs` 与 `dynamic_mapping.rs` 做了以下修正。

### 26.1 precheck.rs

- **BPP 计算**：改为使用视频流大小（`stream_size::extract_stream_sizes` 的 `video_stream_size`），与 `video_quality_detector` 的“视频流码率/像素”一致；无法取得流大小时回退到文件总大小。
- **huffyuv**：从 `LEGACY_CODECS_STRONGLY_RECOMMENDED` 中移除；HuffYuv 为无损，与 video_quality_detector 的 Lossless/FFV1 路由一致，不再建议“强烈升级到有损”。
- **is_hdr**：仅当色域为 BT.2020（或 2020）**且** 传输函数为 PQ/HLG（smpte2084、arib-std-b67、pq、hlg）时判为 HDR；不再以 10-bit 或像素格式单独判 HDR，避免 ProRes/DPX 等 SDR 被误标。`ffprobe_json` 增加 `color_transfer` 字段以支持判断。
- **FPS 240 边界**：`FpsCategory::from_fps` 中 Normal 改为 `fps < 240`，使 240 fps 归入 Extended；描述更新为 "1–239 fps" / "240–2000 fps"。
- **CannotProcess**：`run_precheck` 在推荐为 `CannotProcess` 时改为 `bail!(...)`，调用方可正确收到错误并中止，不再静默继续。
- **冗余 to_lowercase**：`get_video_info` 中 codec 已由 `get_codec_info` 转为小写，压缩率判断改为直接 `codec.contains(...)`。
- **单次 ffprobe**：`get_video_info` 改为只执行一次 ffprobe（`run_precheck_ffprobe`：`-show_entries stream=codec_name,width,height,r_frame_rate,duration,nb_frames,bit_rate,color_space,color_transfer,pix_fmt,bits_per_raw_sample` + `format=duration`，`-of json`），从同一份 JSON 解析 codec、宽高、时长/帧率/帧数、码率、色彩信息；时长仍按 stream.duration → format.duration → frame_count/fps → ImageMagick 回退，仅在无时长时调用 ImageMagick。删除仅被 precheck 使用的 `get_codec_info`、`get_bitrate`、`extract_color_info`（precheck 内解析 color）；`detect_duration_comprehensive` 保留为公共 API 供其他调用方使用。

### 26.2 dynamic_mapping.rs

- **AV1 CRF 上界**：`gpu_to_cpu` 增加参数 `max_crf`；调用方（gpu_coarse_search）对 AV1 传 63.0、HEVC/H264 传 51.0，避免 AV1 高 CRF 被截断到 51。
- **size_ratio ≥ 1.0**：`calculate_offset_from_ratio` 在 CPU 输出大于等于 GPU 时返回偏移 0，不再使用 +2.5。
- **AV1 路径 vf_args**：非 GIF/非 HEVC 的 CPU 校准中，将多个滤镜用 `;` 拼接为单条 `-vf` 参数，避免重复 `-vf` 导致后者覆盖前者。
- **多锚点分支**：在 `gpu_to_cpu` 多锚点插值处增加注释，说明当前 `quick_calibrate` 首次成功即 break，多锚点分支为未使用代码。
- **temp 文件**：删除与 RAII 重复的手动 `fs::remove_file`，仅依赖 `NamedTempFile` 的 Drop 清理。
- **P3**：删除未使用的 `_offset` 计算；`add_anchor` 中 `self.calibrated = !self.anchors.is_empty()` 改为 `self.calibrated = true`。

### 26.3 P3 收尾（precheck / dynamic_mapping）

- **precheck**  
  - **calculate_bpp 轻量路径**：不再通过完整 `get_video_info` 取 bpp；改为一次 `run_precheck_ffprobe` + 私有 `bpp_from_precheck_json` 仅解析并计算 bpp（仍 1 次 ffprobe，不构建 recommendation/compressibility/color 等）。  
  - **evaluate_processing_recommendation**：唯一调用方 `get_video_info` 已传入小写 codec，去掉冗余 `codec.to_lowercase()`，改为直接使用 `codec`，并注明「caller must pass lowercase codec」。  
- **dynamic_mapping**：P3 已在 26.2 中完成（`_offset` 已删、`calibrated = true`）。

---

## 27. ssim_calculator.rs / stream_analysis.rs / precision.rs 审计修复

依据审计报告对上述三个文件做了以下修正。

### 27.1 P1 正确性

- **ssim_calculator：MS-SSIM 并行 stderr 交错**：Y/U/V 三路不再在子线程内 `eprint!`/`eprintln!`；子线程只返回 `Option<f64>`，主线程在 `join` 后统一打印 `Y channel... {:.4} ✅` 等，避免输出交错、通道对应关系混乱。
- **ssim_calculator：weighted_avg**：保留 6:1:1 权重，在旁加注释说明为 BT.601 近似，且 chroma 平面在 4:2:0 下 MS-SSIM 可能系统性偏低。
- **precision：CACHE_KEY_MULTIPLIER 与 CrfCache 一致**：`precision::CACHE_KEY_MULTIPLIER` 改为使用 `crf_constants::CRF_CACHE_KEY_MULTIPLIER`（100.0），与 `explore_strategy::CrfCache`、`Crf::to_cache_key` 的 key 一致；`crf_to_cache_key` 对非有限或负 CRF 返回 0，对超过 `CRF_CACHE_MAX_VALID` 的 CRF 截断后再算 key；`cache_key_to_crf` 对 key≤0 返回 0。相关单元测试与属性测试的期望值已按 0.01 分辨率更新（如 20.0→2000）。
- **stream_analysis：get_video_duration**：在路径参数前增加 `--`，与同文件其他 ffprobe 调用及 precheck 一致，避免路径以 `-` 开头时被当成选项。

### 27.2 P2 设计

- **ssim_calculator：MS-SSIM 时长阈值**：与 `gpu_coarse_search::VMAF_DURATION_THRESHOLD`（5 分钟）对齐，仅对 ≤5 分钟视频计算 MS-SSIM（原 5–30 分钟分支删除），避免与调用方策略不一致。
- **ssim_calculator：calculate_ms_ssim 与 _yuv**：在模块头注释中说明 `calculate_ms_ssim_yuv` 为 gpu_coarse_search Phase 3 主入口，`calculate_ms_ssim` 为单通道 luma + standalone vmaf fallback，供其他调用方使用。
- **precision：required_iterations**：使用 `max_crf.saturating_sub(min_crf)` 避免 u8 下溢；若 range≤0 直接返回 1。
- **precision：ssim_quality_grade / ms_ssim_quality_grade**：增加文档注释，说明返回值含中英文混排，勿用于等宽终端对齐（string len ≠ display width）。
- **precision：ThreePhaseSearch**：增加文档注释，说明与 `SearchPhase::step_size()` 一致、用于可选运行时覆盖。

### 27.3 P3

- **ssim_calculator**：去掉 "(Beijing)" 和 "End: ... Beijing"，改为使用系统本地时间的 "Start time"/"End" 显示。
- **stream_analysis**：删除本文件内未使用的 `CrossValidationResult` 枚举。

---

## 28. 日志统一为 tracing（审计建议）

审计意见：「模块大量使用 eprintln! 进行日志输出。虽然符合 CLI 工具特性，但如果能统一集成到 tracing 结构化日志中会更利于维护和调试。」

### 28.1 已做修改

- **progress_mode**：新增 `emit_stderr(line)`，内部使用 `tracing::info!` 输出到 stderr；`quiet_eprintln!`、`verbose_eprintln!`、`log_eprintln!` 以及 XMP 相关计数/失败/汇总的 stderr 输出均改为经 `emit_stderr`，在已初始化 tracing 时走结构化日志（stderr + 可选文件）。
- **logging (init_logging)**：默认 `EnvFilter` 改为同时允许主程序与 `shared_utils`（例如 `vid_hevc=INFO,shared_utils=INFO`），保证库内 tracing 事件能输出到 stderr/文件。
- **ffprobe_json**：`extract_color_info` 中的 FFPROBE 错误由 `eprintln!` 改为 `tracing::warn!`（带 `input`/`error` 等字段）。
- **stream_analysis**：SSIM 计算相关提示由 `eprintln!` 改为 `tracing::info!` / `tracing::warn!` / `tracing::error!`。
- **precheck**：时长回退提示、Precheck Report 行、run_precheck 的提示由 `eprintln!` 改为 `tracing::info!` / `tracing::warn!` / `tracing::error!`；Report 仍按行逐条 `info!` 以便检索。

### 28.2 日志格式与缩进（美观化）

- **文件日志**：去掉 `with_thread_ids` 和 `with_line_number`，使每行前缀宽度稳定（仅 timestamp + level + target），**消息正文左对齐**，便于阅读和检索。写入文件前经 **StripAnsiWriter** 去除 ANSI 转义（如 `\x1b[92m`），避免日志文件中出现乱码式原始控制码。
- **Stderr 统一缩进**：`progress_mode::emit_stderr` 对所有非空行增加固定 **2 空格** 前导（`STDERR_INDENT`），多行块（如 Precheck Report、XMP 汇总）整体缩进一致。
- **Stderr 非 TTY 时去 ANSI**：当 stderr 被重定向或脚本捕获时（非 TTY），`emit_stderr` 会先对消息做 `strip_ansi_str` 再输出，避免出现 `\x1b[92m` 等乱码；终端内仍保留颜色。
- **终端降噪**：`target: "gpu_detection"` 的日志（如「GPU: Apple VideoToolbox」）仅写入文件层，stderr 层用 `FilterFn` 排除该 target，减少终端吵闹信息。
- **Tag 列宽**：`LOG_TAG_WIDTH` 由 34 调整为 24，带 tag 的日志（如 `[file.jpeg]`、`[XMP]`）消息列仍对齐，左侧留白更紧凑。

### 28.3 使用说明

- 各二进制（vid_hevc、vid_av1、img_hevc、img_av1）已在启动时调用 `shared_utils::logging::init_logging`，tracing 事件会写入 stderr 与日志文件。
- 可通过 `RUST_LOG` 控制级别（如 `RUST_LOG=debug`、`RUST_LOG=shared_utils=info`）。
- 未调用 `init_logging` 的场合（如仅链接 shared_utils 的测试或工具），tracing 事件不会输出；原有 `write_to_log` 的日志文件写入逻辑保留，与 tracing 并行。

### 28.4 图片转换 compress 判断统一

- **目标**：所有图片转换在「编码成功、取得 output_size 后、finalize 前」统一走 `check_size_tolerance`，当 `options.compress` 为 true 时仅接受 output < input，否则跳过并保留原文件。
- **已覆盖路径**（img_hevc lossless_converter）：`convert_to_jxl`、`convert_jpeg_to_jxl`（含 ImageMagick fallback）、`convert_to_avif`、`convert_to_avif_lossless`、`convert_to_jxl_matched`。动图→HEVC 仍走 vid_hevc 的 size/compress 逻辑。

---

## 29. CLI 重复与管道错误处理（审计建议）

### 29.1 代码重复 (vid_hevc / vid_av1 main.rs)

- **现象**：vid_av1 与 vid_hevc 的 `main.rs` 中 `run` 命令的 base_dir 解析逻辑完全一致。
- **已做**：在 `shared_utils::cli_runner` 中新增 `resolve_video_run_base_dir(input, recursive, base_dir_override)`，vid_hevc 与 vid_av1 的 Run 分支改为调用该函数，减少样板代码；后续若将更多 run 逻辑（如 flag 校验、banner）抽象到 shared_utils 可继续复用。

### 29.2 管道错误处理 (x265_encoder.rs)

- **现象**：ffmpeg 解码 → 管道 → x265 编码 的管道拷贝由线程执行，BrokenPipe 时难以区分是解码端还是编码端先退出。
- **已做**：先 join 管道拷贝线程并捕获 `io::copy` 的 `Result`；若为 `BrokenPipe` / `ConnectionReset` 则记入 `is_broken_pipe`。在 FFmpeg 失败或 x265 失败的分支中增加 `pipe_broken` 字段与 warn 提示（「Pipe broken: decoder (ffmpeg) likely exited first」或「encoder (x265) likely exited first」），便于日志中区分是编码器崩溃还是解码器崩溃。若两进程均成功但管道拷贝返回 I/O 错误，则单独打 error 并 bail。

### 29.3 并发资源控制 (gpu_accel encode_parallel)

- **现象**：`encode_parallel` 会并发启动多个 FFmpeg GPU 探针；显存小或 Session 受限（如 NVIDIA 消费级 3–5 路）或 CPU 核数极多时易失败。
- **已做**：引入全局并发上限，由环境变量 `MODERN_FORMAT_BOOST_GPU_CONCURRENCY` 配置（默认 4）。`gpu_accel` 内用 `Mutex<usize>` + `Condvar` 实现信号量，`encode_parallel` 中每个探针线程先 `acquire_gpu_slot()`，结束时由 `GpuSlotGuard` 的 drop 调用 `release_gpu_slot()`，从而限制同时进行的 GPU 编码/探针数量。

### 29.4 VAAPI 设备路径可配置

- **现象**：shared_utils 中 VAAPI 设备硬编码为 `/dev/dri/renderD128`，多显卡 Linux 可能使用不同节点。
- **已做**：新增 `vaapi_device_path()`（仅 `#[cfg(target_os = "linux")]`），优先读 `MODERN_FORMAT_BOOST_VAAPI_DEVICE`，其次 `VAAPI_DEVICE`，缺省为 `/dev/dri/renderD128`。hevc_vaapi / av1_vaapi / h264_vaapi 的 `extra_args` 均改为使用该函数。

---

## 30. 正确性与健壮性后续修复（GIF / rsync / processed list）

### 30.1 GIF 解析器越界防护 (image_formats.rs)

- **现象**：`gif::count_frames_from_bytes` 在解析 Image Data / Extension 块时，`pos += block_size` 后未检查 `pos` 是否越界即进入下一轮读取 `data[pos]`，截断或恶意 GIF 可能导致 panic。
- **已做**：在两处“跳过数据块”的循环中，在 `pos += block_size` 前增加 `if pos + block_size > data.len() { break; }`，越界时提前退出循环，避免 panic。

### 30.2 rsync 路径查找 (thread_manager.rs)

- **现象**：`get_rsync_path()` 使用 macOS Homebrew 硬编码路径 `/opt/homebrew/...`，非 macOS 上为死逻辑。
- **已做**：改为使用 `which::which("rsync")` 查找可执行路径，未找到时回退为 `"rsync"`；移除硬编码路径，跨平台一致。

### 30.3 已处理列表文件锁 (conversion.rs)

- **现象**：`load_processed_list` / `save_processed_list` 无文件锁，多进程（如多个 img-av1 实例）同时读写同一列表文件可能导致损坏或竞态。
- **已做**：在 **Unix** 上对列表文件使用 advisory `flock(LOCK_EX)`：加载/保存前加锁，使用 `ProcessedListLockGuard(RawFd)` 在 drop 时解锁，保证读/写期间独占；`libc::flock` 调用包在 `unsafe` 块并保留 fd 于 guard 内，避免与 `file` 可变借用冲突。

---

## 31. 启发式提前终止阈值 (video_explorer Stage A)

- **现象**：Stage A 二分搜索中，方差阈值 `0.00001` 与变化率阈值 `0.005` 为硬编码；比特率曲线极平缓的视频（如静态画面多）在少数几次迭代后即满足“方差收敛”或“变化率极小”，导致过早提前终止，未充分搜索 CRF 范围。
- **已做**：
  - **最小迭代数**：新增 `MIN_ITERATIONS_BEFORE_VARIANCE_EXIT = 6`，方差或变化率提前终止仅在 `iterations >= 6` 时允许，避免 3～4 个样本就退出。
  - **方差阈值**：由 `0.00001` 改为 `1e-6`，仅在窗口内 size ratio 几乎恒定时才判定收敛。
  - **日志**：提前终止日志改为英文并输出迭代次数，便于排查。

---

## 32. SSIM 抽取片段时长 (gpu_accel / dynamic_mapping / video_explorer)

- **目的**：增多 SSIM 调取时抽取片段的时长，更好适配不同类型媒体（如静态多、场景变化多的视频）。
- **已做**：
  - **gpu_accel.rs**：`GPU_SAMPLE_DURATION` 50s → 60s；`GPU_SEGMENT_DURATION` 10s → 15s（多段采样时每段 15s，5 段共 75s）。
  - **dynamic_mapping.rs**：校准与提取时 `sample_duration.min(10.0)` → `sample_duration.min(15.0)`，与段长一致。
  - **video_explorer.rs**（MS-SSIM 长视频）：3 段采样由 10% 改为 15%（start 15% + mid 15% + end 15%），每段时长增加 50%。

---

## 33. 极限模式下调取时长同步增加 (Ultimate mode longer SSIM segments)

- **目的**：极限模式追求极致质量，SSIM 调取时的抽取片段时长同步增大，与模式目标一致。
- **已做**：
  - **gpu_accel.rs**：新增 `GPU_SAMPLE_DURATION_ULTIMATE = 90`、`GPU_SEGMENT_DURATION_ULTIMATE = 25`；`GpuCoarseConfig` 增加 `ultimate_mode: bool`。正常文件下 ultimate 用 90s 总采样与 25s/段（5 段共 125s）；大文件 45s→70s、超大文件 30s→50s。
  - **gpu_coarse_search.rs**：构建 `GpuCoarseConfig` 时传入 `ultimate_mode`；Phase 1 样本时长与校准用 `sample_dur`（ultimate 时取 ULTIMATE 常量）。
  - **video_explorer.rs**：`calculate_ms_ssim` 长视频 3 段采样在 `ultimate_mode` 下由 15% 改为 25%（start/mid/end 各 25%）。

---

## 34. 极限模式下 MS-SSIM 跳过阈值延长（25 分钟）

- **目的**：正常模式 >5 分钟跳过 MS-SSIM（成本/质量权衡）；极限模式使用更长时长参数，仅当视频 >25 分钟才跳过，以加强质量验证。
- **已做**：
  - **gpu_coarse_search.rs**：新增 `VMAF_DURATION_THRESHOLD_ULTIMATE_SECS = 1500`（25 min）。`should_run_vmaf` 在 ultimate 下用 25 min 阈值，日志显示「≤5min」或「≤25min」；`calculate_ms_ssim_yuv` 传入 `max_duration_min`（5.0 或 25.0）。
  - **video_explorer.rs**：新增 `MS_SSIM_SKIP_THRESHOLD_ULTIMATE_SECS = 1500`；`validate_quality` 中 MS-SSIM 跳过阈值在 ultimate 下为 25 min，否则 5 min；日志中显示对应阈值。
  - **ssim_calculator.rs**：`calculate_ms_ssim_yuv` 增加参数 `max_duration_min`，由调用方传入 5.0（正常）或 25.0（极限），跳过逻辑与提示文案据此动态显示。
- **文档**：YUV 加权与采样策略仍为「<1 分钟全帧，1–5 分钟 1/3，>5 分钟跳过」；极限模式下「>25 分钟才跳过」由上述三处阈值控制，见本段与代码注释。

---

## 35. 极端 BPP 的防御性设计（quality_matcher）

- **目的**：在超低/超高 effective BPP 或非有限值（NaN/Inf）输入下，保证 CRF 公式不产生无效结果，且输出 CRF 始终落在编码器合法范围内。
- **已做**：
  - **effective_bpp 前置检查**：`effective_bpp <= 0` 返回 Err；`!effective_bpp.is_finite()` 返回 Err，避免 log2 与公式收到非有限或零。
  - **effective_bpp 安全区间**：在代入 CRF 公式前将 `effective_bpp` 钳位到 `[SAFE_BPP_MIN, SAFE_BPP_MAX]`（1e-6～50），保证 `log2(effective_bpp * 100)` 有限且合理。
  - **CRF 输出钳位**：AV1 使用 `AV1_CRF_CLAMP_MIN/MAX`（15～40），HEVC 使用 `HEVC_CRF_CLAMP_MIN/MAX`（0～35），作为最后一层防护，确保 content-type 与 bias 调整后仍不越界。
- **文档**：模块头新增「Extreme BPP (defensive design)」说明；常量与最终 clamp 处均有注释。

---

## 36. 文件复制策略与扩展名「先修正再校验」设计

### 36.1 复制逻辑（程序内，非 rsync）

- **位置**：全部在程序内完成。`shared_utils::smart_file_copier`（`copy_on_skip_or_fail`、`smart_copy_with_structure`）与 `shared_utils::file_copier`（`copy_unsupported_files`、`verify_output_completeness`）。**未使用 rsync**；`thread_manager::get_rsync_path` 仅作探测，当前复制均为 Rust std::fs + 目录遍历。
- **无遗漏设计**：
  1. **单文件/批量转换失败或跳过**：`copy_on_skip_or_fail(source, output_dir, base_dir)` 将源文件按目录结构复制到输出目录，保证「未成功转换的也出现在输出里」。
  2. **批量结束后**：`copy_unsupported_files(input_dir, output_dir, recursive)` 遍历输入目录，将「扩展名不在支持列表」的文件复制到输出目录（支持列表外的都复制，避免遗漏）。
  3. **校验**：`verify_output_completeness(input_dir, output_dir, recursive)` 比较输入/输出文件数量，输出是否缺失或多余。
- **冲突**：同一路径不会既被转换写入又被 copy 覆盖——转换成功时写入的是目标路径（如 .mp4/.mov），copy_unsupported 只复制「扩展名不在 SUPPORTED_VIDEO/IMAGE 等列表」的文件；已转换文件扩展名在列表内，不会进入 copy_unsupported。唯一需注意：扩展名修正后若文件从「支持列表」变为「非支持」（如 .mp4 修正为 .gif），会在当轮被 copy 到输出而非转换，与预期一致。

### 36.2 扩展名：统一「先修正再校验」

- **原则**：所有**仅凭扩展名**做的分支（如是否视频、是否 GIF、输出命名）应在**扩展名已按内容修正后**再执行，避免伪装扩展名导致误判或 panic。
- **已做**：
  - **img_hevc / img_av1**：入口即 `fix_extension_if_mismatch(input)`，后续一律用修正后的路径。
  - **视频路径（vid_hevc / vid_av1）**：在 `cli_runner::process_single_file` 与 `process_directory` 中，**先**对每个输入调用 `fix_extension_if_mismatch`，得到 `fixed`；**再**用 `has_extension(fixed, SUPPORTED_VIDEO_EXTENSIONS)` 判断是否按视频处理。若修正后扩展名不在视频列表（如内容实为 GIF 的 .mp4 被改为 .gif），则仅复制到输出、不进入视频转换，并 continue/return。
  - **copy 时**：`smart_copy_with_structure` 在复制到目标路径后对**目标路径**调用 `fix_extension_if_mismatch(dest)`，保证写出的文件扩展名与内容一致。
- **结果**：扩展名校验统一在「修正后」进行，避免因错误/伪装扩展名走入错误分支或 panic。

### 36.3 修正扩展名时动图与视频流是否混淆

- **不会**。`smart_file_copier::detect_content_format` 仅根据文件头魔数识别：jpeg、png、gif、webp、heic、avif、tiff、jxl 等**图片/容器**格式；**不识别** MP4/MOV/MKV 等视频容器。因此：
  - 动图（GIF/WebP/AVIF）只会被修正为 .gif / .webp / .avif，不会变成 .mp4/.mov；
  - 真实视频文件若扩展名错误，`detect_content_format` 可能返回 `None`，不会误判为动图格式。
- 动图与视频流的区分在「扩展名修正」阶段不会混淆；后续路由（按扩展名走视频或图片流程）在修正后执行，逻辑一致。

---

## 37. GIF 排除 Apple 兼容 fallback（校验）

- **策略**：GIF 作为源没有任何苹果兼容问题，**完全不应**进入「Apple compat fallback」逻辑。转换失败时：不保留 best-effort HEVC，直接删除输出、将**原始文件**复制到目标目录并返回失败。
- **校验**：
  - **vid_hevc/conversion_api.rs**：`source_is_gif = input_ext.eq_ignore_ascii_case("gif")`；三处 fallback（quality/size 未达标、MS-SSIM 未达标、压缩检查未通过）均为 `if config.apple_compat && !source_is_gif` 才进入「保留 HEVC 并返回成功」。GIF 时跳过该分支，走统一失败路径：删输出、`copy_on_skip_or_fail(input, ...)`、返回 `success: false`。
  - **vid_hevc/animated_image.rs**：动图→视频入口已移除「apple_compat 时保留 best-effort 输出」分支；失败时一律删输出、`copy_on_skip_or_fail`、返回 `success: false, skipped: true`。
- **结论**：GIF 不会出现「APPLE COMPAT FALLBACK」提示，也不会在失败时保留 HEVC；仅复制原文件到目标目录。

---

## 38. 视频处理策略总览（含 ProRes、动图→视频）

### 38.1 入口与文件归属

- **vid_hevc（视频 CLI）**：输入按**扩展名**收集，`SUPPORTED_VIDEO_EXTENSIONS` = mp4, mov, avi, mkv, webm, m4v, wmv, flv, mpg, mpeg, ts, mts。**不含** gif/webp/avif（属图片扩展名）。因此「动图」若扩展名为 .gif/.webp 等，**不会**被 vid_hevc 的 run 模式收集，不会走视频管线。
- **img_hevc（图片 CLI）**：输入按 `SUPPORTED_IMAGE_EXTENSIONS` 收集（含 png, jpg, webp, gif, heic, avif 等）。**动图→视频**由此入口处理：分析为动图后调用 `vid_hevc::animated_image`（如 `convert_to_hevc_mp4` / `convert_to_hevc_mp4_matched`），或 Apple 兼容下 `convert_to_gif_apple_compat`。

### 38.2 视频策略决定（vid_hevc）

1. **detect_video(path)**：ffprobe 得到 codec、码率、分辨率、帧率等，并算得 **CompressionType**：
   - **Lossless**：codec 为 FFV1 / Uncompressed / HuffYUV / UTVideo。
   - **VisuallyLossless**：codec 为 **ProRes / DNxHD**，或 bits_per_pixel > 2.0。
   - HighQuality / Standard / LowQuality：按 bits_per_pixel 分段。

2. **determine_strategy_with_apple_compat(detection, apple_compat)**：
   - 先做 **skip**：`should_skip_video_codec(_apple_compat)(codec)`。HEVC 在 Apple 模式下跳过；VP9/AV1 不跳过（转 HEVC）；**ProRes 不跳过**（正常转 HEVC）。
   - 再按 **CompressionType**：
     - **Lossless** → `HevcLosslessMkv`（HEVC 无损 MKV）。
     - **VisuallyLossless** → `HevcMp4`，理由如 "Source is ProRes (visually lossless) - compressing with HEVC CRF 18"，策略里 crf=18（仅 simple 模式用；auto 用 matched CRF）。
     - **其它** → `HevcMp4`，Standard/Low 等，策略 crf=20。

3. **执行**：
   - **HevcLosslessMkv**：`execute_hevc_lossless`，无 CRF 探索。
   - **HevcMp4**：`calculate_matched_crf(&detection)` 得初始 CRF（quality_matcher 按 codec/码率/分辨率等算），再 `explore_hevc_with_gpu_coarse_full(..., initial_crf, ultimate, ...)` 做 GPU 粗搜 + CPU 细搜；之后做质量/压缩校验，失败时 **非 GIF** 且 **apple_compat** 才走 Apple compat fallback（保留 best-effort HEVC），GIF 仅复制原文件（见 §37）。

### 38.3 ProRes 在当前策略下的路径

- **不跳过**：`should_skip_video_codec("prores")` 与 `should_skip_video_codec_apple_compat("prores")` 均为 **false**，ProRes 会进入转换。
- **CompressionType**：`DetectedCodec::ProRes` 在 `determine_compression_type` 中固定为 **VisuallyLossless**（与 DNxHD 一致）。
- **策略结果**：**HevcMp4**（不选 HevcLosslessMkv）。理由文案："Source is ProRes (visually lossless) - compressing with HEVC CRF 18"。
- **实际编码**：走 **HevcMp4** 分支，`initial_crf = calculate_matched_crf(&detection)`（由 quality_matcher 按 ProRes 的 bpp/分辨率等算出），再 GPU+CPU 探索；输出 HEVC MP4，做 SSIM/MS-SSIM 与压缩检查。若未达标且为 Apple 模式，走 Apple compat fallback（保留 HEVC）；ProRes 非 GIF，故会走该 fallback。

### 38.4 动图→视频（img_hevc → animated_image）

- **入口**：仅当使用 **img_hevc**（图片工具）且分析结果为**动图**（is_animated，格式 GIF/WebP/AVIF/HEIC 等）时，才进入动图逻辑；**vid_hevc** 不按扩展名收集 .gif/.webp，故不会把「未改扩展名的动图」当视频输入。
- **分支概要**：
  - **静态 GIF（1 帧）**：按静态图转 JXL。
  - **duration < 3s**：可跳过或按 Apple 兼容转 GIF（Bayer 256）。
  - **duration ≥ 3s 或高质量**：`convert_to_hevc_mp4_matched` 或 `convert_to_hevc_mkv_lossless`（若 --lossless），即调用 **vid_hevc::animated_image** 做 HEVC 探索与质量校验。
- **animated_image 内**：与 vid_hevc 的 conversion_api 类似，做 explore、SSIM/压缩校验；**不做** Apple compat fallback（§37）：失败则删输出、`copy_on_skip_or_fail`、返回失败。
- **扩展名修正**：若先经 `fix_extension_if_mismatch` 将 .mp4 改为 .gif，则该文件不会进入 vid_hevc 的视频列表，会在 copy_unsupported 或「非视频则复制」时被复制到输出（见 §36）。

### 38.5 视频是否「只按扩展名」？有无伪扩展名修复？

- **收集阶段**：是，视频文件列表按**扩展名**收集（`SUPPORTED_VIDEO_EXTENSIONS`），目录下只有扩展名在该列表中的文件才会被当作视频处理。
- **伪扩展名修复（双方向）**：
  - 在 **cli_runner** 的视频路径中，**先**对每个文件调用 `fix_extension_if_mismatch`，**再**用 `has_extension(fixed, SUPPORTED_VIDEO_EXTENSIONS)` 判断是否当视频处理（§36.2）。
  - `detect_content_format` 按**固定顺序**读文件头魔数：**先图片/动图，再视频**。识别：jpeg, png, gif, webp, heic, avif, tiff, jxl；以及 mp4, mov, avi, flv, mkv, wmv 等视频容器。
  - **不会把动图误判为视频**：GIF（GIF8）、WebP（RIFF+WEBP）、AVIF（ftyp+avif/avis）均在代码中先于视频分支判定；RIFF 先按 WEBP 区分，ftyp 先按 heic/avif 再按 isom/qt 等。因此 GIF/WebP/AVIF 动态图即使用错扩展名，也只会被修正为对应图片扩展名，不会被当作 MP4/MOV 等处理。
  - 效果：
    - **内容实为图片/动图、扩展名伪造成视频**（如 .mp4 实为 GIF）：修正为 .gif 等，不会当视频转换。
    - **内容实为视频、扩展名错误**（如 .jpg 实为 MP4）：修正为 .mp4/.mov 等，修正后可被 vid_hevc 当视频处理。
  - 结论：先修正再校验；**图片与视频的检测顺序保证动图（GIF/WebP/AVIF）不会与视频混淆**。
