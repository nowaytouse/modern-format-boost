# 日志系统改进说明

## 问题分析

### 原有问题
1. **终端输出完整，文件输出不完整**：成功/失败消息使用 `println!()` 输出到 stdout，但日志系统只捕获 stderr
2. **文件日志缺少媒体处理结果**：图像和视频转换的成功/失败信息没有写入文件日志
3. **App模式日志分散**：双击运行时产生3个独立日志文件（drag_drop、img_hevc_run、vid_hevc_run）

## 解决方案

### 1. 修复成功消息输出 (img_hevc/src/main.rs, img_av1/src/main.rs)

**修改前：**
```rust
println!("✅ {}", output.message);  // 输出到 stdout，不被日志系统捕获
```

**修改后：**
```rust
log_eprintln!("✅ {}", output.message);  // 输出到 stderr，同时写入文件日志
```

**效果：**
- 终端显示：保持简洁明了的成功消息
- 文件日志：现在包含所有成功/失败信息，成为最全面的记录

### 2. 日志合并功能 (scripts/drag_and_drop_processor.sh)

**新增函数：**
```bash
merge_run_logs() {
    # 仅在 App 模式下合并日志
    if [[ -n "$FROM_APP" ]]; then
        # 查找最新的 img 和 vid 日志
        # 合并为单个 merged_*.log 文件
        # 删除原始的3个独立日志
    fi
}
```

**调用位置：**
- 在 `show_summary()` 函数末尾，用户按键退出前调用
- 仅当通过 App 双击运行时执行合并

### 3. App 模式检测 (Modern Format Boost.app)

**修改：**
```bash
# 设置环境变量标识 App 模式
export FROM_APP=1 && '$PROCESSOR_SCRIPT' '$SELECTED_DIR'
```

**效果：**
- 拖拽运行：设置 FROM_APP=1
- 双击运行：设置 FROM_APP=1
- 终端直接运行：不设置，保持3个独立日志

## 日志输出层级

### 终端输出（简洁）
- 进度信息：实时显示处理进度
- 成功消息：`✅ JPEG lossless transcode conversion successful: size reduced 15.0%`
- 统计摘要：XMP merge、Images 成功/失败计数
- 最终报告：总体成功率和统计

### 文件输出（全面）
- **所有终端输出内容**
- 详细的图像质量分析（size、format、content_type、complexity等）
- Tracing 事件（带时间戳和级别）
- 外部工具调用日志（cjxl、ffmpeg等）
- 错误堆栈和调试信息

### App 模式合并日志
当通过双击 App 运行时，生成单个 `merged_*.log` 文件，包含：
1. 🔧 Drag & Drop Script Log（脚本执行日志）
2. 🖼️ Image Processing Log（图像处理完整日志）
3. 🎬 Video Processing Log（视频处理完整日志）

## 使用方式

### 终端运行（保持3个独立日志）
```bash
./scripts/drag_and_drop_processor.sh /path/to/folder
```
生成：
- `logs/drag_drop_2026-03-09_14-30-00.log`
- `logs/img_hevc_run_2026-03-09_14-30-05.log`
- `logs/vid_hevc_run_2026-03-09_14-30-10.log`

### App 运行（自动合并为单个日志）
双击 `Modern Format Boost.app` 或拖拽文件夹到 App 图标

生成：
- `logs/merged_2026-03-09_14-30-00.log`（包含所有内容）

## 技术细节

### 日志宏说明
- `log_eprintln!()`：输出到 stderr + 文件日志（INFO级别）
- `verbose_eprintln!()`：仅在 --verbose 模式输出到终端，始终写入文件（DEBUG级别）
- `quiet_eprintln!()`：尊重 quiet 模式，可被静默

### 日志级别
- **TRACE**：最详细，包含所有调试信息
- **DEBUG**：详细信息，包括进度行
- **INFO**：常规信息，成功/失败消息
- **WARN**：警告信息
- **ERROR**：错误信息

### 文件日志特性
- ANSI 转义序列自动剥离（纯文本，无颜色代码）
- 64KB 缓冲，每行立即刷新（崩溃不丢失）
- Unix 文件锁（flock LOCK_EX）防止并发截断
- 自动轮转和清理（可配置）

## 验证方法

### 1. 检查文件日志是否包含成功消息
```bash
grep "✅.*conversion successful" logs/img_hevc_run_*.log
```
应该能看到所有成功转换的消息

### 2. 检查 App 模式日志合并
双击运行后，检查 `logs/` 目录：
```bash
ls -lh logs/merged_*.log
```
应该只有一个合并日志，包含所有内容

### 3. 对比终端和文件输出
终端应该简洁明了，文件应该包含所有详细信息（包括终端显示的内容）

## 注意事项

1. **重新编译**：修改 Rust 代码后需要重新编译
   ```bash
   ./scripts/smart_build.sh img_hevc vid_hevc
   ```

2. **日志时间戳**：img/vid 日志的时间戳可能比 drag_drop 日志晚几秒（正常现象）

3. **日志大小**：合并日志可能较大（几MB到几十MB），取决于处理的文件数量

4. **向后兼容**：终端直接运行时保持原有行为（3个独立日志）
