# 近期改动说明（用于本次 push）

## 一、改动文件列表与影响

### 1. 核心行为改动（与需求直接相关）

| 文件 | 改动前 | 改动后 | 影响 |
|------|--------|--------|------|
| **shared_utils/src/video_explorer.rs** | GIF 时静默用 SSIM-only / explore SSIM 兜底，视为“通过”；GIF 校准时走 Y4M→x265 管道易 FFmpeg decode failed；短视频在 MS-SSIM+SSIM All 全失败时用单通道/explore SSIM 兜底当通过；长视频/无时长在 SSIM All 失败时未明确置为未通过 | GIF 时响亮报错，不兜底；报错内容同步写入 `result.log`；校准用 Y4M 抽取加 `-an`，失败时打印完整 FFmpeg stderr；**GIF 专用**：动态校准时用 FFmpeg 单步 libx265；**根除伪造成功**：MS-SSIM+SSIM All 全失败时不再用单通道/explore 兜底当通过，改为响亮报错并 `ms_ssim_passed = Some(false)`；长视频/无时长下 SSIM All 失败时明确 `ms_ssim_passed = Some(false)` 并写入 log | GIF 不再虚假成功；校准更稳；任何“验证未完成”均不再被标为通过，避免数据损失 |
| **shared_utils/src/cli_runner.rs** | 单文件转换返回 `Err` 时直接向上抛错，不写输出目录 | 单文件转换失败时，若有 `output`（相邻目录），先 `copy_on_skip_or_fail` 把原文件复制到输出目录，再返回 `Err` | 单文件报错时相邻目录仍有一份原文件，实现「跳过/报错都无遗漏」 |
| **shared_utils/src/msssim_parallel.rs** | GIF 时返回 `Ok(MsssimResult::skipped())`，日志提示用 SSIM-only | GIF 时返回 `Err(AppError::Other(...))`，明确报错不降级 | 与 video_explorer 一致，GIF 不静默跳过 MS-SSIM |

### 2. 其他已修改文件（多为优化/注释/依赖）

- **Cargo.toml / shared_utils/Cargo.toml**：依赖或版本
- **README.md**：文档
- **img_hevc/src/lossless_converter.rs, img_av1/src/lossless_converter.rs**：结构/风格调整
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

---

## 四、本轮：体积/质量日志 + CLI 与脚本默认

### 4.1 动图与视频：体积与质量指标日志

- **shared_utils/src/video_explorer.rs**（Phase 3 质量验证后）  
  - 新增每文件统一输出（同时写入 stderr 与 `result.log`）：  
    - **SizeChange**：`0.xx x (±xx.x%) vs original`（基于输入/输出文件大小）；缺失时输出 `N/A (missing original or output size)`。  
    - **Quality**：有 MS-SSIM 时 `xx.x% (MS-SSIM=0.xxxx)`，仅 SSIM 时 `xx.x% (SSIM=0.xxxx, approx.)`，验证失败时 `N/A (quality check failed)`。  
    - **QualityCheck**：`PASSED` / `FAILED` / `WAIVED` 及简要原因。  
  - 便于在终端与相邻目录日志中看到每个文件的最完整处理透明度。

### 4.2 CLI 默认行为与可关闭项（vid-hevc）

- **子命令**：由 `auto` 改为 `run`，即 `vid-hevc run <path>`。
- **默认行为**：不传任何 flag 时等价于：  
  **推荐组合**（不可关）：`explore` + `match_quality` + `compress`；  
  **可选功能**（默认 on，可关）：`apple_compat` + `recursive` + `allow_size_tolerance`。  
  说明：「只认推荐组合」仅指上述三个（explore/match_quality/compress）必须同时 on，与苹果兼容、递归、容差无关；后三者默认开启，可单独或组合关闭。
- **可关闭项**（可组合使用，例如同时 `--no-apple-compat --no-allow-size-tolerance`）：  
  - `--no-apple-compat`：关闭苹果兼容。  
  - `--no-allow-size-tolerance`：关闭 1% 体积容差。  
- **递归**：强制开启，无 `--no-recursive`；工具与脚本均不提供关闭递归的选项（若需关闭须通过环境或后续扩展）。  
- **vid_hevc/src/main.rs**：上述项默认 true，并增加 `no_apple_compat` / `no_recursive` / `no_allow_size_tolerance` 解析与覆盖逻辑。

### 4.3 拖拽脚本与子命令统一

- **子命令统一为 `run`**：  
  - **vid-hevc**、**vid-av1**、**img-hevc**、**img-av1** 的「自动/推荐」子命令均由 `auto` 改为 `run`（即 `xxx run <path>`），与简化 flag 逻辑一致。  
- **scripts/drag_and_drop_processor.sh**：  
  - **图片**：`process_images` 仅传 `run` 与路径/输出（不再显式传 `--explore --match-quality ...`），与视频一致。  
  - **视频**：`process_videos` 仅传 `run` 与路径/输出；递归强制开启，脚本不再处理 `NO_RECURSIVE` / `--no-recursive`。  
  - 关闭项可组合：环境变量 `NO_APPLE_COMPAT=1`、`NO_ALLOW_SIZE_TOLERANCE=1` 时脚本追加对应 `--no-*`。

---

## 五、技术债务清理：过时 flag 逻辑全面简化

- **shared_utils/src/flag_validator.rs**  
  - 仅接受**推荐组合**：`explore && match_quality && compress`（三者均为 true）；可选 `--ultimate`。  
  - 其余**这三者的**组合一律返回 `Invalid`，不再兼容「单独 --compress / 单独 --explore / 单独 --match-quality」等老旧组合。  
  - 与推荐组合无关的 flag（如 `apple_compat`、`recursive`、`allow_size_tolerance`）由各工具 CLI 处理：默认 on，可通过 `--no-*` 单独或组合关闭。  
  - `FlagMode` 枚举仅保留 `PreciseQualityWithCompress`、`UltimateExplore`；删除 `Default`、`CompressOnly`、`ExploreOnly`、`QualityOnly`、`CompressWithQuality`、`PreciseQuality`。  
  - 单元测试与 `print_flag_help` 已同步简化。

- **vid_hevc、vid_av1、img_hevc、img_av1**  
  - CLI 中 `explore`、`match_quality`、`compress` 默认改为 `true`（推荐组合即默认）。  
  - conversion_api / lossless_converter 中按 `flag_mode` 的分支仅保留「推荐组合」与「ultimate」两条路径，删除对已移除 `FlagMode` 变体的分支。

- **shared_utils/src/conversion.rs**  
  - `explore_mode()` 仅映射为 `PreciseQualityMatchWithCompression`，与简化后的 `FlagMode` 一致。

- **注释与文档**  
  - `video_explorer.rs` 模块头、`vid_hevc/conversion_api.rs` 顶部注释已更新为「仅推荐组合有效」的说明。
