# 图像质量判断与转换逻辑 — 总计改动审计

## 审计范围

- 图像质量判断（无损/有损）可靠性改进
- 转换入口与文件收集逻辑（JXL 排除、MOV/MP4 不按扩展排除）
- 依赖与测试

---

## 一、依赖

| 文件 | 改动 |
|------|------|
| `Cargo.toml` | 新增 `flate2 = "1.1"`（PNG zTXt 解压等） |
| `shared_utils/Cargo.toml` | 新增 `flate2 = { workspace = true }` |

---

## 二、图像质量判断（shared_utils/image_detection.rs）

### 2.1 格式识别扩展

- **DetectedFormat 枚举**：新增 QOI, JP2, ICO, TGA, EXR, FLIF, PSD, PNM, DDS；魔术字识别与 `as_str`/`is_modern_format` 同步。
- **AVIF vs HEIC**：通过 ftyp `compatible_brands` 区分（avif/avis/MA1B/MA1A → AVIF；heic/heix/hevc/hev1 → HEIC）。

### 2.2 容器/码流解析

- **find_box_data_recursive**：递归查找 ISO BMFF box 并返回 payload，供 AVIF/HEIC/JXL 使用（av1C/colr/pixi/hvcC/jxlc/jxlp/jbrd）。
- **AVIF**：av1C 色度 4:2:0/4:2:2 → 有损；4:4:4 + colr(MC=0)/pixi(≥12bit)/高比特深度 → 无损；仅无 av1C 或 4:4:4 无明确指标时 Err。
- **HEIC**：hvcC profile Main/Main10/MSP → 有损；chromaFormatIdc 4:2:0/4:2:2 → 有损；RExt/SCC + 4:4:4 → 无损；无 hvcC 或 RExt 无 4:4:4 → Err。
- **JXL**：容器 jbrd → 无损；裸码流解析 xyb_encoded → 有损/无损；无 jbrd 且头解析失败 → Err。

### 2.3 PNG 量化检测

- 多因素裁判：结构（索引+alpha、大调色板）、元数据（工具签名，含 zTXt 解压）、统计（抖动、颜色分布、渐变条纹）、启发式（压缩效率、**熵**）。
- **熵因子**：索引色 PNG 计算熵与 palette 最大熵比；低熵+大 palette+大图 → 提高有损得分。
- 灰区 [0.40, 0.58] 无工具签名 → 判无损；阈值 0.58 以上判有损；16-bit/真彩无工具签名 → 直接无损。

### 2.4 TIFF / WebP / 其他格式

- **TIFF**：解析所有 IFD 的 tag 259；支持 BigTIFF；6/7 → 有损，其余 → 无损；无 tag → 无损。
- **WebP**：VP8L/VP8；动图遍历 ANMF 帧，任一层 VP8 → 有损。
- **EXR**：解析 compression 属性（NONE/RLE/ZIPS/ZIP/PIZ → 无损；PXR24/B44/… → 有损）。
- **JP2**：COD 标记小波（9/7 不可逆 → 有损，5/3 可逆 → 无损）；无 COD → 默认有损。
- **ICO**：解析目录；内嵌 PNG 做量化检测（tRNS+索引、工具签名）；BMP/DIB 条目 → 无损。
- **QOI/FLIF/PNM**：视为无损；**TGA/PSD/DDS**：视为无损；**JPEG**：恒有损；**GIF**：视为无损。

### 2.5 detect_compression 分发

- 所有已识别格式均有显式分支；新增格式（QOI/JP2/ICO/TGA/EXR/FLIF/PSD/PNM/DDS）按上述规则返回 Lossless/Lossy。
- 不确定时仅在不含关键 box/头时返回 `Err`，不静默判有损。

---

## 三、image_analyzer 与调用链

| 文件 | 改动 |
|------|------|
| `shared_utils/src/image_analyzer.rs` | `analyze_image` 文档（格式检测顺序、质量来自 detect_compression）；`analyze_heic_image` 使用 `detect_compression(HEIC)` 得到 `is_lossless`（不再用 heic_analysis.is_lossless）；`analyze_jxl_image` 使用 `detect_compression(JXL)` 得到 `is_lossless`；`detect_lossless` 统一通过 `detect_compression`（PNG/TIFF/WebP/AVIF）；移除 `check_avif_lossless`（原固定 false）。 |

---

## 四、转换入口与文件收集（img_hevc）

| 文件 | 改动 |
|------|------|
| `img_hevc/src/main.rs` | 在 `auto_convert_single_file` 中若 `analysis.format == "JXL"` 直接跳过并返回“Already JXL”；目录转换使用 `IMAGE_EXTENSIONS_FOR_CONVERT`（不含 jxl）收集待转换图片；注释说明分发顺序（先格式过滤 HEIC/JXL，再按 format/lossless/animated 分发）。 |

---

## 五、file_copier 与 lib

| 文件 | 改动 |
|------|------|
| `shared_utils/src/file_copier.rs` | 新增 `IMAGE_EXTENSIONS_FOR_CONVERT`（与支持列表一致但**排除 jxl**）；`SUPPORTED_VIDEO_EXTENSIONS` 文档：不按扩展排除 mov/mp4，由 codec 检测决定是否转换；保留 `IMAGE_EXTENSIONS_ANALYZE`。 |
| `shared_utils/src/lib.rs` | 导出 `IMAGE_EXTENSIONS_FOR_CONVERT`；`#[cfg(test)] mod image_detection_tests`。 |

---

## 六、测试

| 文件 | 改动 |
|------|------|
| `shared_utils/src/image_detection_tests.rs` | 新增/调整：AVIF 4:2:0 确定性有损、HEIC Main 确定性有损、JXL 无 jbrd 时 Ok(Lossy/Lossless) 或 Err；PNG/TIFF/WebP/JXL container jbrd/HEIC RExt 等用例；所有格式在 detect_compression 中有分支的编译期检查。 |

---

## 七、文档

- `shared_utils/src/image_detection.rs` 模块头：可靠性说明与「Quality judgment reliability audit」表已更新（含 EXR/JP2/ICO/TIFF BigTIFF/WebP 动图/AVIF 4:4:4 ambiguous→Err 等）。
- `docs/quality_detection_improvements.md`：既有改进说明保留。

---

## 八、可靠度结论

- **AVIF/HEIC/JXL**：基于容器/码流多维判断，可靠度高；仅关键信息缺失时 `Err`。
- **PNG**：启发式+灰区+熵因子，中–高；不检测「从有损源导出的 PNG」。
- **TIFF/WebP/EXR/JP2/ICO**：按规范或约定解析，有明确规则。
- **其余格式**：按约定视为无损或有损（JP2 默认有损），标注为假定。

---

*审计日期：基于当前工作区与 git diff 汇总。*
