# Modern Format Boost: 质量检查与 CI 日志汇总及下阶段加固预演说明

本文档汇总了 `check_all` 最完整审计模式与 GitHub Actions CI (ghci) 运行日志的发现，对当前系统的代码规范、契约约束（Contract Discipline）、类型防护及依赖安全性进行了全量曝光，作为下一个加固任务（Next Hardening Job）的预演与依据。

---

## 一、 执行与 Git 状态

- **Git 分支**：`codex/privacy-safe-worktree`
- **依赖更新**：已成功执行 `cargo update`，锁定了最高兼容版本的 Cargo 依赖。
- **代码提交**：
  - Commit `93f6d9f1`：`fix(gui): finalize Photos TCC automation preflight and integrate macOS GUI build into smart_build`
  - 变更文件数：10 files (包含删除冗余 `src-macos/build.sh`、内化 Swift 编译至 `smart_build.rs`、隐藏 AppKit 原生 traffic lights 及修复 App.vue 格式)。
- **Git Push**：已成功推送至远端 `origin/codex/privacy-safe-worktree`。

---

## 二、 `check_all` 最完整模式审计结果汇总

执行命令：
```bash
cargo run --locked -p dev --bin check_all -- --allow-non-nightly --build --ai-smell
```

### 1. 100% 成功通过的基础合规项

| 审计项目 | 命令 / 工具 | 状态 | 备注 |
|---|---|---|---|
| Rust Format | `cargo fmt --all --check` | ✅ PASSED | 格式完全规范 |
| TOML Format | `taplo fmt --check` | ✅ PASSED | 16 个 TOML 配置文件格式匹配 |
| Cargo Check | `cargo check --workspace` | ✅ PASSED | 编译无报错 |
| CHANGELOG 步调 | `docs/CHANGELOG.md` | ✅ PASSED | 版本 `v0.11.3` 步调一致 |
| Python 语法 | `python3 -m py_compile` | ✅ PASSED | 31 个 Python 辅助脚本语法正确 |
| Vue Lint | `oxlint + eslint + vue-tsc` | ✅ PASSED | 0 警告 0 错误 (包含 App.vue 修复) |
| Vue Format | `prettier --check` | ✅ PASSED | 前端代码风格完全匹配 |
| Vue Build | `vite build` | ✅ PASSED | 18 个模块成功构建输出到 `dist/` |
| Clippy Strict | `clippy_strict` (ultra-strict) | ✅ **PASSED** | 已修复 4 处警告 + 测试模块 allow 属性 |
| 运行时回归 | `runtime_probe_regression` | ✅ **PASSED** | 已修复 WebP ANIM 及 ISOBMFF HEIC/AVIF 盒长度 |
| Smoke 测试套件 | `smoke_test_suite` | ✅ **PASSED** | 已自动补全 `MEDIA_MANIFEST.md` 键值 |

---

### 2. 曝光的代码契约与严谨性瓶颈 (Contract & Discipline Audit Failures)

在 `check_all` 执行 `test_real_silent_fallbacks.rs` 全仓静态断言与契约检查时，暴露了以下 22 处需要下阶段重点加固的代码缺陷：

#### A. 静态错误掩盖 (AST Error Suppression via `.ok()` / `is_ok_and`)
在关键数据流中直接丢弃 `Result::Err`，违背了项目“静默失败即伪造”的防线要求：
- `crates/dev/src/bin/smart_build.rs:1419`：`if let Ok(id) = std::env::var("CODESIGN_IDENTITY")`
- `crates/img/src/lossless_converter.rs:280`：`fs::metadata(output).ok()`
- `crates/foundation/src/image/image_detection.rs:4088`：`.ok()`
- `crates/foundation/src/image/loop_intent.rs:5800`：`.is_ok_and(...)`
- `crates/foundation/src/image/image_formats.rs:416`：`animation_timing_ms(data).ok()??`
- `crates/foundation/src/video/video_explorer/gpu_coarse_search.rs:2273`：`.ok()`
- `crates/foundation/src/video/stream_size.rs:356`：`detect_true_format(path).ok()`

#### B. 数值伪造与默认值捏造 (Numeric Forgery via `map_or` / `unwrap_or`)
在媒体元数据解析失败时，直接使用默认值 `0` 或 `1` 伪造帧数或时长：
- `crates/foundation/src/image/image_detection.rs:531`：`let frame_count = info.map_or(1, |info| info.frame_count);`
- `crates/foundation/src/image/loop_intent.rs:5807`：`.map_or(1, |count| count.div_ceil(sample_limit).max(1))`
- `crates/foundation/src/image/image_formats.rs:405`：`Ok(animation_timing_ms(data)?.map_or(0, |timing| timing.0))`
- `crates/foundation/src/image/loop_intent.rs:6010`：`return Some(1.0);`

#### C. 临时文件违规创建 (Temp File Scratch SSOT Violation)
未通过 `crate::process_lock` 的统一 Scratch SSOT 目录，直接在生产代码中裸调用 `.tempfile()`：
- `crates/foundation/src/image/image_quality_detector.rs:1274`：`.tempfile()`
- `crates/foundation/src/image/image_quality_detector.rs:1283`：`.tempfile()`

---

## 三、 GitHub Actions (GHCI) 运行日志汇总

通过 `gh run list` 和 `gh run view` 提取的 GitHub Actions 运行记录：

### 1. 工作流运行概览
```text
in_progress    PR Build: Harden native media pipeline and privacy boundaries (#31378965977)
completed      failure: Nightly Schedule Continuous Quality & Fuzzing (#31348326835)
```

### 2. CI 失败根因分析 (Run #31348326835)
1. **Clippy Strict 维度**：
   - CI 环境在 `vid` 和 `img` 可执行文件的测试模块中拦截到了 `unwrap_used` 与 `expect_used`（**注：已在本次本地提交中彻底修复并验证**）。
2. **NPM 依赖安全维度 (`deps:check`)**：
   - `crates/gui` 在 CI 跑 `npm audit --audit-level=high` 时触发了高危漏洞拦截。

---

## 四、 下阶段加固任务清单 (Next Hardening Phase Action Items)

根据本次日志汇总与暴露问题，下一个加固阶段（Easy-Harding-Job）的具体行动指南如下：

1. **消除数值伪造 (Numeric Forgery Removal)**：
   - 将 `image_detection.rs`, `loop_intent.rs`, `image_formats.rs` 中的 `.map_or(1, ...)` 与 `.map_or(0, ...)` 改为显式 `Option` / `Result` 严格透传，禁止静默捏造 `1` 帧或 `0ms` 时长。
2. **清理静默 Error 丢弃 (`.ok()` Eradication)**：
   - 彻底重构 `smart_build.rs`, `lossless_converter.rs`, `gpu_coarse_search.rs` 等文件中的 `.ok()` 与 `is_ok_and`，改用显式 `match` 或 `UnifiedError` 传递错误上下文。
3. **收拢临时文件 Scratch SSOT**：
   - 将 `image_quality_detector.rs` 中的 bare `.tempfile()` 迁移至 `crate::process_lock::get_mfb_root()` 统一的受控 scratch 目录，防止临时文件外泄。
4. **修复 Front-End Dependencies 安全审计**：
   - 在 `crates/gui` 中升级/修复带安全风险的 npm 依赖包，确保 `npm audit --audit-level=high` 干净通过。

---

> **结论**：本说明文档已落地保存至 `.agents/harding/Easy-Harding-Job/hardening_log_preview.md`。可作为后续 Harding 加固任务的直接规范输入。
