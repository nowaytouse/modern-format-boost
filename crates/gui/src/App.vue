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
const isCliMode = ref(false);
const useExternalTerminal = ref(true); // Default to external terminal
const processing = ref(false);
const progress = ref(0);
const folderPath = ref("");
const logs = ref<string[]>([]);
const uiNotice = ref("");
const prefersReducedMotion = ref(false);
const displayedLogs = computed(() => logs.value.slice(-160));
const terminalRef = ref<HTMLElement | null>(null);
const processorBinaryPath = ref<string>("");
const cliCommandPreview = computed(() => generateCliCommand());
const shouldAnimateAmbient = computed(
  () => !prefersReducedMotion.value && !isCliMode.value,
);
const AUTO_SCROLL_THRESHOLD = 48;

// ─── Exact Drag & Drop Script Configs ───
// Processing Mode: Both, Images Only, Videos Only
const processingModeOpts = computed(() => [
  { id: "both", label: t("options.all"), icon: "📦" },
  { id: "images_only", label: t("options.images"), icon: "🖼️" },
  { id: "videos_only", label: t("options.videos"), icon: "🎬" },
]);
const processingMode = ref("both");

// Output Mode / Tools
const outputModeOpts = computed(() => [
  { id: "adjacent", label: t("format.avif_hevc") + " (Adj)", icon: "📂" },
  { id: "fast_img", label: t("tier.fast") + " (IMG JXL)", icon: "⚡" },
  {
    id: "fast_img_avif",
    label: t("tier.fast") + " (IMG AVIF Meme)",
    icon: "🤡",
  },
  { id: "fast_vid", label: t("tier.fast") + " (VID)", icon: "🚀" },
  { id: "restore_jpeg", label: "Restore to JPEG", icon: "⏪" },
  { id: "collect", label: "Collect Optimized", icon: "📥" },
  { id: "merge_xmp", label: "Merge XMP", icon: "📝" },
  { id: "icloud_import", label: "iCloud Import", icon: "☁️" },
  { id: "diagnostic", label: "Diagnostic Analysis", icon: "🩺" },
  { id: "cache_clean", label: "Cache Cleaner", icon: "🧹" },
  { id: "database_manager", label: "Database Manager", icon: "🗄️" },
]);
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

// ─── Drag & Drop ───
const isDragging = ref(false);
const onDragEnter = (e: DragEvent) => {
  e.preventDefault();
  isDragging.value = true;
};
const onDragLeave = (e: DragEvent) => {
  e.preventDefault();
  isDragging.value = false;
};
const onDrop = (e: DragEvent) => {
  e.preventDefault();
  isDragging.value = false;
  if (e.dataTransfer && e.dataTransfer.files.length > 0) {
    folderPath.value =
      e.dataTransfer.files[0].name || t("processing.dragged_folder");
    startMockProcessing();
  }
};

const selectFolder = async () => {
  if (processing.value) return;

  if (!isCliMode.value) {
    folderPath.value = t("processing.selected_folder");
    startMockProcessing();
    return;
  }

  // 30-minute timeout for dialog (1800000 ms)
  dialogTimeout = setTimeout(() => {
    alert(
      t("dropzone.timeout") ||
        "Folder selection cancelled (no action for 30 minutes).",
    );
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

let progressInterval: ReturnType<typeof setInterval> | null = null;
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
  logs.value.push(entry);

  // Memory bound: avoid infinite array growth over very long sessions
  if (logs.value.length > 3000) {
    logs.value.splice(0, 600);
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

const startMockProcessing = () => {
  if (progressInterval) clearInterval(progressInterval);
  processing.value = true;
  progress.value = 0;
  progressInterval = setInterval(() => {
    progress.value += Math.random() * 3;
    if (progress.value >= 100) {
      progress.value = 100;
      if (progressInterval !== null) {
        clearInterval(progressInterval);
      }
      progressInterval = null;
      setTimeout(() => {
        processing.value = false;
        folderPath.value = "";
      }, 1500);
    }
  }, 50);
};

type ResumeAction = "resume" | "fresh" | "cancel";

const currentProcessorRequest = (resume: boolean, fresh = false) => {
  const targetOutputMode =
    outputMode.value === "fast_img_avif" ? "fast_img" : outputMode.value;
  const strategy =
    outputMode.value === "fast_img_avif"
      ? "avif"
      : outputMode.value === "fast_img"
        ? "jxl"
        : null;
  return {
    targetPath: folderPath.value,
    processingMode: processingMode.value,
    outputMode: targetOutputMode,
    strategy,
    ultimate: mfbToggles.ultimateMode,
    verbose: mfbToggles.verboseMode,
    resume,
    fresh,
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
        appendLog("[FRESH] User chose a fresh task; saved state will not be reused.");
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

const toggleCliMode = () => {
  isCliMode.value = !isCliMode.value;
};

const generateCliCommand = () => {
  if (!folderPath.value) return "";
  if (!processorBinaryPath.value) return "Binary not found";

  // Get parent directory of target folder
  const targetPath = folderPath.value;
  const parentDir =
    targetPath.slice(0, Math.max(0, targetPath.lastIndexOf("/"))) || "/";

  const shellQuote = (value: string) => `'${value.split("'").join(`'"'"'`)}'`;
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

const preventDrag = (e: Event) => {
  e.preventDefault();
};
onMounted(() => {
  // We cannot use requestAnimationFrame because a hidden window may pause rendering pipelines.
  // A small timeout allows Vue to paint without deadlocking the webview.
  setTimeout(() => {
    void getCurrentWindow().show();
  }, 100);

  // Web Fallbacks
  globalThis.addEventListener("dragover", preventDrag);
  globalThis.addEventListener("drop", preventDrag);
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

  // Version Alignment Check Mechanism
  invoke("check_version_alignment")
    .then((message: unknown) => {
      console.log(message);
    })
    .catch((error: unknown) => {
      console.error("Version alignment check failed:", error);
    });

  // Real Native File Drop Listener
  listen("file-drop", (event: { payload: unknown }) => {
    const paths = event.payload as string[];
    if (paths.length > 0) {
      if (isCliMode.value) {
        folderPath.value = paths[0];
        if (useExternalTerminal.value) {
          void openInTerminal();
        } else {
          void startCliProcessing();
        }
      } else {
        folderPath.value = t("processing.dragged_folder");
        startMockProcessing();
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
  globalThis.removeEventListener("dragover", preventDrag);
  globalThis.removeEventListener("drop", preventDrag);
  globalThis.removeEventListener("mousemove", onMouseMove);
  motionMediaQuery?.removeEventListener("change", syncReducedMotionPreference);
  stopVisualWatch?.();
  for (const unlisten of nativeUnlisteners) {
    unlisten();
  }
  nativeUnlisteners.length = 0;
  if (dialogTimeout) clearTimeout(dialogTimeout);
  if (noticeTimeout) clearTimeout(noticeTimeout);
  if (progressInterval) clearInterval(progressInterval);
  if (scrollTimeout) clearTimeout(scrollTimeout);
  if (rafId !== null) cancelAnimationFrame(rafId);
});
</script>

<template>
  <div class="app" @contextmenu.prevent>
    <!-- Dynamic background hardware accelerated mapping -->
    <div
      class="ambient-bg"
      :class="{ 'ambient-bg--static': !shouldAnimateAmbient }"
    />

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
          <button
            class="icon-btn cli-toggle-btn"
            title="Toggle CLI Mode"
            @click="toggleCliMode"
          >
            <span class="text-icon">CLI</span>
          </button>
          <button class="icon-btn" :title="t('lang')" @click="toggleLanguage">
            <span class="text-icon">{{ locale.toUpperCase() }}</span>
          </button>
          <button class="icon-btn" title="Theme" @click="toggleTheme">
            <span class="icon">{{ isDark ? "☀️" : "🌙" }}</span>
          </button>
        </div>

        <!-- Window Controls -->
        <div class="window-controls">
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
    <main v-if="isCliMode" class="cli-content">
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

    <!-- ─── MAIN CONTENT ─── -->
    <main v-else class="main-content">
      <!-- LEFT PANEL: Controls -->
      <aside class="panel controls-panel liquid-glass">
        <div class="panel-header">
          <h3 class="panel-title">
            <span class="icon">⚙️</span> {{ t("advanced") }}
          </h3>
        </div>
        <div class="panel-scroll">
          <!-- Processing Mode -->
          <div class="control-group">
            <label class="group-label">{{ t("optionsLabel") }}</label>
            <div class="segmented-control">
              <label
                v-for="mode in processingModeOpts"
                :key="mode.id"
                class="segment"
                :class="{ active: processingMode === mode.id }"
              >
                <input v-model="processingMode" type="radio" :value="mode.id" />
                <span class="seg-icon">{{ mode.icon }}</span>
                <span class="seg-label">{{ mode.label }}</span>
              </label>
            </div>
          </div>

          <!-- Output Mode -->
          <div class="control-group">
            <label class="group-label">{{ t("formatLabel") }}</label>
            <div class="select-wrapper">
              <select v-model="outputMode" class="liquid-select">
                <option
                  v-for="opt in outputModeOpts"
                  :key="opt.id"
                  :value="opt.id"
                >
                  {{ opt.icon }} {{ opt.label }}
                </option>
              </select>
            </div>
          </div>

          <!-- Boolean Toggles -->
          <div class="control-group toggles-group">
            <label class="toggle-row">
              <span class="toggle-text"
                >{{ t("toggles.ultimateMode") }}
                <span class="badge warning">SLOW</span></span
              >
              <div class="switch" :class="{ on: mfbToggles.ultimateMode }">
                <input v-model="mfbToggles.ultimateMode" type="checkbox" />
                <div class="switch-knob" />
              </div>
            </label>
            <label class="toggle-row">
              <span class="toggle-text">{{ t("toggles.verboseMode") }}</span>
              <div class="switch" :class="{ on: mfbToggles.verboseMode }">
                <input v-model="mfbToggles.verboseMode" type="checkbox" />
                <div class="switch-knob" />
              </div>
            </label>
            <label class="toggle-row">
              <span class="toggle-text">{{ t("toggles.resumeMode") }}</span>
              <div class="switch" :class="{ on: mfbToggles.resumeMode }">
                <input v-model="mfbToggles.resumeMode" type="checkbox" />
                <div class="switch-knob" />
              </div>
            </label>
            <label class="toggle-row">
              <span class="toggle-text"
                >{{ t("toggles.purgeCache") }}
                <span class="badge info">CACHE</span></span
              >
              <div class="switch" :class="{ on: mfbToggles.shortestPath }">
                <input v-model="mfbToggles.shortestPath" type="checkbox" />
                <div class="switch-knob" />
              </div>
            </label>
            <label class="toggle-row">
              <span class="toggle-text">{{ t("toggles.skipRefresh") }}</span>
              <div class="switch" :class="{ on: mfbToggles.cleanOutput }">
                <input v-model="mfbToggles.cleanOutput" type="checkbox" />
                <div class="switch-knob" />
              </div>
            </label>
          </div>

          <div class="panel-footer">
            <button
              class="liquid-btn primary"
              :disabled="processing"
              @click="selectFolder"
            >
              <span v-if="!processing">{{ t("dropzone.browse") }}</span>
              <span v-else>Engine Running...</span>
            </button>
          </div>
        </div>
      </aside>

      <!-- RIGHT PANEL: Drop Zone -->
      <section
        class="panel files-panel liquid-glass"
        :class="{ 'drag-active': isDragging }"
        @dragenter="onDragEnter"
        @dragover="onDragEnter"
        @dragleave="onDragLeave"
        @drop="onDrop"
      >
        <div class="drag-glow" :class="{ visible: isDragging }" />

        <Transition name="fade" mode="out-in">
          <!-- Idle State -->
          <div v-if="!processing" key="idle" class="drop-idle">
            <div class="drop-icon-container" :class="{ float: isDragging }">
              <span class="huge-icon">📁</span>
              <div class="ripple-ring" />
            </div>
            <h2 class="drop-title">
              {{ isDragging ? t("dropzone.release") : t("dropzone.drag") }}
            </h2>
            <p class="drop-subtitle">
              Folders drop here automatically route through pipeline.
            </p>
          </div>

          <!-- Processing State -->
          <div v-else key="proc" class="drop-processing">
            <div class="progress-ring">
              <svg width="140" height="140" viewBox="0 0 140 140">
                <circle cx="70" cy="70" r="64" class="ring-bg" />
                <circle
                  cx="70"
                  cy="70"
                  r="64"
                  class="ring-fill"
                  :stroke-dasharray="`${2 * Math.PI * 64}`"
                  :stroke-dashoffset="`${2 * Math.PI * 64 * (1 - progress / 100)}`"
                />
              </svg>
              <span class="progress-text">{{ Math.round(progress) }}%</span>
            </div>
            <div class="proc-details">
              <h3 class="proc-target">
                {{ folderPath }}
              </h3>
              <p class="proc-status">
                {{ t("pipeline.gate") }}
              </p>
            </div>
            <div ref="terminalRef" class="terminal-logs">
              <div
                v-for="(log, idx) in displayedLogs"
                :key="idx"
                class="log-line"
              >
                {{ log }}
              </div>
            </div>
          </div>
        </Transition>
      </section>
    </main>
  </div>
</template>

<style>
/* ─── CLI Mode Styles ─── */
.cli-toggle-btn {
  border: 1px solid var(--glass-border);
  background: var(--glass-highlight);
  font-weight: 900;
}
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
.ambient-bg--static {
  transform: translate3d(0, 0, 0);
  will-change: auto;
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

/* ─── Main Content ─── */
.main-content {
  flex: 1;
  display: flex;
  gap: 12px;
  min-height: 0;
}
.panel {
  display: flex;
  flex-direction: column;
  contain: layout paint;
}

/* Left Panel */
.controls-panel {
  width: 340px;
  flex-shrink: 0;
  padding: 20px;
  gap: 20px;
}
.panel-header {
  margin-bottom: 4px;
}
.panel-title {
  font-size: 1.1rem;
  font-weight: 600;
  display: flex;
  align-items: center;
  gap: 8px;
  margin: 0;
}
.panel-scroll {
  flex: 1;
  overflow-y: auto;
  overflow-x: hidden;
  display: flex;
  flex-direction: column;
  gap: 24px;
  padding-right: 4px;
  margin-right: -4px;
}
.panel-scroll::-webkit-scrollbar {
  width: 4px;
}
.panel-scroll::-webkit-scrollbar-thumb {
  background: rgba(128, 128, 128, 0.3);
  border-radius: 4px;
}

/* Forms */
.control-group {
  display: flex;
  flex-direction: column;
  gap: 8px;
}
.group-label {
  font-size: 0.75rem;
  font-weight: 600;
  color: var(--text-muted);
  text-transform: uppercase;
  letter-spacing: 0.05em;
}

.segmented-control {
  display: flex;
  background: rgba(0, 0, 0, 0.15);
  border-radius: 10px;
  padding: 4px;
  gap: 4px;
}
[data-theme="light"] .segmented-control {
  background: rgba(0, 0, 0, 0.05);
}
.segment {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 4px;
  padding: 8px 4px;
  border-radius: 8px;
  cursor: pointer;
  transition:
    background-color 0.2s ease,
    color 0.2s ease,
    box-shadow 0.2s ease,
    transform 0.15s ease;
  color: var(--text-muted);
  position: relative;
}
.segment:hover {
  transform: translateY(-1px);
}
.segment:active {
  transform: translateY(0);
}
.segment input {
  display: none;
}
.segment.active {
  background: var(--glass-bg);
  color: var(--text-main);
  box-shadow:
    0 2px 8px rgba(0, 0, 0, 0.1),
    inset 0 1px 1px var(--glass-highlight);
  transform: translateY(-1px);
}
.seg-icon {
  font-size: 1.2rem;
}
.seg-label {
  font-size: 0.7rem;
  font-weight: 500;
  text-align: center;
}

.select-wrapper {
  position: relative;
}
.liquid-select {
  width: 100%;
  appearance: none;
  background: rgba(0, 0, 0, 0.15);
  border: 1px solid var(--glass-border);
  color: var(--text-main);
  padding: 10px 14px;
  border-radius: 10px;
  font-size: 0.9rem;
  font-weight: 500;
  outline: none;
  cursor: pointer;
  box-shadow: inset 0 1px 1px var(--glass-highlight);
  transition:
    background-color 0.2s ease,
    border-color 0.2s ease,
    box-shadow 0.2s ease,
    transform 0.15s ease;
}
[data-theme="light"] .liquid-select {
  background: rgba(0, 0, 0, 0.05);
}
.liquid-select:hover {
  border-color: var(--accent);
  box-shadow:
    inset 0 1px 1px var(--glass-highlight),
    0 0 0 3px rgba(10, 132, 255, 0.1);
}
.liquid-select:focus {
  border-color: var(--accent);
  box-shadow:
    inset 0 1px 1px var(--glass-highlight),
    0 0 0 3px rgba(10, 132, 255, 0.2);
}

/* Switch Toggles */
.toggles-group {
  background: rgba(0, 0, 0, 0.1);
  border-radius: 12px;
  padding: 4px;
}
[data-theme="light"] .toggles-group {
  background: rgba(0, 0, 0, 0.03);
}
.toggle-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 10px 12px;
  border-radius: 8px;
  cursor: pointer;
  transition:
    background-color 0.2s ease,
    transform 0.15s ease;
}
.toggle-row:hover {
  background: rgba(128, 128, 128, 0.1);
  transform: translateX(2px);
}
.toggle-text {
  font-size: 0.85rem;
  font-weight: 500;
  display: flex;
  align-items: center;
  gap: 8px;
}
.badge {
  font-size: 0.6rem;
  padding: 2px 6px;
  border-radius: 4px;
  font-weight: 700;
}
.badge.warning {
  background: rgba(255, 149, 0, 0.2);
  color: var(--warning);
}
.badge.info {
  background: rgba(10, 132, 255, 0.2);
  color: var(--accent);
}

.switch {
  width: 36px;
  height: 20px;
  background: rgba(128, 128, 128, 0.3);
  border-radius: 10px;
  position: relative;
  transition:
    background-color 0.3s ease,
    box-shadow 0.3s ease;
  box-shadow: inset 0 1px 3px rgba(0, 0, 0, 0.2);
}
.switch input {
  display: none;
}
.switch-knob {
  position: absolute;
  top: 2px;
  left: 2px;
  width: 16px;
  height: 16px;
  background: #fff;
  border-radius: 50%;
  transition:
    transform 0.3s cubic-bezier(0.34, 1.56, 0.64, 1),
    box-shadow 0.3s ease;
  box-shadow: 0 2px 4px rgba(0, 0, 0, 0.2);
}
.switch:hover {
  box-shadow:
    inset 0 1px 3px rgba(0, 0, 0, 0.2),
    0 0 0 3px rgba(52, 199, 89, 0.1);
}
.switch.on {
  background: var(--success);
}
.switch.on .switch-knob {
  transform: translateX(16px);
  box-shadow: 0 2px 6px rgba(0, 0, 0, 0.3);
}

.panel-footer {
  margin-top: auto;
  padding-top: 16px;
}
.liquid-btn {
  width: 100%;
  padding: 14px;
  border-radius: 12px;
  border: none;
  font-size: 0.95rem;
  font-weight: 600;
  cursor: pointer;
  background: var(--glass-bg);
  color: var(--text-main);
  box-shadow:
    inset 0 1px 1px var(--glass-highlight),
    0 4px 12px rgba(0, 0, 0, 0.1);
  border: 1px solid var(--glass-border);
  transition:
    background-color 0.3s cubic-bezier(0.4, 0, 0.2, 1),
    border-color 0.3s cubic-bezier(0.4, 0, 0.2, 1),
    color 0.3s cubic-bezier(0.4, 0, 0.2, 1),
    transform 0.3s cubic-bezier(0.4, 0, 0.2, 1),
    box-shadow 0.3s cubic-bezier(0.4, 0, 0.2, 1);
}
.liquid-btn.primary {
  background: var(--accent);
  color: #fff;
  border-color: var(--accent-hover);
  box-shadow:
    inset 0 1px 1px rgba(255, 255, 255, 0.3),
    0 4px 15px rgba(10, 132, 255, 0.3);
}
.liquid-btn.primary:hover:not(:disabled) {
  background: var(--accent-hover);
  transform: translateY(-1px);
}
.liquid-btn.primary:active:not(:disabled) {
  transform: translateY(1px) scale(0.97);
  box-shadow:
    inset 0 2px 4px rgba(0, 0, 0, 0.2),
    0 2px 8px rgba(10, 132, 255, 0.2);
}
.liquid-btn:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}

/* Right Panel */
.files-panel {
  flex: 1;
  display: flex;
  align-items: center;
  justify-content: center;
  position: relative;
  transition:
    border-color 0.3s ease,
    transform 0.3s ease,
    box-shadow 0.3s ease;
}
.files-panel.drag-active {
  border-color: var(--accent);
  transform: scale(0.98);
}
.drag-glow {
  position: absolute;
  inset: 0;
  background: rgba(10, 132, 255, 0.1);
  pointer-events: none;
  opacity: 0;
  transition: opacity 0.3s;
}
.drag-glow.visible {
  opacity: 1;
}

.drop-idle,
.drop-processing {
  display: flex;
  flex-direction: column;
  align-items: center;
  text-align: center;
  z-index: 2;
}

.drop-icon-container {
  position: relative;
  margin-bottom: 20px;
  transition: transform 0.4s cubic-bezier(0.34, 1.56, 0.64, 1);
}
.drop-icon-container.float {
  transform: translateY(-20px) scale(1.1);
}
.huge-icon {
  font-size: 5rem;
  filter: drop-shadow(0 10px 20px rgba(0, 0, 0, 0.2));
  position: relative;
  z-index: 2;
  transition: transform 0.3s ease;
}
.drop-icon-container:hover .huge-icon {
  transform: scale(1.05);
}
.ripple-ring {
  position: absolute;
  inset: -20px;
  border-radius: 50%;
  border: 2px dashed var(--glass-border);
  animation: spin-slow 10s linear infinite;
  transition: border-color 0.3s ease;
}
.drop-icon-container:hover .ripple-ring {
  border-color: var(--accent);
}
@keyframes spin-slow {
  100% {
    transform: rotate(360deg);
  }
}

.drop-title {
  font-size: 1.8rem;
  font-weight: 700;
  margin-bottom: 8px;
  letter-spacing: -0.02em;
}
.drop-subtitle {
  font-size: 0.95rem;
  color: var(--text-muted);
  max-width: 300px;
  line-height: 1.4;
}

/* Processing UI */
.progress-ring {
  position: relative;
  display: flex;
  align-items: center;
  justify-content: center;
  margin-bottom: 24px;
}
.ring-bg {
  fill: none;
  stroke: rgba(128, 128, 128, 0.1);
  stroke-width: 6;
}
.ring-fill {
  fill: none;
  stroke: var(--accent);
  stroke-width: 6;
  stroke-linecap: round;
  transform: rotate(-90deg);
  transform-origin: 50% 50%;
  transition:
    stroke-dashoffset 0.1s linear,
    stroke 0.3s ease;
  filter: drop-shadow(0 0 6px rgba(10, 132, 255, 0.3));
}
.progress-text {
  position: absolute;
  font-size: 2rem;
  font-weight: 700;
}

.proc-details {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 12px;
}
.proc-target {
  font-size: 1.2rem;
  font-weight: 600;
}
.pulse-bar {
  width: 150px;
  height: 4px;
  background: rgba(128, 128, 128, 0.2);
  border-radius: 2px;
  overflow: hidden;
  position: relative;
}
.pulse-indicator {
  position: absolute;
  top: 0;
  left: 0;
  height: 100%;
  width: 40%;
  background: var(--accent);
  border-radius: 2px;
  animation: sweep 1.5s infinite ease-in-out;
  filter: drop-shadow(0 0 8px rgba(10, 132, 255, 0.5));
}
@keyframes sweep {
  0% {
    transform: translateX(-100%);
    opacity: 0;
  }
  50% {
    opacity: 1;
  }
  100% {
    transform: translateX(300%);
    opacity: 0;
  }
}
.proc-status {
  font-size: 0.85rem;
  color: var(--text-muted);
}

/* Transitions */
.fade-enter-active,
.fade-leave-active {
  transition:
    opacity 0.3s ease,
    transform 0.3s ease;
}
.fade-enter-from,
.fade-leave-to {
  opacity: 0;
  transform: scale(0.95);
}

@media (prefers-reduced-motion: reduce) {
  .ambient-bg,
  .drop-icon-container,
  .ripple-ring,
  .pulse-indicator,
  .ring-fill,
  .files-panel,
  .fade-enter-active,
  .fade-leave-active {
    animation: none !important;
    transition: none !important;
  }
}
</style>
