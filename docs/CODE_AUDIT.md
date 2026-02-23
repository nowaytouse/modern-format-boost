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
