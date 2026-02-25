# 复制与无遗漏策略 / Copy and Completeness Strategy

**位置**：全部在**程序内**（Rust），无 rsync 或外部脚本。

---

## 1. 流程概览

批量模式（指定 `--output` 目录）时：

1. **逐文件处理**：支持的视频/图片由主程序转换，输出到 `output_dir` 下保持相对路径。
2. **单文件失败/跳过**：对该文件调用 `copy_on_skip_or_fail(source, output_dir, base_dir, …)`，将**原文件**按相同相对路径复制到输出目录。
3. **批量结束后**：调用 `copy_unsupported_files(input_dir, output_dir, recursive)`，把**不支持格式**的文件（如 .txt、.pdf、.psd 等）按目录结构复制到输出目录。
4. **完整性校验**：`verify_output_completeness(input_dir, output_dir, recursive)` 比较输入/输出文件数量（预期 = 输入总数 − XMP 边车数），并打印是否缺失或多余。

---

## 2. 各层逻辑

### 2.1 `copy_on_skip_or_fail`（smart_file_copier.rs）

- **时机**：某个支持格式的文件被跳过或转换失败时（如 Skip、质量不达标、错误）。
- **行为**：`smart_copy_with_structure(source, output_dir, base_dir)` → 按 `base_dir` 下相对路径写入 `output_dir`，保留目录结构；复制后调用 `copy_metadata`。
- **已存在**：若目标路径已存在，**不覆盖**，仅打日志 "Already exists"，再执行 `copy_metadata`。避免误覆盖已有转换结果。

### 2.2 `copy_unsupported_files`（file_copier.rs）

- **时机**：`cli_runner::run_auto_command` 在**所有支持文件处理完后**、且配置了 `config.output` 时执行。
- **无遗漏设计**：遍历输入目录，对 `should_copy_file(path) == true` 的文件复制到输出目录。  
  `should_copy_file` 为 **false** 的：支持的图片/视频扩展名、.xmp、以 `.` 开头的文件；**true** 的：其余（.txt、.pdf、.psd 等）。
- **行为**：`std::fs::copy` 到 `output_dir.join(rel_path)`，会**覆盖**已存在的同路径文件；复制后 `copy_metadata`，并对复制文件尝试 `merge_xmp_for_copied_file`，失败则尝试复制 .xmp 边车。

### 2.3 `verify_output_completeness`（file_copier.rs）

- **预期数量**：`input_stats.expected_output() = total - sidecars`（不要求输出里有 XMP 文件）。
- **实际数量**：`output_stats.total`（输出目录下所有非隐藏文件数）。
- **结果**：`diff = expected - actual`；diff=0 通过，diff>0 报缺失，diff<0 报“输出多出若干文件”但仍视为通过。

---

## 3. 是否会出现冲突

| 场景 | 是否冲突 | 说明 |
|------|----------|------|
| 同一路径被「转换输出」与「copy_unsupported」同时写 | 否 | 支持格式只做转换不进入 copy_unsupported；不支持格式只被复制，路径与转换输出扩展名不同。 |
| 失败/跳过时 copy_on_skip_or_fail，目标已存在 | 不覆盖 | 目标已存在则跳过写入，只更新 metadata；若之前是旧的一次运行结果，会保留旧文件而非用原文件覆盖。 |
| copy_unsupported 多次写同一路径 | 可覆盖 | 同一输入目录下同一相对路径只会被复制一次；若两次运行输入不同但输出目录相同，后一次会覆盖前一次的同名文件。 |
| 并行处理同一输出目录 | 未加锁 | 当前设计假定单进程顺序/并行写不同文件；若多进程同时写同一 output_dir，可能产生竞态，需由调用方避免。 |

**结论**：在单进程、单次批量 run 的前提下，支持/不支持文件路径不重叠，不会出现「同一路径既被转换又被 copy_unsupported」的冲突。唯一需注意：**copy_on_skip_or_fail 目标已存在时不覆盖**，若希望“失败时一定用原文件覆盖”，需在调用前删除目标或单独逻辑。

---

## 4. 相关模块

- **shared_utils**: `smart_file_copier`（copy_on_skip_or_fail、smart_copy_with_structure）、`file_copier`（copy_unsupported_files、verify_output_completeness、should_copy_file）。
- **cli_runner**: `run_auto_command` 中在 process_directory 结束后调用 copy_unsupported_files 与 verify_output_completeness；单文件失败时调用 copy_on_skip_or_fail。
- **各工具**：vid_hevc / vid_av1 / img_hevc / img_av1 在 Skip/失败分支内调用 `shared_utils::copy_on_skip_or_fail`，将原文件复制到 output_dir。
