# 设计问题审计 — 边界、一致性、回退与默认值

本文档审计与「最终产出质量」相关的**设计层面**问题：边界条件、回退逻辑一致性、默认值语义、TOCTOU 与零值防护等。编号 P0 为优先修复，D 为设计/文档类。

---

## P0-1：Apple 回退与视频流大小校验

**现象**  
探索路径下 `!explore_result.quality_passed` 时（含「视频流未压缩」、SSIM 未过等），若开启 Apple 兼容且源为 AV1/VP9/VVC/AV2，会**保留** best-effort HEVC 输出并返回 success。此时**未**再校验「视频流是否真的更小」：即使 `!video_stream_compressed`（输出视频流 ≥ 输入），仍会 commit 并保留该输出。

**对比**  
- **require_compression 路径**（约 643–717 行）：在 `require_compression && !can_compress` 时，同样有 Apple 回退保留输出；此处 `can_compress` 在 `allow_size_tolerance` 下为 `video_compression_ratio < 1.01`，即允许视频流最多约 1% 更大仍算「可接受」。  
- **探索失败路径**（约 326–464 行）：仅根据 `quality_passed` 与 Apple 条件决定保留，**未**使用与 require_compression 一致的「视频流必须更小或 within 1%」条件。

**风险**  
用户可能得到「总文件或视频流比原文件更大」的 HEVC，与「压缩/质量未达标则保护原文件」的直觉不一致。

**建议**  
- 在探索失败分支的 Apple 回退中，增加与 require_compression 一致的条件：仅当「视频流已压缩」或「在 allow_size_tolerance 下 video_compression_ratio < 1.01」时才保留 best-effort 输出；否则丢弃输出、保护原文件。  
- 或明确文档/日志说明：Apple 回退可能保留「体积未缩小」的 HEVC，仅用于获得可导入格式。

**负责文件**  
`vid_hevc/src/conversion_api.rs`（探索失败分支 Apple 回退）；`vid_av1` 若有类似逻辑需一并审查。

---

## P0-2：compress 模式边界条件澄清

**现象**  
- `shared_utils/conversion.rs`：`check_size_tolerance` 中先判 `output_size > max_allowed_size`（拒收），再判 `options.compress && output_size >= input_size`（拒收）。  
- 各 `conversion_api`：`config.compress && out_size >= detection.file_size` 时删输出、保留原文件。  
- 文档已写「compress 时须严格更小，相等即未达标」。

**一致性**  
实现上均为 `>=`，即「相等」一律视为未达标，与「严格更小」一致。

**建议**  
- 在**单一权威位置**（如 `conversion.rs` 的 `check_size_tolerance` 文档或模块头）明确写：  
  「compress 模式：仅当 output_size **<** input_size 接受；output_size **>=** input_size 一律拒绝（含相等）。」  
- 各 conversion_api 中的 compress 分支注释可简短引用该定义，避免后续改为 `>` 导致行为不一致。

**负责文件**  
`shared_utils/src/conversion.rs`（文档）；`img_hevc/src/conversion_api.rs`，`img_av1/src/conversion_api.rs`（注释引用）。

---

## D1：allow_size_tolerance 默认值与视频「可接受」语义

**现象**  
- `ConversionConfig::default()` 中 `allow_size_tolerance: true`。  
- 图像：`max_allowed_size = input * 1.01`，即仅当 output **>** input×1.01 才因「过大」拒收。  
- 视频：`can_compress = video_compression_ratio < 1.01`（当 tolerance 为 true 时），即视频流在 1% 以内更大仍视为「可压缩通过」。

**影响**  
默认策略略宽松：允许输出略大于输入（图像 ≤1%、视频流 <1.01×）仍可能被接受。与「compress 必须更小」叠加时，需区分：  
- 图像：compress 分支单独用 `output_size >= input_size` 拒收，与 tolerance 叠加正确。  
- 视频：require_compression 下「可接受」包含「视频流略大但 <1.01×」，需在文档或日志中写清。

**建议**  
- 在用户可见文档或帮助文本中说明：`allow_size_tolerance` 为 true 时，视频流允许最多约 1% 更大仍可能接受（与图像 1% 容差对齐）。  
- 若产品策略改为「默认不接受任何更大」，再考虑将默认改为 false 或拆分「图像容差 / 视频容差」选项。

**负责文件**  
`shared_utils/src/conversion_types.rs`（默认值）；`vid_hevc/src/conversion_api.rs`（can_compress 逻辑）；文档/CLI 帮助。

---

## D2：safe_delete_original 的 min_output_size 不一致

**现象**  
- 图像：`img_hevc/src/conversion_api.rs`、`img_av1/src/conversion_api.rs` 及 `shared_utils/conversion.rs` 中调用 `safe_delete_original(_, _, 100)`。  
- 视频：`vid_hevc/src/conversion_api.rs`、`vid_av1/src/conversion_api.rs` 中调用 `safe_delete_original(_, _, 1000)`。  
- 动图→视频：`vid_hevc/animated_image.rs` 中为 `100`。

**影响**  
视频要求输出至少 1000 字节才允许删原文件；图像与动图为 100。若输出因编码异常产生极小文件（如几百字节），图像路径可能删原文件而视频路径不删，行为不一致。

**建议**  
- 统一为同一常量（如 1000），或按格式在共享处定义（如 `MIN_OUTPUT_SIZE_IMAGE` / `MIN_OUTPUT_SIZE_VIDEO`），并在 `checkpoint.rs` 文档中说明「低于此大小视为无效输出，不删原文件」。  
- 或明确设计理由（例如视频容器最小合理大小约 1KB），在代码注释中写明。

**负责文件**  
`shared_utils/src/checkpoint.rs`（`verify_output_integrity`、`safe_delete_original` 文档）；各调用点。

---

## D3：两处 Apple 回退逻辑重复与条件差异

**现象**  
- **探索失败**（约 326–464 行）：`!explore_result.quality_passed && (match_quality || explore_smaller)` 时，若 Apple 兼容且源为不兼容编码，保留输出；**不**再检查视频流是否更小。  
- **require_compression 失败**（约 643–717 行）：`require_compression && !can_compress` 时，同样有 Apple 回退；此处 `can_compress` 依赖 `video_compressed` 或 `video_compression_ratio < 1.01`。

**影响**  
两处 fallback 条件与后续行为不完全对称，维护时易漏改其一；且探索路径可能保留「视频流更大」的输出（见 P0-1）。

**建议**  
- 将「是否允许保留 Apple best-effort 输出」的条件收口到单一函数，例如：  
  `should_keep_apple_fallback_output(codec, video_stream_compressed, video_compression_ratio, allow_size_tolerance)`，  
  要求：源为 Apple 不兼容编码，且（视频流已压缩 或 在容差内 ratio < 1.01）。  
- 探索失败与 require_compression 两处均调用该函数，再决定 commit 或丢弃。

**负责文件**  
`vid_hevc/src/conversion_api.rs`（两段 Apple 回退）；可选：`shared_utils/quality_matcher.rs` 或新辅助模块。

---

## D4：input_size / output_size 为零的防护

**现象**  
- `conversion.rs`：`input_size == 0` 时仅用于百分比计算（避免除零），未拒绝转换。  
- `img_hevc/img_av1 conversion_api`：`if detection.file_size == 0` 提前返回或跳过。  
- 部分路径有 `output_size == 0` 检查（如 img_hevc 363、430、518 行；vid_hevc animated_image 等）。

**风险**  
若某路径未检查 output_size 就 commit 或调用 `safe_delete_original`，可能用「空输出」覆盖或删除原文件。当前 `verify_output_integrity(output, min_output_size)` 会拒绝 len < min，能挡住 0，但依赖所有成功路径都经 safe_delete 且 min≥1。

**建议**  
- 在 `commit_temp_to_output` 或最终「接受输出」的单一入口处，增加 `output_size == 0` 的拒绝（若尚未覆盖）。  
- 在审计清单中标注：所有「写 output 并可能删原文件」的路径均经 `check_size_tolerance` 或 `verify_output_integrity`/`safe_delete_original`，且无「先删原文件再写 output」的顺序。

**负责文件**  
`shared_utils/src/conversion.rs`；各 conversion_api 与 animated_image。

---

## D5：TOCTOU 与原子写入

**现象**  
`pre_conversion_check` 已注明：`output.exists()` 仅为建议性检查，调用方必须使用 `temp_path_for_output` + `commit_temp_to_output` 做原子写入。

**建议**  
- 确认所有图片/视频转换路径均为：写 temp → 校验（size/quality）→ `commit_temp_to_output`，无「直接写最终 output 再校验」的路径。  
- 新加转换流程时在清单中勾选「使用 temp + commit」。

**负责文件**  
`shared_utils/src/conversion.rs`（pre_conversion_check、temp_path_for_output、commit_temp_to_output）；各 lossless_converter、conversion_api、animated_image。

---

## D6：探索路径与 require_compression 的触发关系

**现象**  
- 探索路径：`!explore_result.quality_passed && (config.match_quality || config.explore_smaller)` 时进入失败处理（含 Apple 回退）。  
- 后续还有 `require_compression && !can_compress` 的校验（在已 commit 的探索成功路径之后，对同一输出再做一次 verify）。

**影响**  
若探索阶段认为 quality_passed 但实际 output 在 require_compression 下未通过（例如视频流略大），会在后面被拒绝并删输出；逻辑正确，但两阶段条件（quality_passed vs can_compress）的差异需在注释或文档中说明，避免误以为「探索过 = 一定接受」。

**建议**  
在 `vid_hevc/conversion_api.rs` 中探索成功后的 `require_compression` 块前，加简短注释：说明此处为「总文件与视频流大小的二次校验」，与探索阶段的 SSIM/融合分数门限互补。

**负责文件**  
`vid_hevc/src/conversion_api.rs`；`vid_av1` 若有对应逻辑同理。

---

## 汇总表

| 编号 | 类型 | 简述 | 建议 |
|------|------|------|------|
| P0-1 | 逻辑/策略 | Apple 回退在探索失败时可能保留「视频流更大」的输出 | 回退前增加与 require_compression 一致的视频流大小条件 |
| P0-2 | 文档 | compress 边界 `>=` 与「严格更小」需统一写明 | 在 conversion 模块单点文档化并引用 |
| D1 | 默认值/语义 | allow_size_tolerance 默认 true，视频可接受 ratio&lt;1.01 | 文档/帮助中说明默认容差语义 |
| D2 | 一致性 | safe_delete min 图 100、视频 1000 | 统一常量或文档化差异原因 |
| D3 | 重复逻辑 | 两处 Apple 回退条件略不同 | 收口到单一判定函数 |
| D4 | 边界 | input/output size 为 0 的防护分散 | 确认所有成功路径经校验；必要时在 commit 前统一拒 0 |
| D5 | 并发/安全 | TOCTOU、原子写入 | 确认全部使用 temp + commit |
| D6 | 可读性 | 探索与 require_compression 两阶段条件差异 | 注释说明两阶段职责 |

以上为设计问题审计清单，可与 `AUDIT_QUALITY_DECISION_CHAIN.md` 配合做全链路与设计层面审查。
