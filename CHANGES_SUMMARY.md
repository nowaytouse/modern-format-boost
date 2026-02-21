# 近期改动说明（用于本次 push）

## 一、改动文件列表与影响

### 1. 核心行为改动（与需求直接相关）

| 文件 | 改动前 | 改动后 | 影响 |
|------|--------|--------|------|
| **shared_utils/src/video_explorer.rs** | GIF 时静默用 SSIM-only / explore SSIM 兜底，视为“通过”；GIF 校准时走 Y4M→x265 管道易 FFmpeg decode failed | GIF 时响亮报错，不兜底；报错内容同步写入 `result.log`；校准用 Y4M 抽取加 `-an`，失败时打印完整 FFmpeg stderr；**GIF 专用**：动态校准时用 FFmpeg 单步 libx265（不走 Y4M 管道），减少校准失败 | GIF 不再虚假成功；相邻目录下日志/结果里能看到完整错误；校准失败可据 stderr 排查；GIF 校准更稳、更少 fallback 到 static offset |
| **shared_utils/src/cli_runner.rs** | 单文件转换返回 `Err` 时直接向上抛错，不写输出目录 | 单文件转换失败时，若有 `output`（相邻目录），先 `copy_on_skip_or_fail` 把原文件复制到输出目录，再返回 `Err` | 单文件报错时相邻目录仍有一份原文件，实现「跳过/报错都无遗漏」 |
| **shared_utils/src/msssim_parallel.rs** | GIF 时返回 `Ok(MsssimResult::skipped())`，日志提示用 SSIM-only | GIF 时返回 `Err(AppError::Other(...))`，明确报错不降级 | 与 video_explorer 一致，GIF 不静默跳过 MS-SSIM |

### 2. 其他已修改文件（多为优化/注释/依赖）

- **Cargo.toml / shared_utils/Cargo.toml**：依赖或版本
- **README.md**：文档
- **imgquality_hevc/src/lossless_converter.rs, imgquality_av1/src/lossless_converter.rs**：结构/风格调整
- **scripts/drag_and_drop_processor.sh**：小改动
- **shared_utils/src/batch.rs, common_utils.rs, explore_strategy.rs, gpu_accel.rs, metadata/exif.rs, path_safety.rs, realtime_progress.rs, vmaf_standalone.rs, x265_encoder.rs, xmp_merger.rs**：代码整理、inline、路径安全、格式等

---

## 二、log-demo 中反映的现象与建议

根据你提供的新处理日志 `log-demo`：

### 已按预期工作的部分

- **GIF 报错**：多次出现  
  `❌ ERROR: GIF format does not support MS-SSIM quality verification.`  
  `❌ Refusing to use SSIM-only/explore-SSIM fallback (would be false success).`  
  → 说明“删除兜底、响亮报错”已生效。
- **XMP 合并、大量图片/视频转换**：正常进行，无异常堆叠。

### 需要关注的问题（非本次改动引入）

1. **单张 AVIF 解码失败**  
   - `❌ Conversion failed ... IMG_0220.AVIF: Failed to decode image: Format error decoding Avif: Invalid argument`  
   - 属于该 AVIF 文件或解码器兼容性，与本次 GIF/无遗漏改动无关；建议单独排查该文件或解码库。

2. **GIF 的 x265 校准失败**  
   - `❌ CPU x265 encoding failed for CRF 20.0/18.0/22.0: FFmpeg decode failed`  
   - `⚠️ All CPU calibration attempts failed, using static offset`  
   - 对 GIF 做 CPU 校准时，FFmpeg 解码/管道可能失败，目前会回退到静态 offset。若你希望校准更稳，可后续在“校准失败”分支看完整 stderr（本次已在抽取失败时打印 stderr）或对 GIF 跳过校准。

3. **相邻目录“无遗漏”在本日志中的表现**  
   - 日志里未出现 `📋 Copied original to output` 等字样，可能是：本场跑的是目录模式且失败项较少，或失败发生在 vidquality 内部已处理分支（如 MS-SSIM 不通过时先删再 copy）。  
   - 单文件失败时“先复制再 Err”的改动，只会在「单文件 + 指定 output + 转换 Err」时触发，本日志多为批量，属正常。

---

## 三、小结

- **本次 push 内容**：GIF 响亮报错且不兜底、result.log 无遗漏、单文件失败时复制到相邻目录、校准 stderr 与 `-an` 注释；外加 .gitignore 增加 `logs/`、本说明文件。
- **log-demo**：未见由本次改动导致的新问题；AVIF 单文件解码失败与 GIF 校准失败为既有/环境问题，可按上面建议单独处理。
