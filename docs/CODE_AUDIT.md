# 代码审计报告 / Code Audit Report

**日期 / Date**: 2026-02-23  
**范围 / Scope**: modern_format_boost 仓库（重点 shared_utils、video_explorer、ffprobe、image_analyzer、progress_mode）

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

**本轮修复**: 路径安全（前轮 + vid_hevc/vid_av1 conversion_api）+ 除零保护（stream_size_change_pct、prev_size）+ progress expect + unsafe 注释 + 审计文档更新。

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
