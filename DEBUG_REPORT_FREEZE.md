# 故障分析报告：FFmpeg 进程死锁 (Freeze Analysis)

> **日期**: 2025-12-13
> **问题**: 大文件处理过程中的随机卡死/冻结 (968.4s/1206.1s)
> **严重性**: 严重 (阻断型 Bug)

## 1. 现象描述

用户在处理 `01.mp4` (时长 1206s) 时，进度条卡死在 80.3% 处，不再更新，且 CPU/GPU 占用可能归零或保持死锁状态。前序较短文件处理正常。

```text
⏳ 🚀 GPU (Apple VideoToolbox) 80.3% | 968.4s/1206.1s | 219fps | 3.64x
```

## 2. 根因分析 (Root Cause Analysis)

经过对 `shared_utils/src/video_explorer.rs` 代码的审查，确认为经典的 **Standard I/O Pipe Deadlock (管道死锁)** 问题。

### 死锁原理

1.  **双管道配置**:
    代码中配置了 `ffmpeg` 同时输出到 `stdout` (用于进度条) 和 `stderr` (用于日志/警告)。
    ```rust
    // L1417-1418
    cmd.stdout(Stdio::piped())
       .stderr(Stdio::piped());
    ```

2.  **单线程读取**:
    Rust 主线程在一个 `for` 循环中不断读取 `stdout` 以更新 UI 进度条。
    ```rust
    // L1427
    if let Some(stdout) = child.stdout.take() {
        // ...只读取 stdout...
    }
    // ⚠️ 此时 stderr 的句柄被忽略，没有任何人读取它
    ```

3.  **缓冲区溢出**:
    *   操作系统为每个管道（Pipe）分配有限的缓冲区（通常为 64KB）。
    *   `ffmpeg` 在默认日志级别下，会向 `stderr` 输出各种信息（Banner、配置信息、编码过程中的警告）。
    *   对于长视频或特定编码场景（本次案例中的 `01.mp4`），积累的日志量超过了 64KB。
    *   **死锁触发**: `ffmpeg` 试图写入 `stderr` -> 缓冲区满 -> `ffmpeg` 阻塞等待读取 -> Rust 进程阻塞在 `stdout` 等待 `ffmpeg` 输出进度 -> **双方互相等待，产生死锁**。

### 为什么是这个文件？
*   `01.mp4` 时长最长 (1206s)，产生的日志/警告可能更多。
*   短文件可能日志量未填满 64KB 缓冲区，因此侥幸通过。

## 3. 解决方案 (Resolution)

必须确保 `stdout` 和 `stderr` 同时被消耗。由于这是同步代码，不能简单的在单线程中同时读取两个阻塞流。

### 方案 A: 后台线程排空 Stderr (推荐)
启动一个独立的后台线程，专门负责读取并丢弃（或记录） `stderr` 的内容，防止缓冲区填满。

```rust
// 伪代码示例
let mut stderr = child.stderr.take().expect("failed to capture stderr");
std::thread::spawn(move || {
    use std::io::Read;
    let mut buffer = [0; 1024];
    while let Ok(n) = stderr.read(&mut buffer) {
        if n == 0 { break; }
        // 持续消耗数据，防止阻塞
    }
});
```

### 方案 B: 禁用 Stderr (临时)
将 `stderr` 重定向到 `/dev/null`。缺点是如果 `ffmpeg` 报错，无法获取错误详情。

```rust
cmd.stderr(Stdio::null()); // 风险：丢失错误信息
```

## 4. 结论

这是一个典型的并发编程陷阱。为了保证工具的健壮性，必须在下一个版本中修复此 I/O 处理逻辑。建议采用 **方案 A** 以保留错误诊断能力。
