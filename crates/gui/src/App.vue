<script setup lang="ts">
import { ref, computed, onMounted, onUnmounted, reactive, watch } from "vue";
import { useI18n } from "./composables/useI18n";
import {
  getCurrentWindow,
  invoke,
  listen,
  open,
  type UnlistenFn,
} from "./nativeHost";

const { t, setLocale, locale } = useI18n();

const locales = ["zh", "en", "ja"];
const currentLocaleIndex = ref(0);

const isDark = ref(true);
const useExternalTerminal = ref(true); // Default to external terminal
const processing = ref(false);
const folderPath = ref("");
const logs = ref<string[]>([]);
const uiNotice = ref("");
const prefersReducedMotion = ref(false);
const displayedLogs = computed(() => logs.value.slice(-160));
const terminalRef = ref<HTMLElement | null>(null);
const processorBinaryPath = ref<string>("");
const cliCommandPreview = computed(() => generateCliCommand());
const shouldAnimateAmbient = computed(() => !prefersReducedMotion.value);
const AUTO_SCROLL_THRESHOLD = 48;

// ─── CLI Processing Config ───
const processingMode = ref("both");

// Output Mode / Tools
const outputMode = ref("adjacent");

// Advanced Toggles
const mfbToggles = reactive({
  ultimateMode: true,
  verboseMode: true,
  resumeMode: false,
  shortestPath: false,
  cleanOutput: false,
});

// ─── Window Controls ───
const minimizeWindow = () => getCurrentWindow().minimize();
const toggleMaximize = async () => {
  const win = getCurrentWindow();
  if (await win.isMaximized()) {
    await win.unmaximize();
  } else {
    await win.maximize();
  }
};
const closeWindow = () => getCurrentWindow().close();
const startWindowDrag = (event: MouseEvent) => {
  if (event.button !== 0 || (event.target as Element).closest("button")) return;
  void getCurrentWindow().startDragging();
};

const toggleLanguage = () => {
  currentLocaleIndex.value = (currentLocaleIndex.value + 1) % locales.length;
  setLocale(locales[currentLocaleIndex.value]);
};
const toggleTheme = () => {
  isDark.value = !isDark.value;
  document.documentElement.dataset.theme = isDark.value ? "dark" : "light";
};

const selectFolder = async () => {
  if (processing.value) return;

  // 30-minute timeout for dialog (1800000 ms)
  dialogTimeout = setTimeout(() => {
    alert(t("dropzone.timeout"));
    console.warn("30-minute timeout reached");
  }, 1_800_000);

  try {
    const selected = await open({
      directory: true,
      multiple: false,
      title: "Select folder to process (30-minute timeout):",
    });

    clearTimeout(dialogTimeout);

    if (selected && typeof selected === "string") {
      folderPath.value = selected;
      // Don't auto-start in CLI mode - let user click Start button
    }
  } catch (error) {
    clearTimeout(dialogTimeout);
    console.error("Folder selection failed:", error);
  }
};

let scrollTimeout: ReturnType<typeof setTimeout> | null = null;
let dialogTimeout: ReturnType<typeof setTimeout> | null = null;
let noticeTimeout: ReturnType<typeof setTimeout> | null = null;
let motionMediaQuery: MediaQueryList | null = null;
let stopVisualWatch: (() => void) | null = null;
const nativeUnlisteners: UnlistenFn[] = [];

const setUiNotice = (message: string, durationMs = 3200) => {
  uiNotice.value = message;
  if (noticeTimeout) {
    clearTimeout(noticeTimeout);
  }
  noticeTimeout = setTimeout(() => {
    uiNotice.value = "";
    noticeTimeout = null;
  }, durationMs);
};

const shouldStickTerminalToBottom = () => {
  const terminal = terminalRef.value;
  if (!terminal) {
    return true;
  }
  return (
    terminal.scrollHeight - terminal.scrollTop - terminal.clientHeight <
    AUTO_SCROLL_THRESHOLD
  );
};

const appendLog = (entry: string) => {
  const shouldStick = shouldStickTerminalToBottom();
  logs.value.push(...entry.split("\n"));

  // Memory bound: avoid infinite array growth over very long sessions
  if (logs.value.length > 3000) {
    logs.value.splice(0, logs.value.length - 2400);
  }

  if (!shouldStick || scrollTimeout) {
    return;
  }

  scrollTimeout = setTimeout(() => {
    if (terminalRef.value) {
      terminalRef.value.scrollTop = terminalRef.value.scrollHeight;
    }
    scrollTimeout = null;
  }, 16);
};

type ResumeAction = "resume" | "fresh" | "cancel";

const currentProcessorRequest = (isResume: boolean, isFresh = false) => {
  const targetOutputMode =
    outputMode.value === "fast_img_avif" ? "fast_img" : outputMode.value;

  let strategy = null;
  if (outputMode.value === "fast_img_avif") {
    strategy = "avif";
  } else if (outputMode.value === "fast_img") {
    strategy = "jxl";
  }

  return {
    targetPath: folderPath.value,
    processingMode: processingMode.value,
    outputMode: targetOutputMode,
    strategy,
    ultimate: mfbToggles.ultimateMode,
    verbose: mfbToggles.verboseMode,
    resume: isResume,
    fresh: isFresh,
    shortestPath: mfbToggles.shortestPath,
  };
};

const requestResumeAction = (): ResumeAction => {
  if (
    globalThis.confirm(
      "检测到上次未完成的任务。\n\n选择“确定”继续上次任务；选择“取消”可改为重新开始。",
    )
  ) {
    return "resume";
  }
  return globalThis.confirm(
    "确定重新开始吗？旧状态会被丢弃；已有输出不会被覆盖，而是使用新的输出目录。",
  )
    ? "fresh"
    : "cancel";
};

const runProcessorWithResumeDecision = async () => {
  let shouldResume = mfbToggles.resumeMode;
  let shouldStartFresh = false;
  for (;;) {
    try {
      const result = await invoke(
        "process_media",
        currentProcessorRequest(shouldResume, shouldStartFresh),
      );
      appendLog(`[SUCCESS] ${String(result)}`);
      return;
    } catch (error) {
      const hasResumeDecision = logs.value.some((line) =>
        line.includes("MFB_RESUME_DECISION_REQUIRED"),
      );
      if (!hasResumeDecision || shouldResume || shouldStartFresh) throw error;

      const action = requestResumeAction();
      if (action === "resume") {
        shouldResume = true;
        mfbToggles.resumeMode = true;
        appendLog("[RESUME] User chose to continue the saved task.");
      } else if (action === "fresh") {
        shouldStartFresh = true;
        appendLog(
          "[FRESH] User chose a fresh task; saved state will not be reused.",
        );
      } else {
        appendLog("[INFO] Task cancelled; saved state was preserved.");
        return;
      }
    }
  }
};

const startCliProcessing = async () => {
  if (processing.value) return;
  if (!folderPath.value) {
    setUiNotice("Please select a folder first.");
    return;
  }

  processing.value = true;
  logs.value = [];
  appendLog(`[INFO] Starting processing: ${folderPath.value}`);

  try {
    await runProcessorWithResumeDecision();
  } catch (error) {
    appendLog(`[ERROR] ${String(error)}`);
  } finally {
    processing.value = false;
    appendLog(
      "[INFO] Processing completed. You can close this window or process another folder.",
    );
  }
};

const shellQuote = (value: string) =>
  "'" + value.split("'").join("'\"'\"'") + "'";

const generateCliCommand = () => {
  if (!folderPath.value) return "";
  if (!processorBinaryPath.value) return "Binary not found";

  // Get parent directory of target folder
  const targetPath = folderPath.value;
  const parentDir =
    targetPath.slice(0, Math.max(0, targetPath.lastIndexOf("/"))) || "/";

  let command = `cd ${shellQuote(parentDir)} && ${shellQuote(processorBinaryPath.value)}`;

  if (processingMode.value === "images_only") {
    command += " --images-only";
  } else if (processingMode.value === "videos_only") {
    command += " --videos-only";
  }

  switch (outputMode.value) {
    case "fast_img": {
      command += " --mode fast-img --strategy jxl";
      break;
    }
    case "fast_img_avif": {
      command += " --mode fast-img --strategy avif";
      break;
    }
    case "fast_vid": {
      command += " --mode fast-vid";
      break;
    }
    case "restore_jpeg": {
      command += " --mode restore-jpeg";
      break;
    }
    case "collect": {
      command += " --mode collect";
      break;
    }
    case "merge_xmp": {
      command += " --mode merge-xmp";
      break;
    }
    case "icloud_import": {
      command += " --mode icloud-import";
      break;
    }
    case "diagnostic": {
      command += " --mode diagnostic";
      break;
    }
    case "cache_clean": {
      command += " --mode cache-clean";
      break;
    }
    case "database_manager": {
      {
        command += " --mode database-manager";
        // No default
      }
      break;
    }
  }

  if (mfbToggles.ultimateMode) command += " --ultimate";
  if (mfbToggles.verboseMode) command += " --verbose";
  if (mfbToggles.resumeMode) command += " --resume";

  command += ` ${shellQuote(targetPath)}`;
  return command;
};

const copyCliCommand = async () => {
  const command = generateCliCommand();
  if (!command) {
    setUiNotice("Please select a folder first.");
    return;
  }

  try {
    await navigator.clipboard.writeText(command);
    setUiNotice("Command copied to clipboard.");
  } catch {
    // Fallback for older browsers
    const textarea = document.createElement("textarea");
    textarea.value = command;
    document.body.append(textarea);
    textarea.select();
    // eslint-disable-next-line @typescript-eslint/no-deprecated
    document.execCommand("copy");
    textarea.remove();
    setUiNotice("Command copied to clipboard.");
  }
};

const openInTerminal = async () => {
  const command = generateCliCommand();
  if (!command) {
    setUiNotice("Please select a folder first.");
    return;
  }

  try {
    // Copy to clipboard first
    await navigator.clipboard.writeText(command);

    // Try to open in external terminal
    const result = await invoke(
      "open_in_terminal",
      currentProcessorRequest(mfbToggles.resumeMode),
    );
    console.log(result);
    setUiNotice("Command copied and forwarded to your terminal.");
  } catch (error) {
    console.error("Failed to open terminal:", error);
    setUiNotice("Command copied to clipboard. Paste it in your terminal.");
  }
};

// Spotlight tracking for Liquid Glass Specular highlight (Optimized)
let rafId: number | null = null;
let currentMouseX = 0;
let currentMouseY = 0;

const resetPointerPosition = () => {
  document.documentElement.style.setProperty("--mouse-x", "0px");
  document.documentElement.style.setProperty("--mouse-y", "0px");
};

const syncReducedMotionPreference = (
  event: MediaQueryList | MediaQueryListEvent,
) => {
  prefersReducedMotion.value = event.matches;
  if (prefersReducedMotion.value) {
    resetPointerPosition();
  }
};

const onMouseMove = (e: MouseEvent) => {
  if (prefersReducedMotion.value) return;
  currentMouseX = e.clientX;
  currentMouseY = e.clientY;
  if (rafId === null) {
    rafId = requestAnimationFrame(() => {
      document.documentElement.style.setProperty(
        "--mouse-x",
        `${String(currentMouseX)}px`,
      );
      document.documentElement.style.setProperty(
        "--mouse-y",
        `${String(currentMouseY)}px`,
      );
      rafId = null;
    });
  }
};

const syncPointerTracking = () => {
  globalThis.removeEventListener("mousemove", onMouseMove);
  if (shouldAnimateAmbient.value) {
    globalThis.addEventListener("mousemove", onMouseMove, { passive: true });
    return;
  }
  if (rafId !== null) {
    cancelAnimationFrame(rafId);
    rafId = null;
  }
  resetPointerPosition();
};

onMounted(() => {
  // We cannot use requestAnimationFrame because a hidden window may pause rendering pipelines.
  // A small timeout allows Vue to paint without deadlocking the webview.
  setTimeout(() => {
    void getCurrentWindow().show();
  }, 100);

  // Web Fallbacks
  document.documentElement.dataset.theme = "dark";

  motionMediaQuery = globalThis.matchMedia("(prefers-reduced-motion: reduce)");
  syncReducedMotionPreference(motionMediaQuery);
  motionMediaQuery.addEventListener("change", syncReducedMotionPreference);
  stopVisualWatch = watch(shouldAnimateAmbient, syncPointerTracking, {
    immediate: true,
  });

  // Fetch processor binary path
  invoke<string>("get_processor_binary_path")
    .then((path) => {
      processorBinaryPath.value = path;
    })
    .catch((error: unknown) => {
      console.error("Failed to get processor binary path:", error);
    });

  invoke("check_version_alignment")
    .then((message: unknown) => {
      const msg =
        typeof message === "string"
          ? message
          : (() => {
              try {
                return JSON.stringify(message ?? "");
              } catch {
                return String(message);
              }
            })();
      console.log(msg);
      // Surface binary-missing or mismatched-version warnings in the UI
      // so the user doesn't have to open DevTools to see them.
      if (msg.includes("Warning") || msg.includes("Skipped")) {
        setUiNotice(`⚠️ ${msg}`, 8000);
      }
    })
    .catch((error: unknown) => {
      console.error("Version alignment check failed:", error);
    });

  // Real Native File Drop Listener
  listen("file-drop", (event: { payload: unknown }) => {
    const paths = event.payload as string[];
    if (paths.length > 0) {
      folderPath.value = paths[0];
      if (useExternalTerminal.value) {
        void openInTerminal();
      } else {
        void startCliProcessing();
      }
    }
  })
    .then((unlisten) => {
      nativeUnlisteners.push(unlisten);
    })
    .catch((error: unknown) => {
      console.error("Failed to register native file-drop listener:", error);
    });

  listen<string>("process-log", (event) => {
    appendLog(event.payload);
  })
    .then((unlisten) => {
      nativeUnlisteners.push(unlisten);
    })
    .catch((error: unknown) => {
      console.error("Failed to register native process-log listener:", error);
    });
});
onUnmounted(() => {
  globalThis.removeEventListener("mousemove", onMouseMove);
  motionMediaQuery?.removeEventListener("change", syncReducedMotionPreference);
  stopVisualWatch?.();
  for (const unlisten of nativeUnlisteners) {
    unlisten();
  }
  nativeUnlisteners.length = 0;
  if (dialogTimeout) clearTimeout(dialogTimeout);
  if (noticeTimeout) clearTimeout(noticeTimeout);
  if (scrollTimeout) clearTimeout(scrollTimeout);
  if (rafId !== null) cancelAnimationFrame(rafId);
});
</script>

<template>
  <div class="app" @contextmenu.prevent>
    <!-- Dynamic background hardware accelerated mapping -->
    <div class="ambient-bg" />

    <!-- ─── HEADER (Liquid Glass) ─── -->
    <header
      class="header liquid-glass"
      @mousedown="startWindowDrag"
      @dblclick="toggleMaximize"
    >
      <div class="header-left">
        <div class="logo-container">
          <div class="logo-icon">🚀</div>
        </div>
        <div class="title-group">
          <h1>
            {{ t("title") }}
          </h1>
          <p>
            {{ t("subtitle") }}
          </p>
        </div>
      </div>

      <div class="header-right">
        <div class="action-group">
          <button class="icon-btn" :title="t('lang')" @click="toggleLanguage">
            <span class="text-icon">{{ locale.toUpperCase() }}</span>
          </button>
          <button class="icon-btn" title="Theme" @click="toggleTheme">
            <span class="icon">{{ isDark ? "☀️" : "🌙" }}</span>
          </button>
        </div>

        <!-- Window Controls: macOS HIG order — close · minimize · maximize -->
        <div class="window-controls">
          <button class="window-btn close" @click="closeWindow">
            <svg width="10" height="10" viewBox="0 0 12 12">
              <path
                d="M2 2 L10 10 M10 2 L2 10"
                stroke="currentColor"
                stroke-width="1.5"
                stroke-linecap="round"
              />
            </svg>
          </button>
          <button class="window-btn minimize" @click="minimizeWindow">
            <svg width="10" height="10" viewBox="0 0 12 12">
              <rect x="2" y="5" width="8" height="2" fill="currentColor" />
            </svg>
          </button>
          <button class="window-btn maximize" @click="toggleMaximize">
            <svg width="10" height="10" viewBox="0 0 12 12">
              <rect
                x="2"
                y="2"
                width="8"
                height="8"
                fill="none"
                stroke="currentColor"
                stroke-width="1.5"
              />
            </svg>
          </button>
        </div>
      </div>
    </header>

    <div
      v-if="uiNotice"
      class="ui-notice liquid-glass"
      role="status"
      aria-live="polite"
    >
      {{ uiNotice }}
    </div>

    <!-- ─── CLI MODE CONTENT ─── -->
    <main class="cli-content">
      <div class="cli-panel">
        <h2>{{ t("cli.title") }}</h2>

        <!-- Terminal Mode Toggle -->
        <label class="cli-toggle-row">
          <span class="toggle-label">{{ t("cli.use_external_terminal") }}</span>
          <div class="switch" :class="{ on: useExternalTerminal }">
            <input v-model="useExternalTerminal" type="checkbox" />
            <div class="switch-knob" />
          </div>
        </label>

        <div class="cli-divider" />

        <div class="cli-field">
          <label>{{ t("cli.mode") }}</label>
          <select v-model="processingMode">
            <option value="both">
              {{ t("cli.process_everything") }}
            </option>
            <option value="images_only">
              {{ t("cli.images_only") }}
            </option>
            <option value="videos_only">
              {{ t("cli.videos_only") }}
            </option>
          </select>
        </div>
        <div class="cli-field">
          <button :disabled="processing" @click="selectFolder">
            {{ t("cli.select_folder") }}
          </button>
          <span class="cli-path">{{ folderPath || t("cli.no_folder") }}</span>
        </div>

        <!-- External Terminal Mode -->
        <div v-if="useExternalTerminal" class="external-terminal-section">
          <div class="cli-buttons">
            <button
              :disabled="!folderPath"
              class="cli-action-btn cli-open-btn"
              @click="openInTerminal"
            >
              🚀 {{ t("cli.open_in_terminal") }}
            </button>
            <button
              :disabled="!folderPath"
              class="cli-action-btn cli-copy-btn"
              @click="copyCliCommand"
            >
              📋 {{ t("cli.copy_command") }}
            </button>
          </div>
          <div v-if="folderPath" class="cli-command-preview">
            <pre>{{ cliCommandPreview }}</pre>
          </div>
          <p class="cli-hint">
            {{ t("cli.command_hint") }}
          </p>
        </div>

        <!-- Internal Terminal Mode -->
        <div v-else>
          <div class="cli-field">
            <button
              :disabled="!folderPath || processing"
              class="cli-start-btn"
              @click="startCliProcessing"
            >
              {{ processing ? t("cli.processing") : t("cli.start") }}
            </button>
          </div>
          <div ref="terminalRef" class="cli-terminal">
            <div
              v-for="(log, idx) in displayedLogs"
              :key="idx"
              class="cli-log-line"
            >
              {{ log }}
            </div>
          </div>
        </div>
      </div>
    </main>

  </div>
</template>

<style>
/* ─── CLI Mode Styles ─── */
.cli-content {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
}
.cli-panel {
  background: rgba(0, 0, 0, 0.85);
  border-radius: 8px;
  padding: 24px;
  width: 700px;
  color: #00ff00;
  font-family: monospace;
  box-shadow: 0 10px 30px rgba(0, 0, 0, 0.5);
  border: 1px solid #333;
  font-size: 15px;
}
.cli-panel h2 {
  margin-bottom: 20px;
  font-size: 1.5rem;
  text-transform: uppercase;
  border-bottom: 1px solid #00ff00;
  padding-bottom: 8px;
}

/* Toggle Row - Clean Layout */
.cli-toggle-row {
  display: flex;
  justify-content: space-between;
  align-items: center;
  background: rgba(255, 255, 255, 0.05);
  border-radius: 6px;
  padding: 12px 16px;
  margin-bottom: 12px;
  cursor: pointer;
  transition: background-color 0.2s ease;
}
.cli-toggle-row:hover {
  background: rgba(255, 255, 255, 0.08);
}
.toggle-label {
  font-size: 14px;
  font-weight: 600;
  color: #00ff00;
}

.cli-divider {
  height: 1px;
  background: rgba(0, 255, 0, 0.3);
  margin: 16px 0;
}

.cli-field {
  margin-bottom: 16px;
  display: flex;
  align-items: center;
  gap: 12px;
}
.cli-field label {
  min-width: 80px;
  font-size: 14px;
}
.cli-field select,
.cli-field button {
  background: #222;
  color: #00ff00;
  border: 1px solid #00ff00;
  padding: 8px 14px;
  font-family: monospace;
  cursor: pointer;
  outline: none;
  font-size: 14px;
  border-radius: 4px;
  transition:
    background-color 0.2s ease,
    color 0.2s ease,
    border-color 0.2s ease;
}
.cli-field select:focus,
.cli-field button:hover {
  background: #00ff00;
  color: #000;
}
.cli-field button:disabled {
  border-color: #555;
  color: #555;
  background: #222;
  cursor: not-allowed;
}
.cli-path {
  font-size: 13px;
  color: #aaa;
  word-break: break-all;
  flex: 1;
}
.cli-start-btn {
  font-weight: bold;
  padding: 12px 24px !important;
  font-size: 15px;
  width: 100%;
}

.cli-terminal {
  margin-top: 20px;
  background: #000;
  height: 250px;
  overflow-y: auto;
  padding: 10px;
  border: 1px solid #333;
  font-size: 13px;
  color: #ccc;
  border-radius: 4px;
  contain: content;
  content-visibility: auto;
  overscroll-behavior: contain;
}
.cli-log-line {
  border-bottom: 1px solid rgba(255, 255, 255, 0.05);
  padding: 2px 0;
  white-space: pre-wrap;
  word-break: break-all;
}

/* External Terminal Section */
.external-terminal-section {
  margin-top: 16px;
}
.cli-buttons {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 12px;
  margin-bottom: 16px;
}
.cli-action-btn {
  font-weight: bold;
  padding: 14px 20px;
  font-size: 15px;
  background: #222;
  color: #00ff00;
  border: 1px solid #00ff00;
  border-radius: 6px;
  cursor: pointer;
  outline: none;
  transition:
    background-color 0.2s ease,
    color 0.2s ease,
    border-color 0.2s ease,
    transform 0.2s ease;
  font-family: monospace;
}
.cli-action-btn:hover:not(:disabled) {
  background: #00ff00;
  color: #000;
  transform: translateY(-2px);
}
.cli-action-btn:active:not(:disabled) {
  transform: translateY(0);
}
.cli-action-btn:disabled {
  border-color: #555;
  color: #555;
  background: #222;
  cursor: not-allowed;
}
.cli-open-btn {
  background: #004400;
  border-color: #00ff00;
}
.cli-open-btn:hover:not(:disabled) {
  background: #00aa00;
  color: #000;
}

.cli-command-preview {
  background: #000;
  border: 1px solid #333;
  border-radius: 6px;
  padding: 12px;
  margin-bottom: 12px;
  overflow-x: auto;
}
.cli-command-preview pre {
  margin: 0;
  color: #0f0;
  font-size: 13px;
  white-space: pre-wrap;
  word-break: break-all;
}
.cli-hint {
  font-size: 13px;
  color: #888;
  text-align: center;
  margin: 0;
  font-style: italic;
}

/* ─── Global Variables & Reset ─── */
.terminal-logs {
  width: 90%;
  height: 180px;
  background: rgba(0, 0, 0, 0.4);
  border-radius: 8px;
  margin-top: 16px;
  padding: 12px;
  overflow-y: auto;
  text-align: left;
  font-family: "Menlo", "Monaco", "Courier New", monospace;
  font-size: 0.75rem;
  color: #a8a8b3;
  line-height: 1.4;
  border: 1px solid rgba(255, 255, 255, 0.1);
  box-shadow: inset 0 2px 10px rgba(0, 0, 0, 0.3);
  contain: content;
  content-visibility: auto;
  overscroll-behavior: contain;
}
.log-line {
  white-space: pre-wrap;
  word-break: break-all;
  border-bottom: 1px solid rgba(255, 255, 255, 0.02);
  padding-bottom: 2px;
  margin-bottom: 2px;
}
.log-line:last-child {
  border-bottom: none;
}
:root {
  --glass-bg: rgba(255, 255, 255, 0.6);
  --glass-border: rgba(255, 255, 255, 0.4);
  --glass-highlight: rgba(255, 255, 255, 0.8);
  --glass-shadow: rgba(0, 0, 0, 0.05);
  --text-main: #1d1d1f;
  --text-muted: #86868b;
  --accent: #0066cc;
  --accent-hover: #0077ed;
  --bg-gradient: linear-gradient(135deg, #e0e5ec 0%, #f4f5f7 100%);
  --danger: #ff3b30;
  --warning: #ff9500;
  --success: #34c759;
  --mouse-x: 0px;
  --mouse-y: 0px;
}

[data-theme="dark"] {
  --glass-bg: rgba(30, 30, 32, 0.45);
  --glass-border: rgba(255, 255, 255, 0.08);
  --glass-highlight: rgba(255, 255, 255, 0.12);
  --glass-shadow: rgba(0, 0, 0, 0.3);
  --text-main: #f5f5f7;
  --text-muted: #86868b;
  --accent: #0a84ff;
  --accent-hover: #409cff;
  --bg-gradient: linear-gradient(135deg, #0f0f11 0%, #1c1c1e 100%);
}

* {
  box-sizing: border-box;
  margin: 0;
  padding: 0;
}
body {
  font-family:
    -apple-system, BlinkMacSystemFont, "SF Pro Text", "Segoe UI", Roboto,
    Helvetica, Arial, sans-serif;
  -webkit-font-smoothing: antialiased;
  background: var(--bg-gradient);
  color: var(--text-main);
  overflow: hidden;
  user-select: none;
  -webkit-user-select: none;
  background-color: transparent !important;
  transition:
    background 0.5s cubic-bezier(0.4, 0, 0.2, 1),
    color 0.5s cubic-bezier(0.4, 0, 0.2, 1);
}

/* ─── Dynamic Ambient Background ─── */
.ambient-bg {
  position: absolute;
  inset: -50%;
  z-index: -1;
  background-image:
    radial-gradient(
      circle at 50% 20%,
      rgba(10, 132, 255, 0.15) 0%,
      transparent 40%
    ),
    radial-gradient(
      circle at 80% 80%,
      rgba(191, 90, 242, 0.15) 0%,
      transparent 40%
    ),
    radial-gradient(
      circle at 20% 80%,
      rgba(48, 209, 88, 0.1) 0%,
      transparent 40%
    );
  filter: blur(60px);
  opacity: 0.8;
  will-change: transform;
  transform: translate3d(
    calc(var(--mouse-x) * -0.05),
    calc(var(--mouse-y) * -0.05),
    0
  );
  transition: opacity 0.5s ease;
}

.app {
  width: 100vw;
  height: 100vh;
  display: flex;
  flex-direction: column;
  padding: 12px;
  gap: 12px;
}

.ui-notice {
  align-self: center;
  padding: 10px 14px;
  font-size: 0.85rem;
  font-weight: 600;
  color: var(--text-main);
}

/* ─── LIQUID GLASS MATERIAL ─── */
.liquid-glass {
  background: var(--glass-bg);
  backdrop-filter: blur(28px) saturate(180%) contrast(110%);
  -webkit-backdrop-filter: blur(28px) saturate(180%) contrast(110%);
  border: 1px solid var(--glass-border);
  box-shadow:
    inset 0 1px 1px var(--glass-highlight),
    inset 0 -1px 1px rgba(0, 0, 0, 0.05),
    0 8px 32px var(--glass-shadow);
  border-radius: 16px;
  position: relative;
  overflow: hidden;
  transition:
    background 0.4s cubic-bezier(0.4, 0, 0.2, 1),
    border-color 0.4s cubic-bezier(0.4, 0, 0.2, 1),
    box-shadow 0.4s cubic-bezier(0.4, 0, 0.2, 1);
}

/* ─── Header ─── */
.header {
  height: 56px;
  flex-shrink: 0;
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 0 16px;
}

.header-left {
  display: flex;
  align-items: center;
  gap: 12px;
  height: 100%;
}
.logo-icon {
  font-size: 1.5rem;
  filter: drop-shadow(0 2px 4px rgba(0, 0, 0, 0.2));
}
.title-group h1 {
  font-size: 0.95rem;
  font-weight: 600;
  margin: 0;
  letter-spacing: -0.01em;
}
.title-group p {
  font-size: 0.7rem;
  color: var(--text-muted);
  margin: 0;
  font-weight: 500;
  pointer-events: none;
}

.header-right {
  display: flex;
  align-items: center;
  gap: 16px;
}
.action-group {
  display: flex;
  align-items: center;
  gap: 4px;
}
.icon-btn {
  background: transparent;
  border: none;
  color: var(--text-main);
  width: 32px;
  height: 32px;
  border-radius: 8px;
  cursor: pointer;
  display: flex;
  align-items: center;
  justify-content: center;
  transition:
    background-color 0.2s ease,
    color 0.2s ease,
    transform 0.15s ease;
  font-size: 1rem;
  font-weight: 600;
}
.icon-btn:hover {
  background: rgba(128, 128, 128, 0.15);
  transform: scale(1.05);
}
.icon-btn:active {
  transform: scale(0.95);
}
.text-icon {
  font-size: 0.75rem;
  letter-spacing: 0.05em;
}

/* Window Controls (Traffic Lights style) */
.window-controls {
  display: flex;
  gap: 8px;
  margin-left: 8px;
}
.window-btn {
  width: 14px;
  height: 14px;
  border-radius: 50%;
  border: none;
  display: flex;
  align-items: center;
  justify-content: center;
  cursor: pointer;
  color: transparent;
  transition: all 0.2s;
}
.window-btn svg {
  width: 8px;
  height: 8px;
}
.window-controls:hover .window-btn {
  color: rgba(0, 0, 0, 0.5);
}
.window-btn.close {
  background: #ff5f56;
  border: 1px solid #e0443e;
}
.window-btn.minimize {
  background: #ffbd2e;
  border: 1px solid #dea123;
}
.window-btn.maximize {
  background: #27c93f;
  border: 1px solid #1aab29;
}

</style>
