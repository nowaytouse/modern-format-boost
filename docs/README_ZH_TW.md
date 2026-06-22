# Modern Format Boost

![Version](https://img.shields.io/badge/version-0.11.3-0969DA?style=for-the-badge&logo=rust&logoColor=white)
![Rust](<https://img.shields.io/badge/rust-2024_edition_(nightly)-E57324?style=for-the-badge&logo=rust&logoColor=white>)
![Platform](https://img.shields.io/badge/platform-macOS_%7C_Linux_%7C_Windows-8257E5?style=for-the-badge&logo=apple&logoColor=white)
![License](https://img.shields.io/badge/license-MIT-00B265?style=for-the-badge)

**下一代媒體最佳化引擎 —— 零品質損失，最大化壓縮。**

[English](../README.md) · [简体中文](README_ZH.md) · [繁體中文](README_ZH_TW.md) · [日本語](README_JA.md) · [한국어](README_KO.md) · [Español](README_ES.md) · [Français](README_FR.md) · [Português](README_PT.md) · [Русский](README_RU.md) · [العربية](README_AR.md)

## 什麼是 Modern Format Boost？

**Modern Format Boost** 是一個高效能、基於 Rust 的媒體最佳化引擎。它依媒體領域劃分工作：

- `img` 僅處理**靜態圖片**
- `vid` 處理**影片與動畫媒體**

在目前的實作中，典型的路線是：

- 📸 **靜態圖片（`img run` 主 CLI 路徑）**：JPEG → JXL 無損重建；PNG/TIFF/BMP 及其他無損靜態圖 → JXL；通常跳過有損現代靜態圖；忽略動畫或動畫狀態不明的輸入。
- 🎬 **影片**：H.264 及其他非目標編碼通過 HEVC/AV1 品質搜尋；編碼/容器的選擇取決於 `--codec` 和 `--apple-compat`。
- 🎞️ **動畫媒體**：GIF/WebP/AVIF/APNG/HEIC/HEIF/JXL 動畫路由由 `vid` 加上共享的 `loop_intent` 策略管理。

可以將其視為一個保守的最佳化器，寧願選擇誠實的跳過/忽略結果，也不願造成隱性的品質損壞：

- 🍎 **蘋果生態優先**：全蘋果相容模式、原況相片 (Live Photo) 偵測、AAE 側錄檔案處理。
- 🔒 **元資料守護者**：保留 EXIF、XMP、ICC 設定檔、建立時間戳記、macOS xattrs、Finder 標籤。
- ⚡ **感知速度最佳化**：「深度優先」排序策略——優先處理目錄層級較深的檔案，然後依檔案大小和格式排序，以確保高效的批次處理和最大吞吐量。
- 🎞️ **HDR10+ 動態元資料**：通過提取側錄檔案和 x265 SEI 注入，實現 SMPTE 2094-40 元資料的完整保留。
- 🌅 **HDR 增益圖合成**：自動從蘋果/三星/ISO HEIC 增益圖 (Gainmap) 合成高真度 32 位元線性 HDR 緩衝區，確保在轉換為 JXL 時保留最大動態範圍。
- **🔍 廠商元資料識別**：智慧掃描 HEIC 檔案中三星/Google 特定的 XMP 命名空間，以確保最大程度的上下文保留。

## ⚠️ 免責聲明與重要提示

1. **資料安全第一**：為避免任何潛在的資料遺失，強烈建議將處理後的檔案輸出到單獨的目錄（例如，使用 `-o /path/to/output`），而不是使用原地轉換 (`--in-place`)，特別是對於不可替代的媒體。
2. **測試版軟體**：雖然該程式已經過廣泛的測試、除錯和最佳化以防止品質或資料遺失（詳見更新日誌），但不能保證 100% 無錯誤。請在 GitHub 上回報您遇到的任何問題。
3. **運算洞察**：雖然針對效率進行了最佳化（尤其是在蘋果 M 系列晶片上），但在 `--ultimate` 模式下處理大規模批次處理仍可能耗時較長。它將長時間佔用系統資源；請相應地規劃您的任務。
4. **工具成熟度**：統一工具（`img`、`vid`）預設使用 HEVC，它比 AV1 策略更成熟、更穩定。對於高可靠性的生產任務，建議使用 HEVC（預設）。

## 🔒 隱私與資料完整性

**Modern Format Boost** 構建在「在地優先」架構之上，確保您的創意資產完全在您的控制之下。

- **離線操作**：100% 離線處理。無遙測、無使用追蹤或雲端請求。核心執行檔不含任何網路相關代碼。
- **Rust 加固執行階段**：使用 Rust 構建，原生消除記憶體損壞錯誤（緩衝區溢位等）。
- **安全整合**：所有外部工具（FFmpeg、cjxl）都通過安全的、逸出的原語調用——絕不通過原始 shell 執行——從而防止任意指令注入。
- **路徑隔離**：先進的規範化處理可防止目錄遍歷，並保護無關的系統檔案。
- **系統路徑黑名單**：內建敏感系統目錄屏蔽，防止意外修改作業系統檔案。
- **動態資源平衡**：根據記憶體/CPU 負載自動調整處理執行緒，防止極端任務期間系統當機。
- **全方位元資料管家**：嚴格逐位元保留 EXIF、XMP、ICC 和檔案系統時間戳記 (btime/mtime)。
- **安全處理與工作階段隔離**：
  - **零工作區污染**：集中式追蹤 (`~/.mfb_progress/`) 保持您的媒體資料夾 100% 清潔。您的相片/影片中不會留下隱藏的元資料檔案。
  - **無衝突暫存檔**：每個中間分析檔案（YUV 流、分析分段）都使用隨機 UUID 進行唯一識別。這可以防止多執行個體碰撞，並確保清理時的「手術級精度」。
  - **啟動即清理**：無論任務成功完成還是在中斷後恢復，系統都會自動清除所有暫態資料。這種「自清理」架構確保您的磁碟不會留下被遺棄的处理殘餘。
  - **智慧檢查點重置**：自動偵測使用者何時手動刪除輸出目錄以「重新開始」，即使在恢復模式下也會觸發完整的狀態重置。

## 🛠️ 技術深入：工作流程 —— 流水線

### 圖片流水線邏輯

每個檔案都會經過多階段決策流水線：

- **階段 1 — 智慧偵測**：在二進位層級分析 JPEG DQT 表（UltraHDR 增益圖偵測）、WebP VP8L 區塊和 AVIF `av1C` 框。現在具備 **零技術債架構**，100% 符合 Clippy 標準，並具有穩健的 `OpenEXR`/`JPEG 2000` 標頭解析。
- **階段 2 — 路由與編碼**：JPEG 使用 JXL VarDCT（位元精確）；無損源（PNG、無損 WebP/AVIF/HEIC/EXR/JP2）使用 Modular 模式。
- **階段 3 — 迂迴路徑**：TIFF/WebP/BMP/HEIC 等格式被預處理為暫時 16 位元 PNG 或 **32 位元 OpenEXR**，以確保在不損失品質的情況下相容 `cjxl`（8/16/32 位元匹配流水線）。
- **階段 4 — HEIC HDR 合成**：攔截帶有增益圖（蘋果/Google）的 HEIC 檔案，並通過中間 **OpenEXR** 護送流水線合成 32 位元線性光 HDR 緩衝區，提供真實的 HDR JXL 輸出。
- **階段 5 — 靜態/動畫分離**：`img` 現在硬性拒絕動畫或動畫狀態不明的資產。動畫現代格式被委託給 `vid`，而不是在靜態流水線中進行轉換。
- **階段 6 — 循環意圖 v3**：共享的循環意圖邏輯決定動畫媒體是保持類似 GIF 的狀態還是進入影片流水線。蘋果相容的現代動畫交付策略在此集中管理。

### 影片流水線：三階段飽和搜尋

1. **階段 1：GPU 粗略搜尋**：在硬體編碼器（VideoToolbox/NVENC）上進行二分搜尋，以找到「品質拐點」。
2. **階段 2：CPU 精細調節**：將 GPU CRF 映射到 `x265` 等級。使用 **衝刺與回溯 (Sprint & Backtrack)**（成功時雙倍步長，超出時重置為 0.1）。
3. **階段 3：終極 3D 品質閘門**：要求同時通過 VMAF-Y ≥ 86.0（基準線，動態關聯）、CAMBI ≤ 6.0（色帶）和 PSNR-UV ≥ 30.0 dB（色度基準線）。
   - **融合評分**：結合 MS-SSIM + SSIM_All（權重 0.6/0.4）進行穩健的結構分析。
   - **色度防護**：自動偵測可能導致 libvmaf MS-SSIM 當機的小解析度，並回退到僅 Y 評分，以確保處理可靠性。
   - _註：在 `--ultimate` 模式下，只有在 **連續 50 個樣本** 顯示零品質增益後，搜尋才會終止，確保絕對飽和。_

### 元資料與 HDR 保留

- **HDR**：保留 bt2020 原色、PQ/HLG TRC 和母帶顯示元資料。
- **杜比視界 (Dolby Vision)**：通過 `dovi_tool` 提取 RPU 並注入 x265（Profile 7 → 8.1 轉換）。
- **macOS xattrs**：通過 `copyfile` 和 `setattrlist` 保留 Finder 標籤、加入日期和建立時間戳記。

### 🖥️ 執行階段

![Runtime](../assets/runtime.png)

執行階段

### 兩個執行檔

| 執行檔    | 用途                 | 目標編碼                  |
| --------- | -------------------- | ------------------------- |
| **`img`** | 僅靜態圖片最佳化     | → JXL / 跳過 / 忽略       |
| **`vid`** | 影片與動畫媒體最佳化 | → HEVC / AV1 / GIF / 跳過 |

此外還有一個 **macOS 按兩下應用程式** (`Modern Format Boost.app`)，用於拖放式批次處理。

## 📉 真實世界壓縮示例

| 輸入格式       | 原始大小 | 輸出格式       | 輸出大小 | 節省     | 方法             |
| :------------- | :------- | :------------- | :------- | :------- | :--------------- |
| 風景 JPEG      | 4.2 MB   | **JXL**        | 3.3 MB   | **~21%** | 無損組件重建     |
| 螢幕截圖 PNG   | 2.5 MB   | **JXL**        | 1.1 MB   | **~56%** | Modular d=0.0    |
| 運動相機 H.264 | 1.2 GB   | **HEVC**       | 480 MB   | **~60%** | GPU/CPU CRF 搜尋 |
| 動畫 WebP      | 15 MB    | **AV1 / HEVC** | 1.8 MB   | **~88%** | 轉碼為影片格式   |

## 📊 處理矩陣

### 圖片格式決策矩陣

| 輸入格式                              | 靜態？ | `img run` 中的動作 | 輸出       | 備註                              |
| :------------------------------------ | :----: | :----------------- | :--------- | :-------------------------------- |
| JPEG                                  |   ✅   | **無損重建**       | `.jxl`     | 位元精確 `cjxl --lossless_jpeg=1` |
| PNG / TIFF / BMP / 其他無損靜態圖     |   ✅   | **無損轉換**       | `.jxl`     | 可能先走迂迴路徑                  |
| WebP / AVIF / HEIC / HEIF (無損靜態)  |   ✅   | **轉換**           | `.jxl`     | 允許轉換無損現代靜態圖            |
| 帶有增益圖的 HEIC / HEIF              |   ✅   | **HDR 合成**       | `.jxl`     | 增益圖路徑合成線性 HDR            |
| 靜態驗證後的遺留有損靜態圖            |   ✅   | **近無損轉換**     | `.jxl`     | 目前 `img run` 批次路徑專注於 JXL |
| 有損 WebP / AVIF / HEIC / HEIF 靜態圖 |   ✅   | **跳過**           | 保留原檔案 | 避免代際損失                      |
| JXL 靜態圖                            |   ✅   | **跳過**           | 保留原檔案 | 已經是最佳格式                    |
| 任何動畫或動畫狀態不明的圖片          |   ❌   | **忽略**           | 無         | 超出 `img` 靜態唯一領域           |

### `img` 路由說明

目前倉庫中有兩個圖片轉換入口：

- `img run` / 批次 CLI 路徑，位於 `crates/img/src/main.rs`
- 函式庫助手 `smart_convert()`，位於 `crates/img/src/conversion_api.rs`

它們目前**尚未完全對齊**。

- 主 CLI 路徑目前針對接受的靜態轉換，實際偏向於 JXL。
- 較舊的助手 API 仍包含一些針對有損非 JPEG 靜態圖的 AVIF 目標分支。
- `img` CLI 也會解析 `--codec`，但在目前的靜態批次路徑中，該旗標**不會**實質性改變實際的路由決策。

本 README 首先記錄了**目前的 CLI/執行階段行為**，因為這是使用者在正常批次使用中遇到的情況。

### 動畫媒體決策矩陣

| 輸入格式                                    | 歸屬                  | 動作             | 輸出                     | 備註                       |
| :------------------------------------------ | :-------------------- | :--------------- | :----------------------- | :------------------------- |
| GIF                                         | `vid`                 | **循環意圖路由** | `.gif` 或 影片           | 保留 GIF 快速路徑          |
| 動畫 WebP / AVIF / APNG / HEIC / HEIF / JXL | `vid`                 | **循環意圖路由** | `.gif` / `.mov` / `.mp4` | `img` 忽略這些             |
| 帶有 `--apple-compat` 的短無聲現代動畫      | `vid` + `loop_intent` | **強制 GIF**     | `.gif`                   | 時長 `<= 6s`               |
| 帶有 `--apple-compat` 的長現代動畫          | `vid` + `loop_intent` | **不強制 GIF**   | 影片目標                 | 時長 `>= 15s` 保持影片風格 |
| 帶有 `--apple-compat` 的不確定現代動畫      | `vid` + `loop_intent` | **強制 GIF**     | `.gif`                   | 相容性回退                 |

### 影片編碼決策矩陣

| 輸入編碼                  | 普通模式     | `--apple-compat` 模式 | 備註                              |
| :------------------------ | :----------- | :-------------------- | :-------------------------------- |
| H.264 (AVC)               | **轉換**     | **轉換**              | 兩種模式下都不會被預跳過          |
| VP9                       | **跳過**     | **轉換為 HEVC**       | 蘋果不相容的源                    |
| AV1                       | **跳過**     | **轉換為 HEVC**       | 蘋果不相容的源                    |
| VVC / AV2                 | **跳過**     | **轉換為 HEVC**       | 蘋果不相容的源                    |
| HEVC (H.265)              | **跳過**     | **跳過**              | 已經是蘋果原生目標                |
| ProRes / DNxHD / 遺留編碼 | **按需轉換** | **按需轉換**          | 最終的保留/跳過仍取決於最佳化結果 |

路由後仍需通過品質和大小門檻。在 `--ultimate` 和其他品質匹配流程中，符合轉換條件的路由如果產生的檔案未通過品質/大小要求且沒有允許的最佳實務回退，最終仍可能以跳過告終。

### HDR 格式策略

| HDR 類型          | 偵測                                       | 保留策略                                                                  |
| :---------------- | :----------------------------------------- | :------------------------------------------------------------------------ |
| **HDR10**         | side_data 中的 mastering_display + max_cll | 通過 FFmpeg 參數完整保留靜態元資料                                        |
| **HEIC 增益圖**   | HEIC 輔助圖片 (蘋果/三星/ISO)              | 合成為 32 位元線性 HDR -> JXL (真實 HDR)                                  |
| **UltraHDR JPEG** | JPEG APP1/APP2 + XMP (hdrgm:)              | 保留元資料；發出增益圖損失警告                                            |
| **HLG**           | color_trc = arib-std-b67                   | 保留原色 + TRC                                                            |
| **杜比視界**      | 流/幀中的 DOVI side_data                   | 通過 `dovi_tool` 提取 RPU → x265 注入；Profile 7 → 8.1 轉換               |
| **HDR10+**        | ST2094-40 動態元資料                       | 通過 `hdr10plus_tool` 側錄提取和 x265 注入支援（保留 Profile A/B 元資料） |
| **SDR**           | 無 HDR 標記                                | 標準處理 (yuv420p)                                                        |

## ⬇️ 安裝

### 預編譯執行檔

對於不想安裝 Rust 工具鏈的使用者，可以從 **[Releases](https://github.com/nowaytouse/modern-format-boost/releases)** 頁面下載預編譯的執行檔。

```bash
# macOS/Linux 一鍵安裝（以 macOS ARM64 為例）
curl -LO https://github.com/nowaytouse/modern-format-boost/releases/latest/download/modern-format-boost-aarch64-apple-darwin.tar.gz
tar -xzf modern-format-boost-aarch64-apple-darwin.tar.gz

```

### 前提條件

| 工具                 | 必需？ | 用途                 | 安裝指令                                                                                    |
| :------------------- | :----: | :------------------- | :------------------------------------------------------------------------------------------ |
| **Rust** (nightly)   |   ✅   | 構建與安裝           | `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \| sh && rustup default nightly` |
| **FFmpeg** (5.0+)    |   ✅   | 影片處理與指標計算   | `brew install ffmpeg` / `apt install ffmpeg`                                                |
| **libjxl**           |   ✅   | JXL 編碼核心         | `brew install jpeg-xl`                                                                      |
| **ExifTool**         |   ✅   | 元資料保留           | `brew install exiftool`                                                                     |
| **ImageMagick**      |   ✅   | 圖片迂回路徑         | `brew install imagemagick`                                                                  |
| **libwebp**          |   ✅   | WebP 原生解碼        | `brew install webp`                                                                         |
| **libheif**          |   ✅   | HEIC/HEIF 解碼       | `brew install libheif`                                                                      |
| **PostgreSQL** (12+) |   ✅   | 快取與品質特徵資料庫 | `brew install postgresql pgvector` / `apt install postgresql`                               |
| **dovi_tool**        |  可選  | 杜比視界 RPU 提取    | `cargo install dovi_tool`                                                                   |
| **hdr10plus_tool**   |  可选  | HDR10+ 元資料提取    | `cargo install hdr10plus_tool`                                                              |

#### macOS (Homebrew)

```bash
brew install ffmpeg jpeg-xl exiftool imagemagick webp libheif postgresql pgvector
```

#### Linux (Ubuntu/Debian)

```bash
sudo apt update && sudo apt install ffmpeg libimage-exiftool-perl imagemagick \
  webp libheif-dev postgresql postgresql-contrib postgresql-server-dev-all
# Linux 下必須編譯並安裝 pgvector 擴充功能：
git clone --branch v0.5.1 https://github.com/pgvector/pgvector.git
cd pgvector
make && sudo make install
```

### 🗄️ 資料庫設定

Modern Format Boost 使用 PostgreSQL (並啟用 `pgvector` 擴充功能) 作為強制的本地快取與品質特徵推理引擎。`img` 和 `vid` 二進位程式在啟動時均會嘗試連線資料庫，如果資料庫服務無法連線，程式將直接報錯並退出。

#### 1. 啟動 PostgreSQL 服務

- **macOS**: `brew services start postgresql`
- **Linux**: `sudo systemctl start postgresql`

#### 2. 建立資料庫

預設的資料庫名稱為 `modern_format_boost`。在執行工具前，請建立該資料庫：

```bash
createdb modern_format_boost
```

或通過 SQL：

```sql
CREATE DATABASE modern_format_boost;
```

### 從原始碼構建

```bash
git clone https://github.com/nowaytouse/modern-format-boost.git
cd modern-format-boost
cargo build --release

```

## 🚀 使用方法

### 快速開始

```bash
# 圖片路徑轉換
img run /path/to/media
# 影片路徑轉換
vid run /path/to/media

# 使用 AV1 策略：
vid run --codec av1 /path/to/media
```

### ⚡ 極速模式與智能斷點續傳

針對拖放式 UI 工作流，**Fast Img Flow**（極速模式，由 `crates/dev/src/bin/drag_and_drop_processor.rs` 驅動）帶來了高可靠性的斷點續傳能力：

- **`WorkingCopyMarker` 狀態管理**：安全追蹤關閉和中斷時的處理進度。
- **陳舊原始檔案檢測**：如果原始檔案發生變更，系統會自動探測到資料集過時，放棄髒重試並觸發全新重建。
- **Fail-Closed 錯誤保護**：深度上下文捕獲和 Blake3 校验保證在 `img run` 中斷時不會出現任何檔案損壞，安全回退。

### 詳細選項

- `--ultimate`：存檔級 **0.01 精度** 搜尋（高品質，高時間成本）。
- `--apple-compat`：啟用蘋果生態系統相容性（原況相片/AAE）。CLI 預設為開啟；`--no-apple-compat` 可停用。
- `--in-place`：替換原始檔案。**警告：不可逆。**
- `-o /dir`：安全輸出目錄。（推薦）
- `--verbose`：顯示詳細的處理日誌。
- `--no-recursive`：不進入子目錄。
- `--force-video`：強制將動畫圖片視為影片，不考慮循環意圖 (Loop Intent)。

### 進階次指令

- `img cache-stats`：檢視 SQLite 分析快取統計資訊。
- `vid strategy <file>`：預覽特定檔案的流水線策略。
- `img restore-timestamps`：根據檔名模式批次修復建立日期（元資料恢復）。

### 💡 多執行個體說明

**Modern Format Boost** 原生支援執行多個視窗/執行個體。

- **並行處理**：允許執行多個視窗以獨立處理不同路徑。
- **注意**：請根據您的硬體 I/O 效能進行擴展；過高的並行可能會導致檔案系統競態條件。

## 🏗️ 架構

### CI/CD 與測試門控體系

Modern Format Boost 使用嚴苛的質量門控系統保證核心架構的零技術債：

- **Rust 優先開發工具鏈**：工程入口是 `crates/dev/src/bin` 下的 Rust 二進位；Python 原件僅作為相容參考保留，直到確認可安全刪除。
- **本地 CI 校驗**：開發前務必使用 `just fix-gate` 或 `cargo run --locked -p dev --bin check_all -- --allow-non-nightly` 進行檢查，這是代碼格式、靜態檢查、以及自動化測試的“單一事實來源”(SSOT)。
- **測試強化與穩定性**：禁用 "Fail Fast" 以便在多平台上收集全量診斷信息；同時增加了對圖像（如 JPEG 恢復斷言）錯誤狀態的深度上下文捕獲。

### 核心架構

- `crates/img/`：靜態圖片最佳化器（目前 CLI 路徑中為 `JXL` / 跳過 / 忽略）
- `crates/vid/`：影片與動畫媒體最佳化器（`HEVC` / `AV1` / `GIF`）
- `crates/foundation/`：核心大腦（GPU/CPU 混合引擎、HDR 映射、元資料）
- `Modern Format Boost.app/`：macOS 拖放式 UI

## ❓ 常見問題

**1. JXL 是否得到廣泛支援？**
macOS 14+ / iOS 17+、Chrome 91+ 和 Firefox 128+ 已提供原生支援。然而，目前仍存在一些已知的生態系統問題：

- **動畫**：現代動畫格式 (JXL/AV1/HEIF) 在原生 macOS/iOS 相片應用程式或 Finder 中經常無法作為動畫預覽（僅顯示靜態），尤其是在通過 iCloud 同步時。它們在現代瀏覽器或專用工具中可以正常播放。
- **縮圖**：使用**灰階 ICC 設定檔**的 JXL 檔案在 Finder/iCloud 中可能顯示為**黑色縮圖**，儘管開啟時渲染完美。
  對於位元精確存檔和高真度 HDR 儲存，JXL 仍然是卓越的格式。

**2. HDR10+ 如何處理？**
完全支援。我們使用 `hdr10plus_tool` 提取 SMPTE 2094-40 動態元資料，並通過 `libx265` 的 `--dhdr10-info` 參數將其注回 HEVC 流。請確保已安裝該工具以啟用此功能。

**3. 為什麼跳過 WebP/AVIF/HEIC？**
靜態有損 WebP/AVIF/HEIC/HEIF 通常會被跳過，因為它們本身已經是現代有損格式，重新編碼可能會面臨代際損失，而收益有限。目前代碼中的重要例外包括：

- 無損現代靜態圖仍可轉換為 JXL
- HEIC/HEIF 增益圖資產可以合成位元 HDR JXL
- 動畫現代格式不由 `img` 處理；它們通過 `vid` 和 `loop_intent` 進行路由

---

## ⚖️ 許可證

根據 **MIT 許可證** 授權。

## 執行階段依賴

本專案編排了多個開源巨作。我們感謝其作者的貢獻：

| 元件                   | 許可證     | 用途               |
| ---------------------- | ---------- | ------------------ |
| **FFmpeg**             | LGPL/GPL   | 影片處理與指標計算 |
| **libjxl** (cjxl/djxl) | BSD-3      | JPEG XL 編碼       |
| **ExifTool**           | Perl/GPL   | 元資料保留         |
| **ImageMagick**        | Apache 2.0 | 圖片迂回路徑       |
| **SVT-AV1**            | BSD+Patent | AV1 編碼           |
| **x265**               | GPL-2.0    | HEVC 編碼          |

所有 Rust 依賴項均通過 `Cargo.toml` 管理，並遵循其各自的開源許可證（MIT/Apache/BSD）。
