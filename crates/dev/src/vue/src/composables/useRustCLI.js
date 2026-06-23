/**
 * Rust CLI 调用封装
 * 🔥 基于新的 pixly-converter 内核
 * 命令格式: pixly-converter convert <INPUT> --format <FORMAT> [OPTIONS]
 */

import { ref } from "vue";
import { logger, LOG_KEYS } from "../utils/logger";
import { useI18n } from "./useI18n";

export function useRustCLI() {
  const { t } = useI18n();
  const isConverting = ref(false);
  const progress = ref(0);
  const currentFile = ref("");
  const rustBinaryPath = ref(null);

  /**
   * 初始化：查找 pixly-eagle-core 共享二进制文件
   */
  const initRustCLI = async () => {
    if (rustBinaryPath.value) {
      logger.debug(LOG_KEYS.RUST_CLI_EXEC, "Rust CLI already initialized", {
        path: rustBinaryPath.value,
      });
      return rustBinaryPath.value;
    }

    const { spawn } = require("child_process");
    const path = require("path");
    const fs = require("fs");

    // 🔥 获取插件根目录
    // Eagle 环境：从 window.location 获取实际路径
    // 开发环境：使用 __dirname
    let pluginRoot;
    if (window.location && window.location.pathname) {
      // Eagle: file:///path/to/eagle-plugins/xxx/dist/index.html
      // 需要解析出插件目录（dist 的父目录）
      const htmlPath = decodeURIComponent(window.location.pathname);

      // 检查是否在 dist/ 目录中（Eagle 环境）
      if (htmlPath.includes("/dist/")) {
        // 从 /path/to/plugin/dist/index.html 提取 /path/to/plugin
        pluginRoot = path.dirname(path.dirname(htmlPath));
      } else {
        // 开发环境或其他情况
        pluginRoot = path.dirname(htmlPath);
      }
    } else {
      // Fallback: 使用 __dirname（Node.js 环境）
      pluginRoot = path.resolve(__dirname, "../..");
    }

    // 🔥 共享二进制路径（符号链接或复制）
    const possiblePaths = [
      path.join(pluginRoot, "dist/bin/pixly-eagle-core"), // Eagle: plugin/dist/bin/
      path.join(pluginRoot, "bin/pixly-eagle-core"), // 开发: plugin/bin/ (符号链接)
      path.join(pluginRoot, "../shared/bin/pixly-eagle-core"), // 开发: plugin/shared/bin/
      // 移除所有外部路径，仅使用插件内嵌或共享二进制
    ];

    logger.info(LOG_KEYS.RUST_CLI_EXEC, "Searching for pixly-eagle-core", {
      pluginRoot,
      searchPaths: possiblePaths.length,
    });

    // 生产模式：检测 Eagle 环境
    const isEagleEnv =
      typeof window !== "undefined" &&
      window.eagle !== undefined &&
      typeof window.eagle.plugin !== "undefined";

    const isDev = process.env.NODE_ENV === "development";

    if (!isDev && !isEagleEnv) {
      const errorMsg = "This plugin can ONLY run inside Eagle.";
      logger.error(LOG_KEYS.RUST_CLI_ERROR, errorMsg);
      throw new Error(errorMsg);
    }

    // 设置环境变量供 Rust 检测
    if (isEagleEnv) {
      process.env.EAGLE_PLUGIN = "true";
      logger.info(LOG_KEYS.RUST_CLI_EXEC, "Eagle environment detected", {
        EAGLE_PLUGIN: "true",
      });
    } else {
      logger.warn(
        LOG_KEYS.RUST_CLI_EXEC,
        "Development mode: Eagle environment not detected",
      );
    }

    for (const p of possiblePaths) {
      try {
        const resolved = path.resolve(p);

        // 检查文件是否存在
        if (!fs.existsSync(resolved)) {
          logger.debug(LOG_KEYS.RUST_CLI_EXEC, "Path not found", {
            path: resolved,
          });
          continue;
        }
        logger.debug(LOG_KEYS.RUST_CLI_EXEC, "Found file", { path: resolved });

        // 测试执行（开发模式传递 --dev 参数）
        const testArgs = isDev ? ["--dev", "--version"] : ["--version"];
        logger.debug(LOG_KEYS.RUST_CLI_EXEC, "Testing executable", {
          path: p,
          args: testArgs,
        });

        const proc = spawn(p, testArgs, {
          env: {
            ...process.env,
            EAGLE_PLUGIN: "true",
            RUST_BACKTRACE: "1",
          },
        });
        let output = "";
        let error = "";

        proc.stdout.on("data", (data) => {
          const str = data.toString();
          output += str;
          // 🔥 实时打印 Rust 日志到控制台
          console.log("%c[Rust CLI] " + str.trim(), "color: #00aaff");
        });

        proc.stderr.on("data", (data) => {
          const str = data.toString();
          error += str;
          // 🔥 实时打印 Rust 错误到控制台
          console.error("%c[Rust CLI Error] " + str.trim(), "color: #ff5555");
        });

        const _success = await new Promise((resolve) => {
          proc.on("close", (code) => {
            logger.debug(LOG_KEYS.RUST_CLI_EXEC, "Version check result", {
              path: p,
              code,
              output: output.trim(),
              error: error.trim(),
            });
            resolve(code === 0);
          });
          proc.on("error", (err) => {
            logger.debug(LOG_KEYS.RUST_CLI_EXEC, "Spawn error", {
              path: p,
              error: err.message,
            });
            resolve(false);
          });
        });

        // 验证版本
        if (output.includes("pixly-eagle-core")) {
          logger.info(LOG_KEYS.RUST_CLI_EXEC, "✅ Binary verified", {
            path: resolved,
            version: output.trim(),
          });

          // 🔥 强制输出路径给用户看
          console.log(
            "%c[Pixly Debug] Using Binary: " + resolved,
            "background: #222; color: #bada55; font-size: 14px",
          );

          rustBinaryPath.value = resolved;
          return resolved;
        }
      } catch (e) {
        logger.debug(LOG_KEYS.RUST_CLI_EXEC, "Error testing path", {
          path: p,
          error: e.message,
        });
        continue;
      }
    }

    // 🔥 未找到，提供详细错误信息
    const errorMsg = `pixly-eagle-core not found. Searched paths:\n${possiblePaths.map((p) => `  - ${p}`).join("\n")}\n\nPlease:\n1. Run setup: cd ${pluginRoot} && npm run setup\n2. Or build: cd ${path.join(pluginRoot, "../shared")} && bash build.sh`;

    logger.error(LOG_KEYS.RUST_CLI_ERROR, errorMsg);
    throw new Error(errorMsg);
  };

  /**
   * 执行图像转换
   * 🔥 修复：正确的命令格式 pixly-converter convert <INPUT> --format <FORMAT> [OPTIONS]
   * @param {Array} files - 文件列表
   * @param {Object} options - 转换选项
   * @param {Function} onProgress - 进度回调 (fileIndex, fileName, status)
   */
  const convertImages = async (files, options, onProgress = null) => {
    isConverting.value = true;
    progress.value = 0;

    try {
      // 🔥 初始化Rust CLI
      logger.info(LOG_KEYS.RUST_CLI_EXEC, "Initializing Rust CLI");
      await initRustCLI();
      logger.info(LOG_KEYS.RUST_CLI_EXEC, "Rust CLI initialized", {
        path: rustBinaryPath.value,
      });

      const results = [];
      const path = require("path");

      // 🔥 过滤掉 XMP 文件，只转换媒体文件
      const mediaFiles = files.filter((f) => !f.isXmp);

      logger.info(LOG_KEYS.CONVERT_START, "Starting batch conversion", {
        total: files.length,
        media: mediaFiles.length,
        xmp: files.length - mediaFiles.length,
        mediaFileDetails: mediaFiles.map((f) => ({
          name: f.name,
          ext: f.ext,
          hasPath: !!f.path,
          hasXmp: f.hasXmp,
          xmpPath: f.xmpPath,
        })),
      });

      if (mediaFiles.length === 0) {
        throw new Error(
          "No media files to convert (all files are XMP or invalid)",
        );
      }

      for (let i = 0; i < mediaFiles.length; i++) {
        const file = mediaFiles[i];
        currentFile.value = file.name;
        progress.value = Math.round((i / mediaFiles.length) * 100);

        // 🔍 调用进度回调 - 开始处理
        if (onProgress) {
          onProgress(i + 1, mediaFiles.length, file.name, "processing");
        }

        // 🔥 验证文件路径
        if (!file.path) {
          logger.error(LOG_KEYS.CONVERT_ERROR, "File path is undefined", {
            file: file.name,
            fileObject: file,
          });
          throw new Error(`File path is undefined for: ${file.name}`);
        }

        // 🔮 滤镜模式：如果 format 为 null，使用文件原格式
        let cleanFormat;
        if (options.format === null || options.isFilterMode) {
          // 🔮 滤镜模式：保持原格式
          cleanFormat = (file.ext || "")
            .toLowerCase()
            .replace(/['"`.]/g, "")
            .trim();
          if (!cleanFormat || cleanFormat.length === 0) {
            cleanFormat = "avif"; // fallback
          }
          logger.info(
            LOG_KEYS.RUST_CLI_EXEC,
            "🔮 Filter mode: using original format",
            {
              file: file.name,
              originalFormat: cleanFormat,
            },
          );
        } else {
          // 🔧 Bug Fix: 彻底清理格式字符串
          // Eagle有时返回：`"jxl"`, `."jxl"`, 或其他奇怪格式
          cleanFormat = options.format || "avif";
          if (typeof cleanFormat === "string") {
            cleanFormat = cleanFormat
              .replace(/['"`.]/g, "") // 移除所有引号和点号
              .trim()
              .toLowerCase();
          }

          if (!cleanFormat || cleanFormat.length === 0) {
            cleanFormat = "avif"; // 默认格式
          }
        }

        // 🔥 生成输出路径（原地替换：同目录，新扩展名）
        const inputPath = file.path;
        const outputPath = path.join(
          path.dirname(inputPath),
          `${path.basename(inputPath, path.extname(inputPath))}.${cleanFormat}`,
        );

        logger.info(LOG_KEYS.CONVERT_START, "Converting file", {
          input: inputPath,
          output: outputPath,
          format: cleanFormat,
        });

        // 构建命令参数
        const args = [
          "convert",
          inputPath,
          "--format",
          cleanFormat,
          "--quality",
          options.quality.toString(),
          "--output",
          outputPath,
        ];

        // 🔥 Debug: Log arguments
        logger.info(LOG_KEYS.RUST_CLI_EXEC, "Executing Rust CLI", {
          binary: rustBinaryPath.value,
          args: args,
        });
        console.log(
          "%c[Rust CLI Args] " + JSON.stringify(args),
          "color: #ff00ff",
        );

        // 🔥 传递输入格式（来自Eagle元数据）
        // 解决 Eagle 文件没有扩展名导致 image crate 无法识别格式的问题
        if (file.ext) {
          let cleanInputFormat = file.ext;
          if (typeof cleanInputFormat === "string") {
            cleanInputFormat = cleanInputFormat
              .replace(/['"`.]/g, "")
              .trim()
              .toLowerCase();
          }
          if (cleanInputFormat && cleanInputFormat.length > 0) {
            args.push("--input-format", cleanInputFormat);
            logger.debug(
              LOG_KEYS.RUST_CLI_EXEC,
              "Using input format from Eagle metadata",
              {
                original: file.ext,
                cleaned: cleanInputFormat,
              },
            );
          }
        }

        // 添加预设参数
        if (options.preset) {
          args.push("--preset", options.preset);
        }

        // JXL 参数
        if (cleanFormat === "jxl") {
          if (options.effort !== undefined)
            args.push("--effort", options.effort.toString());
          if (options.distance !== undefined)
            args.push("--distance", options.distance.toString());
          if (options.lossless) args.push("--lossless");
          if (options.jpegLossless) args.push("--jpeg-lossless");
          if (options.modular) args.push("--modular");
          if (options.progressive) args.push("--progressive");
          if (options.bitDepth) args.push("--bit-depth", options.bitDepth);
          if (options.colorSpace)
            args.push("--color-space", options.colorSpace);
        }

        // AVIF 参数
        if (cleanFormat === "avif") {
          if (options.speed !== undefined)
            args.push("--speed", options.speed.toString());
          if (options.minQuantizer !== undefined)
            args.push("--min-quantizer", options.minQuantizer.toString());
          if (options.maxQuantizer !== undefined)
            args.push("--max-quantizer", options.maxQuantizer.toString());
          if (options.chroma) args.push("--chroma", options.chroma);
          if (options.tiles) args.push("--tiles", options.tiles);
        }

        // WebP 参数
        if (cleanFormat === "webp") {
          if (options.method !== undefined)
            args.push("--method", options.method.toString());
          if (options.lossless) args.push("--lossless");
          if (options.filterStrength !== undefined)
            args.push("--filter-strength", options.filterStrength.toString());
          if (options.sharpness !== undefined)
            args.push("--sharpness", options.sharpness.toString());
        }

        // HEIC 参数
        if (cleanFormat === "heic") {
          if (options.encoder) args.push("--encoder", options.encoder);
          if (options.lossless) args.push("--lossless");
          if (options.thumbnail) args.push("--thumbnail");
          if (options.chroma) args.push("--chroma", options.chroma);
        }

        // 🔥 快捷工具选项
        const tools = options.quickTools || {};

        // XMP合并
        if (tools.autoMergeXmp !== false) {
          args.push("--merge-xmp");

          // 如果文件有 XMP 路径，直接传递给 Rust CLI（避免扫描）
          if (file.xmpPath) {
            args.push("--xmp-path", file.xmpPath);
            logger.info(
              LOG_KEYS.RUST_CLI_EXEC,
              "Passing XMP path to Rust CLI",
              {
                xmpPath: file.xmpPath,
              },
            );
          }
        }

        // 文件名规范化
        if (tools.normalizeFilenames) {
          args.push("--normalize-filenames");
        }

        // AI 文件验证 (Magika)
        if (tools.fileValidation) {
          args.push("--validate-files");
        }

        // 格式修正
        if (tools.formatCorrection) {
          args.push("--format-correction");
        }

        // 🔥 AI 智能选项
        const ai = options.aiOptions || {};

        // 启用 AI 模式 (统一参数)
        if (ai.smartQuality || ai.autoOptimize) {
          args.push("--ai");
          if (options.optimizeMode) {
            args.push("--optimize-mode", options.optimizeMode);
          }
        }

        // SSIM 质量验证
        if (ai.ssimValidation) {
          args.push("--check-quality");
        }

        // 智能预处理
        if (ai.smartPreprocess) {
          args.push("--preprocess");
        }

        // GPU 加速 (默认启用)
        if (ai.gpuAccel !== false) {
          args.push("--gpu");
        }

        logger.debug(LOG_KEYS.RUST_CLI_EXEC, "Executing command", {
          args: args.join(" "),
        });

        try {
          const result = await executeRustCLI(args);
          results.push({
            success: true,
            file: file.name,
            input: inputPath,
            output: outputPath,
            hasXmp: file.hasXmp,
            xmpId: file.xmpId, // 🔥 传递 XMP ID 用于删除
            stdout: result.stdout,
          });
          logger.info(LOG_KEYS.CONVERT_SUCCESS, "File converted successfully", {
            file: file.name,
            hasXmp: file.hasXmp,
            xmpId: file.xmpId,
          });

          // 🔍 调用进度回调 - 成功
          if (onProgress) {
            onProgress(i + 1, mediaFiles.length, file.name, "success");
          }
        } catch (fileError) {
          results.push({
            success: false,
            file: file.name,
            input: inputPath,
            error: fileError.message,
          });
          logger.error(LOG_KEYS.CONVERT_ERROR, "File conversion failed", {
            file: file.name,
            error: fileError.message,
          });

          // 🔍 调用进度回调 - 失败
          if (onProgress) {
            onProgress(
              i + 1,
              mediaFiles.length,
              file.name,
              "error",
              fileError.message,
            );
          }
        }
      }

      progress.value = 100;

      // 🔥 统计转换结果
      const successCount = results.filter((r) => r.success).length;
      const failCount = results.filter((r) => !r.success).length;
      const xmpMergedCount = results.filter(
        (r) => r.success && r.hasXmp,
      ).length;

      logger.info(LOG_KEYS.CONVERT_SUCCESS, "Batch conversion complete", {
        total: mediaFiles.length,
        success: successCount,
        failed: failCount,
        xmpMerged: xmpMergedCount,
      });

      return {
        success: successCount > 0,
        results,
        summary: {
          total: mediaFiles.length,
          success: successCount,
          failed: failCount,
          xmpMerged: xmpMergedCount,
        },
      };
    } catch (error) {
      logger.error(LOG_KEYS.CONVERT_ERROR, "Batch conversion failed", {
        error: error.message,
        stack: error.stack,
        rustBinaryPath: rustBinaryPath.value,
      });

      return { success: false, error: error.message };
    } finally {
      isConverting.value = false;
      currentFile.value = "";
    }
  };

  /**
   * 执行视频转换
   * 🔮 滤镜模式：当 codec=null 时，保持原编码器只优化质量
   */
  const convertVideos = async (files, options) => {
    isConverting.value = true;
    progress.value = 0;

    try {
      await initRustCLI();
      const results = [];
      const path = require("path");

      // 🔮 检测滤镜模式
      const isFilterMode = options.codec === null || options.isFilterMode;
      if (isFilterMode) {
        logger.info(
          LOG_KEYS.RUST_CLI_EXEC,
          "🔮 Video filter mode: optimizing without codec change",
          {
            fileCount: files.length,
          },
        );
      }

      for (let i = 0; i < files.length; i++) {
        const file = files[i];
        currentFile.value = file.name;
        progress.value = Math.round((i / files.length) * 100);

        // 🔥 生成输出路径
        const inputPath = file.path;

        // 🔮 滤镜模式：保持原容器格式
        const originalExt = (
          file.ext ||
          path.extname(inputPath).slice(1) ||
          ""
        ).toLowerCase();
        const container = isFilterMode
          ? originalExt
          : options.container || "mp4";

        // 🔮 滤镜模式：输出到同一文件（原地优化）
        const outputPath = isFilterMode
          ? inputPath // 原地优化
          : path.join(
              path.dirname(inputPath),
              `${path.basename(inputPath, path.extname(inputPath))}.${container}`,
            );

        logger.info(LOG_KEYS.CONVERT_START, "Converting video", {
          input: inputPath,
          output: outputPath,
          container,
          isFilterMode,
        });

        // 🔥 使用 video 子命令（不是 convert）
        const args = ["video", inputPath, outputPath];

        // 🔮 滤镜模式：不指定 codec，让后端自动检测并保持原编码
        if (!isFilterMode && options.codec) {
          args.push("--codec", options.codec);
        }
        args.push("--container", container);

        // 🔥 视频编码参数
        if (options.crf !== undefined)
          args.push("--crf", options.crf.toString());
        if (options.preset) args.push("--preset", options.preset);

        // 🔥 AI 智能参数 - 滤镜模式下也启用AI优化
        if (options.useAI) {
          args.push("--ai");
          if (options.optimizeMode) {
            args.push("--optimize-mode", options.optimizeMode);
          }
        }

        // 🔥 GPU 加速（默认启用）
        if (options.enableGPU !== false) {
          args.push("--gpu");
        }

        // 🔮 滤镜模式：禁用动画转视频（保持原格式）
        if (!isFilterMode && options.enableVideoForAnimation) {
          args.push("--video-for-animation");
        }

        // 🔥 场景检测
        if (options.enableSceneDetection) {
          args.push("--scene-detection");
        }

        // 🔥 VMAF 质量验证
        if (options.enableVMAF) {
          args.push("--vmaf");
        }

        // 🔥 两遍编码
        if (options.enableTwoPass) {
          args.push("--two-pass");
        }

        // 高级参数
        if (options.gop !== undefined)
          args.push("--gop", options.gop.toString());
        if (options.bframes !== undefined)
          args.push("--bframes", options.bframes.toString());
        if (options.refs !== undefined)
          args.push("--refs", options.refs.toString());
        if (options.meMethod) args.push("--me-method", options.meMethod);
        if (options.pixFmt) args.push("--pix-fmt", options.pixFmt);

        logger.debug(LOG_KEYS.RUST_CLI_EXEC, "Executing command", {
          args: args.join(" "),
        });

        const result = await executeRustCLI(args);
        results.push({
          success: true,
          input: inputPath,
          output: outputPath,
          stdout: result.stdout,
        });
      }

      progress.value = 100;
      return { success: true, results };
    } catch (error) {
      logger.error(LOG_KEYS.CONVERT_ERROR, "Video conversion failed", {
        error: error.message,
      });
      return { success: false, error: error.message };
    } finally {
      isConverting.value = false;
      currentFile.value = "";
    }
  };

  /**
   * 执行 Rust CLI 命令
   * 🔥 修复：添加 --dev 参数支持和 EAGLE_PLUGIN 环境变量
   */
  const executeRustCLI = (args) => {
    return new Promise((resolve, reject) => {
      const { spawn } = require("child_process");

      // 开发模式：添加 --dev 参数
      const isDev = process.env.NODE_ENV === "development";
      const isEagleEnv =
        typeof window !== "undefined" && window.eagle !== undefined;

      const finalArgs = isDev ? ["--dev", ...args] : args;

      logger.debug(LOG_KEYS.RUST_CLI_EXEC, "Executing pixly-eagle-core", {
        args: finalArgs.join(" "),
        devMode: isDev,
        eagleEnv: isEagleEnv,
      });

      // 🔥 设置完整的PATH环境变量（包含Homebrew等工具路径）
      const fullPath = [
        "/opt/homebrew/bin", // macOS Homebrew (Apple Silicon)
        "/opt/homebrew/sbin",
        "/usr/local/bin", // macOS Homebrew (Intel) / Linux
        "/usr/bin",
        "/bin",
        "/usr/sbin",
        "/sbin",
        process.env.PATH || "",
      ]
        .filter(Boolean)
        .join(":");

      const proc = spawn(rustBinaryPath.value, finalArgs, {
        env: {
          ...process.env,
          PATH: fullPath,
          EAGLE_PLUGIN: isEagleEnv ? "true" : undefined,
        },
      });

      let stdout = "";
      let stderr = "";

      proc.stdout.on("data", (data) => {
        const text = data.toString();
        stdout += text;

        // 🔥 解析进度信息
        const lines = text.split("\n");
        for (const line of lines) {
          if (line.trim()) {
            logger.debug(LOG_KEYS.RUST_CLI_EXEC, "Rust CLI output", { line });

            // 检测转换完成
            if (line.includes("✅ Conversion complete")) {
              progress.value = 100;
            }
          }
        }
      });

      proc.stderr.on("data", (data) => {
        const text = data.toString();
        stderr += text;
        logger.debug(LOG_KEYS.RUST_CLI_EXEC, "Rust CLI stderr", { text });
      });

      proc.on("close", (code) => {
        if (code === 0) {
          logger.info(LOG_KEYS.CONVERT_SUCCESS, "Conversion completed", {
            stdout: stdout.substring(0, 200),
          });
          resolve({ success: true, stdout, stderr });
        } else {
          logger.error(LOG_KEYS.CONVERT_ERROR, "Conversion failed", {
            code,
            stderr: stderr.substring(0, 500),
          });

          // 🔥 提供友好的错误信息（使用国际化）
          let errorMessage = stderr || `Exit code: ${code}`;

          // 检测常见错误
          if (stderr.includes("cjxl") && stderr.includes("not found")) {
            errorMessage = t("errors.jxlNotInstalled");
          } else if (
            stderr.includes("avifenc") &&
            stderr.includes("not found")
          ) {
            errorMessage = t("errors.avifNotInstalled");
          } else if (stderr.includes("No such file or directory")) {
            errorMessage = t("errors.fileNotFound");
          }

          reject(new Error(errorMessage));
        }
      });

      proc.on("error", (err) => {
        logger.error(LOG_KEYS.RUST_CLI_ERROR, "Spawn failed", {
          error: err.message,
        });
        reject(err);
      });
    });
  };

  /**
   * 单文件转换（图像）
   * 🔮 滤镜模式：当 disableFormatChange=true 时，保持原格式只优化质量
   */
  const convert = async (options) => {
    // 将单文件选项转换为批量格式
    const file = {
      path: options.inputPath,
      name: options.inputPath.split("/").pop(),
      ext: options.inputPath.split(".").pop(),
    };

    // 🔮 滤镜模式核心：如果禁用格式转换，使用原文件格式
    let targetFormat = options.format;
    if (options.disableFormatChange || options.format === null) {
      // 从文件扩展名获取原格式
      const originalExt = (file.ext || "").toLowerCase();
      targetFormat = originalExt || "avif"; // fallback to avif if no extension
      logger.info(
        LOG_KEYS.RUST_CLI_EXEC,
        "🔮 Filter mode: keeping original format",
        {
          originalFormat: targetFormat,
          disableFormatChange: options.disableFormatChange,
        },
      );
    }

    const result = await convertImages([file], {
      format: targetFormat,
      quality: options.quality || 85,
      quickTools: {
        autoMergeXmp: true,
        normalizeFilenames: false,
        fileValidation: options.enableFileValidation,
        formatCorrection: options.enableFormatCorrection,
      },
      aiOptions: {
        smartQuality: options.useAI,
        autoOptimize: options.useAI,
        ssimValidation: options.enableSSIM,
        smartPreprocess: options.enablePreprocess,
        gpuAccel: options.enableGPU,
      },
      optimizeMode: options.optimizeMode || "balanced",
    });

    return result.results?.[0] || result;
  };

  /**
   * 批量转换（图像）
   * 🔮 滤镜模式：当 disableFormatChange=true 时，每个文件保持原格式
   */
  const batchConvert = async (files, options, onProgress) => {
    // 🔮 滤镜模式：检测是否禁用格式转换
    const isFilterMode = options.disableFormatChange || options.format === null;

    if (isFilterMode) {
      logger.info(
        LOG_KEYS.RUST_CLI_EXEC,
        "🔮 Filter mode: batch converting with original formats",
        {
          fileCount: files.length,
        },
      );
    }

    // 转换选项格式
    const convertOptions = {
      // 🔮 滤镜模式：format 设为 null，在 convertImages 中根据每个文件的扩展名决定
      format: isFilterMode ? null : options.format || "avif",
      quality: options.quality || 85,
      quickTools: {
        autoMergeXmp: true,
        normalizeFilenames: false,
        fileValidation: options.enableFileValidation,
        formatCorrection: options.enableFormatCorrection,
      },
      aiOptions: {
        smartQuality: options.useAI,
        autoOptimize: options.useAI,
        ssimValidation: options.enableSSIM,
        smartPreprocess: options.enablePreprocess,
        gpuAccel: options.enableGPU,
      },
      optimizeMode: options.optimizeMode || "balanced",
      // 🔮 传递滤镜模式标志
      isFilterMode: isFilterMode,
    };

    let result;
    try {
      // 🔥 修复：onProgress 是函数，不是对象
      // convertImages 的第三个参数签名：onProgress(fileIndex, totalFiles, fileName, status, errorMsg?)
      const progressCallback = onProgress
        ? (current, total, fileName, status, errorMsg) => {
            onProgress({
              current,
              total,
              file: fileName,
              status,
              percentage: Math.round((current / total) * 100),
              error: errorMsg,
            });
          }
        : null;

      result = await convertImages(files, convertOptions, progressCallback);
    } catch (error) {
      logger.error(LOG_KEYS.CONVERT_ERROR, "Batch convert error", {
        error: error.message,
        stack: error.stack,
      });
      throw error;
    } finally {
      isConverting.value = false;
    }

    return result.results || [];
  };

  /**
   * 🆕 分析文件优化状态
   * 调用 analyze 命令获取压缩潜力预测
   *
   * @param {string} filePath - 文件路径
   * @param {string} [fileExt] - 文件扩展名（来自Eagle元数据，用于无扩展名文件）
   */
  const analyzeOptimizationStatus = async (filePath, fileExt) => {
    try {
      await initRustCLI();

      if (!rustBinaryPath.value) {
        logger.error(LOG_KEYS.RUST_CLI_ERROR, "❌ Rust binary path is null!");
        throw new Error("Rust CLI not initialized");
      }

      const args = ["analyze", filePath, "--json"];

      // 🔥 彻底清理扩展名（Eagle有时返回奇怪的格式）
      if (fileExt) {
        let cleanExt = fileExt;
        if (typeof cleanExt === "string") {
          cleanExt = cleanExt
            .replace(/['"`.]/g, "") // 移除所有引号和点号
            .trim()
            .toLowerCase();
        }
        if (cleanExt && cleanExt.length > 0) {
          args.push("--format", cleanExt);
          logger.debug(
            LOG_KEYS.RUST_CLI_EXEC,
            "Using format from Eagle metadata",
            {
              original: fileExt,
              cleaned: cleanExt,
            },
          );
        }
      }

      const { spawn } = require("child_process");

      return new Promise((resolve, reject) => {
        logger.info(LOG_KEYS.RUST_CLI_EXEC, "Analyzing optimization status", {
          binary: rustBinaryPath.value,
          args,
        });

        const child = spawn(rustBinaryPath.value, args, {
          stdio: ["ignore", "pipe", "pipe"],
          env: process.env,
        });

        let stdout = "";
        let stderr = "";

        child.stdout.on("data", (data) => {
          stdout += data.toString();
        });

        child.stderr.on("data", (data) => {
          stderr += data.toString();
        });

        child.on("close", (code) => {
          if (code !== 0) {
            logger.error(
              LOG_KEYS.RUST_CLI_ERROR,
              `❌ Analysis command failed with code ${code}`,
              {
                code,
                stderr,
                stdout,
              },
            );
            // 🔥 响亮报错而不是返回null
            reject(new Error(`Analysis failed: ${stderr || "Unknown error"}`));
            return;
          }

          try {
            // 🔥 修复：JSON 是多行的，需要解析完整的 JSON 对象
            // 🔧 Bug Fix: 清理可能存在的 BOM 或其他不可见字符
            const cleanStdout = stdout
              .replace(/^\uFEFF/, "") // Remove BOM
              // eslint-disable-next-line no-control-regex
              .replace(/[\x00-\x08\x0B\x0C\x0E-\x1F]/g, "") // Remove control characters
              .trim();

            // 找到第一个 { 和最后一个 } 之间的内容
            const firstBrace = cleanStdout.indexOf("{");
            const lastBrace = cleanStdout.lastIndexOf("}");

            if (firstBrace === -1 || lastBrace === -1) {
              logger.warn(
                LOG_KEYS.RUST_CLI_ERROR,
                "⚠️ No JSON output from analyze command",
                {
                  stdout: cleanStdout.substring(0, 200),
                },
              );
              resolve(null);
              return;
            }

            const jsonStr = cleanStdout.substring(firstBrace, lastBrace + 1);

            // 🔧 Debug: 记录解析前的JSON字符串
            logger.debug(LOG_KEYS.RUST_CLI_EXEC, "Parsing JSON", {
              firstChars: jsonStr.substring(0, 50),
              length: jsonStr.length,
            });

            const analysisData = JSON.parse(jsonStr);

            // 提取优化状态
            if (analysisData.optimization_status) {
              const opt = analysisData.optimization_status;
              logger.info(
                LOG_KEYS.RUST_CLI_EXEC,
                "✅ Optimization status analyzed",
                {
                  status: opt.status,
                  savings: opt.savings_percent,
                },
              );
              resolve({
                status: opt.status,
                canSkip: opt.can_skip,
                currentSize: opt.current_size,
                predictedSize: opt.predicted_size,
                savingsPercent: opt.savings_percent,
                confidence: opt.confidence,
                method: opt.method,
              });
            } else {
              logger.warn(
                LOG_KEYS.RUST_CLI_ERROR,
                "⚠️ No optimization_status in response",
                {
                  analysisData,
                },
              );
              resolve(null);
            }
          } catch (error) {
            logger.error(
              LOG_KEYS.RUST_CLI_ERROR,
              "❌ Failed to parse analysis result",
              {
                error: error.message,
                stdout,
              },
            );
            reject(error);
          }
        });

        child.on("error", (error) => {
          logger.error(
            LOG_KEYS.RUST_CLI_ERROR,
            "❌ Failed to spawn analyze process",
            {
              error: error.message,
            },
          );
          reject(error);
        });
      });
    } catch (error) {
      logger.error(LOG_KEYS.RUST_CLI_ERROR, "❌ Optimization analysis failed", {
        file: filePath,
        error: error.message,
        stack: error.stack,
      });
      // 🔥 不静默失败，抛出错误
      throw error;
    }
  };

  /**
   * 单文件视频转换
   * 🔮 滤镜模式：当 codec=null 时，保持原编码器只优化质量
   */
  const convertVideo = async (options) => {
    // 将单文件选项转换为批量格式
    const file = {
      path: options.inputPath,
      name: options.inputPath.split("/").pop(),
      ext: options.inputPath.split(".").pop(),
    };

    // 🔮 检测滤镜模式
    const isFilterMode = options.codec === null || options.isFilterMode;

    const result = await convertVideos([file], {
      codec: options.codec,
      container: options.container,
      crf: options.crf,
      preset: options.preset,
      useAI: options.useAI,
      optimizeMode: options.optimizeMode,
      enableGPU: options.enableGPU,
      enableVideoForAnimation: options.enableVideoForAnimation,
      enableSceneDetection: options.enableSceneDetection,
      enableVMAF: options.enableVMAF,
      enableTwoPass: options.enableTwoPass,
      isFilterMode: isFilterMode,
    });

    return result.results?.[0] || result;
  };

  return {
    isConverting,
    progress,
    currentFile,
    initRustCLI, // 🔥 导出初始化函数
    convert, // 🔥 单文件转换
    batchConvert, // 🔥 批量转换（别名）
    convertImages, // 原始方法
    convertVideos, // 视频批量转换
    convertVideo, // 🔮 单文件视频转换（滤镜模式支持）
    analyzeOptimizationStatus, // 🆕 导出新函数
  };
}
