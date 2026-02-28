# 未使用代码与“仅测试出现”功能审计

本文档说明：为何存在仅在测试中出现的功能、这些代码的实际用途、以及更多未使用/死代码的彻查结果。

**说明**：以下列出的「未使用」或「仅测试」符号**保留不删**，不再以“死函数”为由移除；仅作审计与后续可选优化参考。

---

## 一、为何会有“仅在测试出现”的功能

常见原因：

1. **设计了两套方案，只接了一套**  
   早期或并行设计了「基于像素的质量检测」与「基于容器/元数据的检测」，最终主流程只接了后者，前者保留为模块并写了大量单元测试，但从未在 img_hevc/img_av1 主路径中调用。

2. **预留 API / 未完成的集成**  
   如 `analyze_quality`、`check_avif_lossless` 等声明为 pub、文档写好了，但实现是 `NotImplemented` 或恒返回 false，也无人调用，属于未完成或预留接口。

3. **旧实现被新实现替代，旧 API 未删**  
   如 `generate_recommendation(format, is_lossless, is_animated, path)` 与后来的 `image_recommender::get_recommendation(ImageAnalysis)` 功能重叠，主流程只用后者，前者仅测试保留。

4. **统一抽象与各端分支重复实现**  
   如 `should_skip_image_format(format, is_lossless)` 与 main 里手写 `(format, is_lossless, is_animated)` 分支逻辑重复，主流程从未调用该函数，只在本模块测试里用。

（原「已废弃但未删除的入口」如 `full_explore`、`quick_explore` 已**彻底删除**：从 video_explorer 与 lib 导出中移除，无生产调用。）

---

## 二、未使用代码逐项说明（功能与现状）

### 2.1 图像：image_quality_detector（质量维度与路由已接入）

| 符号 | 功能简述 | 现状 |
|------|----------|------|
| **analyze_image_quality** | 输入 (宽高 + RGBA 像素 + 文件大小 + 格式 + 帧数)，输出边缘密度、色彩多样性、纹理方差、内容类型、compression_potential 等 | 由 **analyze_image_quality_from_path** 在需要时加载像素后调用；路由仍不依赖本函数（主流程用 image_analyzer + image_recommender）。 |
| **analyze_image_quality_from_path** | 按路径加载图像为 RGBA，调用 analyze_image_quality，返回质量维度 | **已用**：img_hevc / img_av1 在 run 且配置了 log 文件、静态图时调用，用于质量判断输出。 |
| **log_media_info_for_image_quality** | 将 ImageQualityAnalysis 格式化为多行，**仅写入日志文件**（不输出到终端） | **已用**：同上，与视频的 log_media_info_for_quality 一致。 |
| **ImageQualityAnalysis** | 分析结果（complexity, edge_density, content_type, compression_potential 等） | 随 from_path 与 log 在生产中用于质量维度输出。 |
| **RoutingDecision** | primary_format, alternatives, use_lossless, should_skip 等 | 已参与路由：主路径用其 **should_skip**（是否再跳过）与 **use_lossless**（Legacy Lossy→JXL 用 0.0 或 0.1）；仍标 `#[deprecated]` 表“不用于替代 format 级路由”。 |
| **ImageContentType** | Photo/Artwork/Screenshot/Icon/Animation/Graphic/Unknown 等 | 在日志中输出，供质量判断与调参参考。 |

**结论**：像素级质量维度与 **RoutingDecision** 已接入：静态图会做像素分析、写 log，并用 `routing_decision.should_skip` 与 `routing_decision.use_lossless` 参与「是否跳过」和「Legacy Lossy→JXL 无损/有损」两条路由；其余格式级路由仍以 image_analyzer + should_skip_image_format 为主。

**已做**：新增 `analyze_image_quality_from_path`、`log_media_info_for_image_quality`；在 auto_convert_single_file 中在转换前调用，并依 pixel_analysis 做跳过与 JXL distance 选择（见下「像素级参与路由」）。

**接入前后行为对比（与 README 路由的关系）**  
- **接入前**：图像转换**已按 README 表格执行**——用 `image_analyzer`（格式/无损）+ `should_skip_image_format` 做「检测格式 → 选择目标（JXL/AVIF/HEIC 或跳过）→ 转换（无损或质量匹配）」。缺少的是：① 没有基于像素的**质量维度输出**（无 content_type、complexity、compression_potential 等）；② 没有用像素级结果参与**路由**（是否跳过、无损 vs 有损）。  
- **接入后**：在保持上述格式级路由不变的前提下，增加：① 对每个待转换的静态图做像素级分析，将质量维度写入 run log（`[Image quality]`）；② 用 `routing_decision.should_skip` 再跳过（与格式级互补）；③ 在「Legacy Lossy→JXL」分支用 `routing_decision.use_lossless` 决定 JXL distance 0.0 或 0.1。即：**接入前已按 README 做格式级路由，接入后补上了质量输出与像素级路由参与**。

**「像素级参与路由」具体指什么？**  
这里的**路由**指两件事：**① 是否跳过（不转换）**、**② 若转换，用无损还是用有损参数**。

- **① 是否跳过（像素级补充）**  
  先有**格式级**跳过：`should_skip_image_format(analysis.format, analysis.is_lossless)` 已把「现代有损（AVIF/WebP/HEIC 有损）、JXL」直接跳过。只有在格式级**没**被跳过的文件才会继续往下走、做像素级分析。  
  像素级分析里会算出一个 `RoutingDecision`，其中 `should_skip` 由 `make_routing_decision` 决定：若源格式在像素侧被识别为「现代有损」（avif/jxl/heic/heif），会设 `should_skip: true`、`skip_reason: "Source is ... - already optimal"`。这类文件绝大多数已在格式级被跳过，像素级相当于**兜底**；少数边缘情况（例如扩展名/容器与像素侧判断不一致）下，像素级会再拦一次。  
  **代码位置**：img_hevc/img_av1 的 `auto_convert_single_file` 里，在 `analyze_image_quality_from_path` 与 `log_media_info_for_image_quality` 之后，若 `pixel_analysis.routing_decision.should_skip == true`，直接 `return Ok(ConversionOutput { skipped: true, ... })`，不再进入后面的 `match (format, is_lossless, is_animated)` 分支。

- **② 无损 vs 有损（仅影响「Legacy Lossy→JXL」这一条分支）**  
  只有走到 **最后一个 match 臂** `(_, false, false)` 的文件会用到像素级的「无损 vs 有损」决策。该臂对应的是：**静态、且格式级判定为有损**（非现代无损、非 JPEG、非动图），例如某些老格式或误判为有损的静态图，实际常见的是「Legacy Lossy→JXL」这一类。  
  **接入前**：这里固定调用 `convert_to_jxl(input, &options, 0.1)`，即**固定用有损**（JXL distance 0.1，约等于质量 100）。  
  **接入后**：用 `pixel_analysis.routing_decision.use_lossless` 决定：  
  - `use_lossless == true` → `convert_to_jxl(..., 0.0)`（**无损**）  
  - `use_lossless == false` → `convert_to_jxl(..., 0.1)`（**有损**）  
  `use_lossless` 在 `make_routing_decision` 中的逻辑是：  
  - `compression_potential < 0.2`（像素算出的压缩潜力很低，适合无损）→ true；或  
  - 源为 PNG 且带透明且 `content_type == Icon` → true；  
  - 否则 false。  
  即：**像素级的 compression_potential、content_type、has_alpha** 共同决定这条分支是「无损 JXL」还是「质量 100 的有损 JXL」。其他分支（如「Modern Lossless→JXL」「JPEG→JXL」「Legacy Lossless→JXL」、动图等）**不受**像素级影响，仍按格式与 is_lossless 走固定逻辑。

**像素级在判定「无损 vs 有损」时是否可靠、精确？**  
**结论：是启发式、非精确。** 适合当作「倾向无损/倾向有损」的参考，不能当作与编解码器或率失真严格一致的判定。

- **判定链**：像素 → 边缘密度/色彩多样性/纹理方差/噪声等 → **complexity**（加权和并 clamp）→ **content_type**（规则：尺寸+透明+复杂度/边缘/色彩 → Icon/Screenshot/Graphic/Photo/Artwork，否则 **Unknown**）→ **compression_potential**（`1.0 - complexity` 再按 content_type、has_alpha、is_animated 加减）→ **use_lossless** = `compression_potential < 0.2` 或「PNG + 透明 + Icon」。
- **为何不算精确**：① **complexity** 是单一标量，由多类统计量线性组合，不包含实际码率或编码器行为；② **content_type** 依赖阈值和规则，易落为 **Unknown**，且与「是否适合无损」无直接理论对应；③ **0.2** 为经验阈值，非率失真或码流分析得出；④ 不区分**源是否本就有损**（如已是 JPEG 再转），仅看当前解码后的像素统计。
- **为何仍有一定参考价值**：低复杂度、图标/截图类往往无损压缩率不错；高复杂度、照片类用有损更省空间。因此作为「倾向用无损还是用有损」的**启发式**是合理的，只是不应期待与真实编码结果或主观质量严格一致。若需更稳妥，可依赖格式级（如源已是无损则走无损分支），或由用户通过选项固定无损/有损。

---

### 2.2 图像：quality_matcher::should_skip_image_format → 已统一

| 对比项 | main 手写分支 | should_skip_image_format |
|--------|----------------|---------------------------|
| **输入** | (format, is_lossless, is_animated) 三元组 | (format_str, is_lossless) 二元；不区分动图 |
| **格式识别** | 精确字符串 "WebP","AVIF","HEIC","HEIF","JXL" | parse_source_codec：大小写不敏感，"jxl"/"jpeg xl","avif","heic"/"heif","webp" → 统一 codec |
| **跳过条件** | JXL 单独提前 return；静态现代有损用 (WebP\|AVIF\|HEIC\|HEIF, false, false)；末臂再判 format 是否现代有损 | is_modern_lossy = !is_lossless && (WebpStatic\|Avif\|Heic\|JpegXl)；is_jxl = (codec==JpegXl)；should_skip = is_modern_lossy \|\| is_jxl |
| **动图** | 单独分支：时长、apple_compat、HEIC 原生、GIF/HEVC 等 | 不处理；仅静态「现代有损 / JXL」 |
| **结论** | 静态跳过与函数等价；动图需 main 保留（apple_compat/时长/HEIC 原生等）。**最佳**：静态跳过以 should_skip_image_format 为单源真相，动图逻辑保留在 main。 |

**已做**：main 在 `!analysis.is_animated` 时调用 `should_skip_image_format(analysis.format.as_str(), analysis.is_lossless)`，若 `should_skip` 则直接 return；移除重复的 JXL 提前判断与静态现代有损的 match 臂，实现统一。

---

### 2.3 图像：image_quality_core → **已删除**

该模块（`shared_utils/src/image_quality_core.rs`）整文件未使用、属早期废弃设计，**已彻底删除**：删除 `image_quality_core.rs`，移除 shared_utils 的 `pub mod image_quality_core`，删除 img_hevc/img_av1 的 `quality_core.rs` 及对 `ConversionRecommendation`、`QualityAnalysis`、`QualityParams` 的 re-export。质量分析与推荐逻辑由 **image_analyzer**、**image_quality_detector**、**image_recommender** 承担。

---

### 2.4 现代格式动图的格式级有无损判断（已实现）

项目在 **格式级**（容器/码流元数据）已实现「有无损」判定，用于路由与推荐，**不依赖像素解码**。各格式入口与逻辑如下。

#### 格式级判定是否可靠？

- **可靠场景**：当容器/码流信息完整且无歧义时，格式级判定是可靠的——直接读盒子/子块（如 av1C/hvcC 的 chroma、profile、colr/pixi 等），无歧义则结论明确（Lossy/Lossless）。
- **不可靠或 Err 场景**：① 缺关键盒子（如 AVIF 缺 av1C、HEIC 缺 hvcC）→ 格式级返回 **Err**；② 4:4:4 等歧义配置且无 Identity/高比特深度等佐证时，部分格式会返回 **Err**。
- **Fallback**：上述 Err 或 analyzer 中 `detect_lossless` 失败时，已接入 **像素级 fallback**：`pixel_fallback_lossless(path)` 解码图像并调用 `image_quality_detector::analyze_image_quality_from_path`，用其 `routing_decision.use_lossless`（基于 complexity / content_type / compression_potential 的启发式）作为 `is_lossless`；解码失败（如 HEIC/AVIF/JXL 无进程内解码器）时 fallback 返回 `false`。

#### AVIF 格式级判定是否可靠？

- **可靠**：当 **av1C** 盒子存在且结论明确时，AVIF 格式级判定可靠。具体为：① 4:2:0 / 4:2:2 → 直接判 **Lossy**；② 4:4:4 + colr Identity (MC=0)、或 4:4:4 + high_bitdepth/twelve_bit、或 Profile 0 + 4:4:4、或 pixi 深度≥12、或单色 4:4:4 → 判 **Lossless**。上述情况均基于 AV1/AVIF 规范，无歧义。
- **返回 Err**：仅当 av1C 缺失，或 4:4:4 且无上述任一明确指标（存在 4:4:4 有损编码）时返回 **Err**，此时 analyzer 使用 **pixel_fallback_lossless**，不丢单。

#### 像素级 fallback 接入清单（已全部接入）

| 位置 | 触发条件 | 代码 |
|------|----------|------|
| **analyze_avif_image** | `detect_compression(AVIF, path)` 返回 Err | `Err(_) => pixel_fallback_lossless(path)` |
| **analyze_heic_image** | `detect_compression(HEIC, path)` 返回 Err | `.unwrap_or_else(\|_\| pixel_fallback_lossless(path))` |
| **analyze_jxl_image** | `detect_compression(JXL, path)` 返回 Err | `.unwrap_or_else(\|_\| pixel_fallback_lossless(path))` |
| **analyze_image 通用路径** | `detect_lossless(&format, path)` 返回 Err（PNG/TIFF/WebP/AVIF 等） | `.unwrap_or_else(\|_\| pixel_fallback_lossless(path))` |

上述 4 处均在 `shared_utils/src/image_analyzer.rs` 中实现，img_hevc / img_av1 通过 `analyze_image` 共用。

#### 入口与数据流

- **image_analyzer::analyze_image(path)** 按检测顺序：HEIC → JXL → AVIF → image crate（PNG/JPEG/WebP/GIF/TIFF）。
- 返回的 **ImageAnalysis.is_lossless** 来自各格式专用分析或通用 **detect_lossless** / **image_detection::detect_compression**。
- 动图（如 WebP 动图、未来 AVIF/HEIC 动图）在「格式级」与静态图共用同一套有无损判定逻辑；动图是否跳过/转码由 main 中时长、Apple 兼容等分支单独处理。

#### 各格式的格式级有无损逻辑（image_detection）

| 格式 | 判定方式 | 说明 |
|------|----------|------|
| **WebP（含动图）** | **detect_compression(WebP)** | 读文件后：若 **is_animated_from_bytes** 为真，则 **detect_webp_animation_compression(data)**：遍历顶层 RIFF 块，找 **ANMF**，每个 ANMF 内帧数据子块看前 4 字节；**VP8**（有空格）→ 整文件判为 **Lossy** 并立即返回；全部为 **VP8L** 或未发现 VP8 → **Lossless**。若未检测为动图，则 **is_lossless_from_bytes**：文件中是否存在 **VP8L** 四字节标识 → 有则 Lossless，否则 Lossy。 |
| **HEIC** | **detect_heic_compression(path)** | 解析 **hvcC** 等盒子：**chromaFormatIdc** 为 4:2:0(1)/4:2:2(2) → 直接 **Lossy**；**profile_idc** 为 Main/Main10/MainStillPicture(1/2/3) → **Lossy**；RExt(4)/SCC(9) + **colr** 中 **matrix_coefficients==0**（Identity）或 **pixi** 高比特深度(≥12) 或 hvcC 内 4:4:4 且 luma/chroma 深度≥12 → **Lossless**；RExt/SCC + 4:4:4 无其他指标 → 倾向 **Lossless**。缺 hvcC → **Err**。 |
| **AVIF** | **detect_avif_compression(path)** | 解析 **av1C**：**chroma_subsampling** 为 4:2:0 或 4:2:2 → **Lossy**；4:4:4 时再查 **colr**（nclx）**matrix_coefficients==0** → **Lossless**；或 4:4:4 + high_bitdepth/twelve_bit / seq_profile≥1 → **Lossless**；或 Profile 0 + 4:4:4（无效有损组合）→ **Lossless**；或 **pixi** 最大深度≥12 → **Lossless**；单色 4:4:4 等也按文档处理。缺 av1C 或 4:4:4 且无明确指标 → **Err**。 |
| **JXL** | **detect_jxl_compression(path)** | 容器 **jbrd** 盒子存在 → **Lossless**；否则看码流 **xyb_encoded** 等 → Lossy/Modular；无法解析 → **Err**。 |
| **PNG** | **detect_png_compression** | **analyze_png_quantization**：色型、调色板、透明度等启发式 → 量化/有损 vs 真无损。 |
| **TIFF** | **detect_tiff_compression** | 遍历所有 IFD，压缩标签(259)：6/7(JPEG) → Lossy，其余/无标签 → Lossless；支持 BigTIFF。 |
| **GIF** | 固定 | 始终 **Lossless**（格式本身无损）。 |
| **JPEG** | 固定 | 始终 **Lossy**。 |

#### 动图特例：WebP 动画

- **detect_webp_animation_compression**：按 RIFF 结构走 **ANMF**，每帧子块 **VP8** / **VP8L** 区分；**任一帧为 VP8 → 整文件 Lossy**，全部 VP8L（或未找到 VP8）→ **Lossless**。与静态 WebP 的「单 VP8/VP8L 块」判定一致，只是改为按帧遍历。
- 在 **image_analyzer** 的通用路径（image crate 解码得到 WebP）中，**is_lossless** 来自 **detect_lossless(WebP) → check_webp_lossless(path) → is_lossless_from_bytes**，即仅用「文件中是否出现 VP8L」；**未**在此路径对动图单独调用 **detect_compression(WebP)**（即未走 **detect_webp_animation_compression**）。因此若需与 **detect_compression** 完全一致（尤其动图含混合 VP8/VP8L 时），可在 analyzer 的 WebP 分支改为调用 **detect_compression(DetectedFormat::WebP, path)** 取 **is_lossless**。

#### 与 analyzer 的对接情况（含像素级 fallback）

- **HEIC**：**analyze_heic_image** 中 **is_lossless = detect_compression(HEIC, path)**；若返回 **Err** 则用 **pixel_fallback_lossless(path)**。已对接，img_hevc / img_av1 共用。
- **JXL**：**analyze_jxl_image** 中 **is_lossless = detect_compression(JXL, path)**；若 **Err** 则 **pixel_fallback_lossless(path)**。已对接。
- **AVIF**：**analyze_avif_image** 已接入 **detect_compression(AVIF, path)**，格式级成功则用其结果，**Err** 时用 **pixel_fallback_lossless(path)**。img_hevc / img_av1 两工具均通过 shared_utils 使用该逻辑。
- **WebP（含动图）**：通用路径用 **detect_lossless(WebP) → check_webp_lossless**；若 **detect_lossless** 失败（或其它格式在通用路径失败），则 **unwrap_or_else** 使用 **pixel_fallback_lossless(path)**。
- **通用路径**（PNG/TIFF/JPEG/WebP 等经 image crate 解码）：**is_lossless = detect_lossless(&format, path).unwrap_or_else(|_| pixel_fallback_lossless(path))**，格式级失败即回退到像素级。

综上：格式级有无损在 **image_detection::detect_compression** 中已实现；analyzer 侧 **HEIC / JXL / AVIF** 均已接格式级 + 像素级 fallback，通用路径同样在失败时走像素级 fallback。

#### 静态图 vs 动图：格式级有无损都适用吗？

**适用。** 现代格式的格式级有无损判断**不区分静态/动图**，同一套逻辑对静态和动图都生效：

- **WebP**：动图时显式走 **detect_webp_animation_compression**，按 ANMF 逐帧看 VP8/VP8L（任一帧 VP8 → 整文件 Lossy）；静态时看单 VP8/VP8L 块。同属「格式级」，都适用于动图与静态图。
- **AVIF**：av1C/colr/pixi 等盒子描述整文件的编码配置，动图 AVIF 通常全片共用同一配置，故 **detect_avif_compression** 对静态/动图 AVIF 均适用。
- **HEIC**：hvcC 等描述主图（或序列）的编码，多图/连拍 HEIC 仍用同一套格式级判定。
- **JXL**：jbrd/码流描述整文件，动图 JXL 同样适用 **detect_jxl_compression**。

**is_animated** 只影响主流程中的**路由**（是否按“动图”做转码/跳过等），不影响「有无损」判定本身；有无损判定对静态与动图使用同一套格式级（+ 失败时像素级 fallback）逻辑。

---

### 2.5 视频：video_quality_detector（部分已接入）

| 符号 | 功能简述 | 现状 |
|------|----------|------|
| **analyze_video_quality_from_detection** | 从 VideoDetectionResult 构建 VideoQualityAnalysis | **已用**：vid_hevc/vid_av1 在 SSIM 探索前调用，用于搭配 SSIM 的媒体信息展示。 |
| **log_media_info_for_quality** | 将 VideoQualityAnalysis 格式化为多行，**仅写入日志文件**（不输出到终端） | **已用**：同上，在配置了 log file 时写入 codec/分辨率/码率/bpp/content_type 等。 |
| **analyze_video_quality(...)** | 底层多参数分析（被 from_detection 调用） | 通过 from_detection 间接使用。 |
| ~~**VideoRoutingDecision**~~ | ~~路由结论（primary_format/encoder/recommended_crf 等）~~ | **已删除**：结构体与 make_video_routing_decision 已移除；主流程始终用 video_detection + quality_matcher 做路由。 |
| **to_quality_analysis** | 将 VideoQualityAnalysis 转为 quality_matcher::QualityAnalysis | 仍仅本模块及测试；主流程用 from_video_detection 构建。 |
| **ChromaSubsampling, VideoCodecType, VideoContentType, CompressionLevel** | 视频分析用枚举/类型 | 作为 VideoQualityAnalysis 字段在日志中展示。 |

**结论**：主流程路由仍用 video_detection + quality_matcher；**媒体信息**在 SSIM/质量探索时通过 analyze_video_quality_from_detection + log_media_info_for_quality 写入日志文件，终端不显示。

---

### 2.6 近期修复说明（动图验证 + XMP fallback + 日志）

#### 第二项修复：动图→视频「Enhanced verification failed: Duration mismatch or output probe failed」

- **现象**：GIF/动图转 HEVC 后，增强校验报错「Duration mismatch or output probe failed」，日志里大量出现，但转换其实已成功、输出文件存在且可播。
- **原因**：  
  1. **output_probe 失败**：对输出 MP4 调 ffprobe 失败（例如格式/编码导致 ffprobe 报错）时，原逻辑把 `duration_match` 和 `has_video_stream` 设为 `Some(false)`，导致 `passed()` 为 false，报「Duration mismatch or output probe failed」（语义上其实是 probe 失败，不是真正的 duration 不一致）。  
  2. **input_probe 失败**：对源 GIF 等 ffprobe 拿不到 duration 时，原逻辑把 `duration_match = Some(false)`，无法做时长对比也直接判失败。  
  3. **时长容差过严**：`strict_video()` 的 `duration_tolerance_secs` 为 0.5s，GIF 帧延迟累加与 MP4 duration 存在舍入差时易被判为 mismatch。
- **修改**（`shared_utils/src/quality_verifier_enhanced.rs`）：  
  1. **probe 失败不伪造成功**：input_probe 或 output_probe 失败时，仍将 `duration_match` / `has_video_stream` 设为 `Some(false)`（若选项要求检查），使 `passed()` 为 false；message 使用「Probe failed; duration/stream not verified」，避免在未做 duration/stream 检查时仍报「验证通过」。  
  2. **时长容差**：`strict_video()` 的 `duration_tolerance_secs` 从 0.5 改为 1.0 秒，减少 GIF→视频舍入导致的误判。  
  3. **文案**：仅在真正「两端 probe 都成功且时长差超容差」时使用 "Duration mismatch (input vs output beyond tolerance)"；probe 失败时用 "Probe failed; duration/stream not verified"。
- **结果**：probe 不可用时验证判为失败（不把「未验证」当作通过）；只有实际完成 duration/stream 检查且通过时才 passed。

#### 第三项修复：XMP merge 失败时的 fallback 策略（禁止伪造成功、明确工具 fallback）

- **策略**：ExifTool 合并 XMP 失败（如 GIF 等格式报 "Format error"）时，**不**视为成功；用 **exiv2** 作为补救工具再尝试一次真正写入目标文件，失败则仅记录、不伪造成功。
- **实现**（`shared_utils/src/metadata/mod.rs`）：  
  1. `merge_xmp_sidecar` 中，当 `merger.merge_xmp(&xmp, dst)` 返回 `Err` 时，先调用 **xmp_merge_failure** 记录 ExifTool 失败。  
  2. **try_merge_xmp_exiv2(xmp_path, dst)**：将 XMP 临时复制到目标同目录且同 stem 的 `<stem>.xmp`（exiv2 `-i` 要求），执行 `exiv2 -i <dst>` 做嵌入；成功则调用 **xmp_merge_success**，并写 log「Fallback: exiv2 merge succeeded (ExifTool had failed).」；执行后删除临时 sidecar。  
  3. 若 exiv2 未安装或合并失败，写 log「Fallback: exiv2 merge failed or exiv2 not available; no fake success.」，不报成功。  
  4. 无「仅复制 sidecar 文件」的 fallback：补救措施仅为「换用 exiv2 再尝试嵌入」。
- **结果**：合并失败时先记失败；用 exiv2 能成功则视为成功并明确记录；否则不伪造成功，日志中 fallback 行为清晰可查。

---

### 2.6.1 伪造结果（fake success）全面审计

以下为「未验证即报成功 / 未达标仍报成功」类问题的排查与结论，避免任何把「未验证」或「失败」当作「通过」的判定。

| 位置 | 风险点 | 结论 / 修复 |
|------|--------|-------------|
| **quality_verifier_enhanced::passed()** | 原先用 `duration_match.unwrap_or(true)`：probe 失败时若误设为 None 会变成「通过」。 | **已修**：probe 失败时必设 `Some(false)`；`passed()` 改为 `duration_match != Some(false) && has_video_stream != Some(false)`，仅「未要求检查」(None) 或「明确通过」(Some(true)) 才通过，绝不把「未验证」当通过。 |
| **verify_after_encode 调用方** | `video_explorer/gpu_coarse_search.rs` 原先仅对结果做 `verbose_eprintln`，未用 `enhanced.passed()` 决定是否通过。 | **已修**：在构建 `ExploreResult` 前执行 `quality_passed = quality_passed && enhanced.passed()`，验证未通过时 `quality_passed` 为 false，conversion_api 会按既有逻辑丢弃输出或走 Apple 回退，不再在验证失败时仍报「质量通过」并 commit。 |
| **ConversionResult::skipped_*** | `skipped_duplicate` / `skipped_exists` 等设 `success: true`。 | **合理**：`is_success()` 为 `success && !skipped`，统计时计为 skip 而非 success，不伪造转换成功。 |
| **vid_hevc/vid_av1 Apple compat fallback** | `!quality_passed` 时若保留 best-effort 输出仍返回 `success: true`。 | **已知设计**：文案明确为「Apple compat fallback… quality/size below target」，属「保留可导入文件但目标未达标」的显式折中；若需严格「未达标即 success=false」可再改。参见 AUDIT_DESIGN_ISSUES P0-1。 |

**passed() 语义（当前实现）**：`file_ok && (duration_match != Some(false)) && (has_video_stream != Some(false))`。即：仅当「未要求该项检查」(None) 或「该项检查通过」(Some(true)) 时该项不拉低结果；一旦为 `Some(false)`（含 probe 失败）即判为未通过，不伪造成功。

#### 日志再检查（排除已修复项）

对 `logs/img_hevc_run.log` 做 ❌/⚠️ 类文案统计与抽样，**未发现新的、需修复的同类问题**：

| 日志中的文案 | 次数 | 类型 | 结论 |
|-------------|------|------|------|
| Enhanced verification failed: Duration mismatch or output probe failed | 27 | 已修复 | 动图→视频验证逻辑已修，新运行会走新文案并影响 quality_passed。 |
| XMP merge failed: ExifTool ... Format error | 1 | 已修复 | 已用 exiv2 fallback + 明确写 log。 |
| Conversion failed | 0 | - | 本段 run 未出现图片转换失败记录（多为动图→HEVC）。 |
| ❌ WALL HIT #N / ❌ SSIM ALL BELOW TARGET | 大量 | 正常 | 探索阶段「体积变大 / 质量未达目标」的预期输出，非缺陷。 |
| ⚠️ GPU SSIM too low! Expanding CPU search… / ⚠️ GPU boundary CRF … (TOO LARGE) / ⚠️ CPU start CRF clamped | 若干 | 正常 | 探索策略的提示信息，非缺陷。 |
| ❌ (fail #1/3)～(fail #3/3) | 17×3 | 正常 | CRF 探索连续 3 次体积变大时的计数，属预期行为。 |

**结论**：当前日志中与「转换失败 / 验证伪造 / 元数据合并」相关的，仅包含上述已修复项；其余均为探索与质量未达标的正常输出，无需再修。

#### 日志输出到文件的缺陷与修复（drag_drop 信息量少、未记录最完整输出）

- **原因**：  
  1. **drag_drop_*.log**：由脚本 `tee` 得到的是**二进制 stderr**（eprintln! / log_eprintln!）。Rust 里大量内容只走 **write_to_log()**（如 log_conversion_failure、XMP 失败、verbose_eprintln! 在非 verbose 时），写入的是 `--log-file` 指定的 **verbose_XXX.log**，不会出现在 session log 里。  
  2. **img_hevc_run.log**：直接运行 img_hevc 时由 set_default_run_log_file 写入，内容完整；但通过脚本跑时若传了 `--log-file`，则写入 verbose_XXX.log，不写 img_hevc_run.log。  
  3. **关键写入未 flush**：BufWriter 缓冲导致进程异常退出时，最后几条 write_to_log 可能未落盘。
- **修改**：  
  1. **脚本**（`scripts/drag_and_drop_processor.sh`）：在 `save_log()` 里，在统计 footer 之前，将 **VERBOSE_LOG_FILE**（img_hevc + vid_hevc 的 run log）整段追加到 **LOG_FILE**（drag_drop_*.log），并加「📋 Full run log」分隔，使**单次 session log 即包含完整 run log**。  
  2. **Rust**（`shared_utils/src/progress_mode.rs`）：`log_conversion_failure`、`xmp_merge_failure` 在 `write_to_log` 后立即 **flush_log_file()**，避免关键失败行因缓冲未写入。
- **结果**：drag_drop_*.log 同时拥有「脚本 + 二进制 stderr」与「Rust 内部完整 run log」；关键失败行会及时落盘。

#### 用户五类问题复查（日志 / 动图验证 / XMP / 进度 / 其他）

| # | 问题 | 状态 | 说明 |
|---|------|------|------|
| 1 | JPEG→JXL bitstream reconstruction：错误未进 run log | **已覆盖** | ① 逻辑：strip tail → 原 cjxl → `--allow_jpeg_reconstruction 0` 已实现；② 失败时返回的 `Err` 含 cjxl 完整 stderr（`lossless_converter` 里 `format!("... {}", stderr)`），`main` 里 `log_conversion_failure(path, &err_str)` 会把整条错误（含 allow_jpeg_reconstruction / bitstream reconstruction / too much tail data 等）写入 run log。 |
| 2 | 动图→视频 Enhanced verification failed（27 次） | **已修** | probe 失败或 duration 不一致时不再伪造通过；`quality_passed = quality_passed && enhanced.passed()`，验证未过则走丢弃或 Apple 回退。失败时 summary 为「Probe failed; duration/stream not verified」或「Duration mismatch (input vs output beyond tolerance)」。 |
| 3 | XMP merge 失败（GIF ExifTool Format error） | **已覆盖** | ExifTool 失败后先 `xmp_merge_failure(&err_str)` 写 run log，再尝试 exiv2；exiv2 成功则记成功并写「Fallback: exiv2 merge succeeded」，失败则写「Fallback: exiv2 merge failed or exiv2 not available; no fake success」。不伪造成功。 |
| 4 | 进度「Images: N OK, M failed」无单次失败原因 | **已覆盖** | 单文件转换 `Err(e)` 时调用 `log_conversion_failure(path, &err_str)`，run log 中会有「❌ Conversion failed <path>: <完整 error>」，含 cjxl stderr 等，便于事后筛出同类问题。 |
| 5 | 其他（WALL HIT / CRF +% / SSIM ALL BELOW TARGET） | **非缺陷** | 探索/质量未达目标时的正常输出，非同类转换失败。 |

---

### 2.7 已废弃且无生产调用的导出（含已删除）

| 符号 | 位置 | 说明 |
|------|------|------|
| ~~**full_explore**~~ ~~**quick_explore**~~ | ~~video_explorer~~ | **已删除**：已从 video_explorer 与 lib 导出中移除；原无生产调用，替代为 explore_size_only / explore_precise_quality_match。 |
| **explore_compress_only_gpu** 等一批 `*_gpu` | video_explorer | 部分带 deprecated，实际探索路径走 explore_hevc_with_gpu_coarse 等，不直接调这些。 |
| **SsimCalculationResult / SsimDataSource** | explore_strategy | 类型别名 deprecated，建议用 SsimResult / SsimSource。 |
| **realtime_progress 中某旧类型** | realtime_progress | deprecated，建议用 SimpleIterationProgress。 |
| **estimate_cpu_search_center 的旧版** | gpu_accel | deprecated，用 estimate_cpu_search_center 替代。 |

---

## 三、更多未使用/弱使用项（彻查摘要）

- **image_quality_core::QualityAnalysis/ConversionRecommendation/QualityParams**  
  在 img_hevc 中仅作类型 re-export，无任何业务逻辑使用；实际推荐与质量类型来自 image_analyzer + image_recommender + quality_matcher。

- **video_quality_detector**  
  analyze_video_quality_from_detection、log_media_info_for_quality 已由 vid_hevc/vid_av1 使用。**to_quality_analysis**：仅模块内测试与 lib 导出，主流程用 quality_matcher::from_video_detection，无生产调用。

- **image_quality_detector**  
  已接入：analyze_image_quality_from_path、log_media_info_for_image_quality 及 routing_decision（should_skip/use_lossless）在 img_hevc/img_av1 run 中使用。analyze_image_quality 由 from_path 内部调用；RoutingDecision 仅作结构体字段，主路由仍以 image_analyzer + should_skip_image_format 为主。

- **log_quality_analysis**  
  有使用：vid_hevc/vid_av1 conversion_api、img_hevc/img_av1 lossless_converter。非死代码。

- **from_video_detection**  
  有使用：vid_av1 conversion_api。非死代码。

---

## 四、建议（可选后续动作）

1. **明确“保留但未接入”的模块**  
   若计划日后接入「像素级图像质量」或「video_quality_detector 路由」：在模块顶或 README 注明「当前未接入主流程，仅测试与 API 保留」，避免误以为已在用。

2. **删除或收敛未用 API**  
   - 确定永不接入：可考虑删除或改为 `pub(crate)`，并删掉仅覆盖这些 API 的测试，减少维护成本。  
   - 保留作备用：保留代码但去掉从 lib 的公开 re-export，仅 crate 内可用。

3. **未实现/占位 API**  
   - `analyze_quality`、`check_avif_lossless`：要么实现并接入，要么改为返回 `Option`/明确“未实现”文档并移除 pub，避免被误用。

4. **废弃 API**  
   - ~~`full_explore`、`quick_explore`~~：**已删除**。其余已 deprecated 且无调用方者可视情况在下一大版本移除或改为 `pub(crate)`。

5. **统一图像跳过逻辑**  
   若希望单源真相：可让 main 的分发逻辑改为调用 `should_skip_image_format(analysis.format.as_str(), analysis.is_lossless)`，再根据 SkipDecision 分支，避免与手写分支重复。

---

## 五、汇总表

| 类别 | 模块/符号 | 生产使用 | 仅测试/未实现/废弃 |
|------|-----------|----------|----------------------|
| 图像 | image_quality_detector（from_path / log / routing） | **已用**（img_hevc、img_av1 run） | RoutingDecision 仅作字段保留 |
| 图像 | should_skip_image_format | **已用**（静态跳过单源） | — |
| 图像 | ~~image_quality_core~~ | — | **已删除**（整文件未使用、早期废弃） |
| 视频 | video_quality_detector（from_detection / log） | **已用**（vid_hevc、vid_av1） | to_quality_analysis 仅测试/导出 |
| 视频 | ~~full_explore, quick_explore~~ | — | **已删除** |
| 其它 | 若干 deprecated 类型/函数；load/save/clear_processed_list；print_flag_help；calculate_bpp；部分 explore_*_gpu；count_all_files | 无 | 见第六节 |

上述项均为「从未被主流程使用的代码或从未实现的占位 API」，可按产品与维护策略决定保留、隐藏或删除。

---

## 六、再次彻查：从未被调用的符号（2025 补充）

以下为**从未被生产代码调用**的 API（仅定义/导出/测试内使用）。

### 6.1 图像 / conversion

| 符号 | 位置 | 说明 |
|------|------|------|
| **load_processed_list** | shared_utils/conversion.rs | 仅定义；img_hevc/img_av1 lossless_converter 仅 use，从未调用。 |
| **save_processed_list** | shared_utils/conversion.rs | 同上。 |
| **clear_processed_list** | shared_utils/conversion.rs | 同上。 |

### 6.2 图像 / image_quality_core → **已删除**

整模块已删除，见 2.3。

### 6.3 视频 / video_quality_detector

| 符号 | 说明 |
|------|------|
| **to_quality_analysis(analysis)** | 仅模块内测试与 lib 导出；主流程用 quality_matcher::from_video_detection。 |

### 6.4 视频 / video_explorer（GPU 探索）

| 符号 | 说明 |
|------|------|
| **explore_precise_quality_match_gpu** | 仅导出，无生产调用；vid_av1 实际调用的是 explore_precise_quality_match_**with_compression**_gpu。 |
| **explore_compress_only_gpu** | 仅导出，无生产调用。 |
| **explore_compress_with_quality_gpu** | 仅导出，无生产调用。 |
| **explore_quality_match_gpu** | 仅导出，无生产调用。 |
| **explore_size_only_gpu** | 仅导出，无生产调用。 |

生产路径使用 explore_hevc_with_gpu_coarse* / explore_av1_with_gpu_coarse / explore_precise_quality_match_with_compression_gpu。

### 6.5 其它

| 符号 | 位置 | 说明 |
|------|------|------|
| **print_flag_help()** | flag_validator.rs | 仅导出，四个二进制均未调用。 |
| **calculate_bpp(input)** | video_explorer/precheck.rs | 仅定义，无任何调用方。 |
| **count_all_files** | lib 导出 (file_copier::count_files) | 仅 lib 导出，无二进制或其它 crate 调用。 |
