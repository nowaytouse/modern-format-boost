# Modern Format Boost (Traditional Chinese)

<p align="center">
  <img src="https://img.shields.io/badge/version-0.11.2-0969DA?style=for-the-badge&logo=rust&logoColor=white" alt="版本">
  <img src="https://img.shields.io/badge/rust-2021_edition-E57324?style=for-the-badge&logo=rust&logoColor=white" alt="Rust">
  <img src="https://img.shields.io/badge/platform-macOS_%7C_Linux_%7C_Windows-8257E5?style=for-the-badge&logo=apple&logoColor=white" alt="平台">
  <img src="https://img.shields.io/badge/license-MIT-00B265?style=for-the-badge" alt="授權術語">
</p>

<p align="center">
  <strong>次世代媒體優化引擎 — 畫質零損耗，實現最大壓縮。</strong><br>
</p>

---

## 📖 繁體中文 (Traditional Chinese)

## 什麼是 Modern Format Boost？

**Modern Format Boost** 是一款基於 Rust 的高性能媒體優化引擎。它能將舊有的圖像和影片格式（JPEG, PNG, H.264, VP9…）轉換為頂尖的編碼格式（圖像為 **JPEG XL**，影片為 **HEVC/AV1**）——在保持甚至位元級精確（bit-exact）匹配原始畫質的同時，實現大幅度的檔案體積縮減。

您可以將其視為一個**絕不降質的「智慧壓縮器」**：

- 📸 **圖像**：JPEG → JXL 無損重建（位元精確，縮減約 20%）；PNG/WebP/TIFF/HEIC → JXL
- 🎬 **影片**：H.264/VP9/AV1 → HEVC，具備 GPU 加速的品質搜尋
- 🍎 **Apple 生態系優先**：完整的 Apple 相容模式、Live Photo 偵測、AAE 附屬檔案處理
- 🔒 **元數據守護者**：完整保留 EXIF、XMP、ICC 配置文件、建立時間戳記、macOS xattrs、Finder 標籤
- ⚡ **感知速度優化**：「深度優先」排序策略——優先處理較深層次的目錄，再依檔案大小和格式排序，確保高效的批次處理與最大吞吐量。
- 🎞️ **HDR10+ 動態元數據**：透過提取附屬檔案和 x265 SEI 注入，完整保留 SMPTE 2094-40 元數據。
- 🌅 **HDR 增益圖合成**：自動從 Apple/Samsung/ISO HEIC 增益圖中合成高保真 32 位元線性 HDR 緩衝區，確保轉換為 JXL 時保留最大動態範圍。
- **🔍 廠商元數據識別**：智慧掃描 HEIC 檔案中 Samsung/Google 特有的 XMP 命名空間，確保最大程度保留上下文。

## ⚠️ 免責聲明與重要事項

1. **數據安全第一**：為了避免任何潛在的數據丟失，強烈建議將處理後的檔案輸出到單獨的目錄（例如使用 `-o /path/to/output`），而非使用原地轉換（`--in-place`），對於不可替代的媒體檔案尤應如此。
2. **測試版軟體**：雖然本程式已經過廣泛測試、除錯和優化，以防止品質或數據丟失（詳見更新日誌），但不能保證 100% 無誤。請在 GitHub 上回報您遇到的任何問題。
3. **計算資源說明**：雖然已針對效率進行優化（特別是 Apple Silicon M 系列），但在 `--ultimate` 模式下處理大規模批次任務仍可能耗時較長，並會長時間佔用系統資源；請妥善規劃您的任務。
4. **工具成熟度**：統一後的工具（`img`, `vid`）預設使用 HEVC 策略，這比目前的 AV1 策略更成熟穩定。對於高可靠性的生產任務，建議使用 HEVC 策略（預設）。

## 🔒 隱私與數據完整性

**Modern Format Boost** 採用「在地優先」架構，確保您的創意資產完全由您控制。

- **離線作業**：100% 離線處理。無遙測、無使用追蹤、無雲端通訊。核心執行檔不包含任何網路相關代碼。
- **Rust 加固運行時**：使用 Rust 構建，原生消除內存損壞漏洞（緩衝區溢位等）。
- **安全整合**：所有外部工具（FFmpeg, cjxl）均透過安全的轉義原語調用，絕不透過原始 shell 執行，防止任意指令注入。
- **路徑隔離**：進階規範化防止目錄遍歷並保護無關的系統檔案。
- **系統路徑黑名單**：內建針對敏感系統目錄的防護，防止意外修改作業系統檔案。
- **動態資源平衡**：根據內存/CPU 負載自動調整處理線程，防止在極端任務期間發生系統崩潰。
- **全面元數據託管**：嚴格逐位保留 EXIF、XMP、ICC 和文件系統時間戳記 (btime/mtime)。

<details>
<summary><b>🛠️ 技術深挖：工作原理 — 處理流水線</b></summary>

### 圖像流水線邏輯

每個檔案都會經過多階段決策流水線：

- **階段 1 — 智慧偵測**：在二進制級別分析 JPEG DQT 表（UltraHDR 增益圖偵測）、WebP VP8L 區塊和 AVIF `av1C` 盒子。
- **階段 2 — 路徑與編碼**：JPEG 使用 JXL VarDCT（位元精確）；無損來源（PNG、無損 WebP/AVIF/HEIC/EXR/JP2）使用 Modular 模式。
- **階段 3 — 繞道處理**：TIFF/WebP/BMP/HEIC 等格式會預先處理為臨時 16 位元 PNG 或 **32 位元 OpenEXR**，以確保 `cjxl` 相容性且無品質損失。
- **階段 4 — HEIC HDR 合成**：攔截帶有增益圖（Apple/Google）的 HEIC 檔案，並透過中間的 **OpenEXR** 陪同流水線合成 32 位元線性光 HDR 緩衝區，輸出真正的 HDR JXL。
- **階段 5 — 循環意圖 (Loop Intent v3)**：採用全新的 7 層分層決策樹模型。綜合評估 **Loop Closure (循環閉合度)**、**Motion Gini (運動不均勻度)**、**週期性分析** 以及 **KNN 語義融合**，智能識別圖像或影片的循環意圖。

### 影片流水線：三階段飽和搜尋

1. **階段 1：GPU 粗略搜尋**：在硬體編碼器上進行二分搜尋以找到「品質拐點」。
2. **階段 2：CPU 精細調整**：將 GPU CRF 映射到 `x265` 刻度。使用 **Sprint & Backtrack**（成功則加倍步進，超標則重置回 0.1）。
3. **階段 3：終極 3D 品質關卡**：要求同時通過 VMAF-Y ≥ 92.0, CAMBI ≤ 6.0 (條帶) 和 PSNR-UV ≥ 34.0 dB。
   - **融合評分**：結合 MS-SSIM + SSIM_All 進行穩健的結構分析。

   </details>

### 🖥️ 運行截圖

![Runtime](../assets/runtime.png)

### 兩個統一工具

| 工具      | 用途     | 目標編碼                         |
| --------- | -------- | -------------------------------- |
| **`img`** | 圖像優化 | → JXL (靜態) / HEVC / AV1 (動態) |
| **`vid`** | 影片優化 | → HEVC / AV1                     |

## 📉 真實壓縮案例

| 輸入格式       | 原始大小 | 輸出格式 | 輸出大小 | 節省空間 | 方法             |
| :------------- | :------- | :------- | :------- | :------- | :--------------- |
| 風景 JPEG      | 4.2 MB   | **JXL**  | 3.3 MB   | **~21%** | 無損組件重建     |
| 螢幕截圖 PNG   | 2.5 MB   | **JXL**  | 1.1 MB   | **~56%** | Modular d=0.0    |
| 運動相機 H.264 | 1.2 GB   | **HEVC** | 480 MB   | **~60%** | GPU/CPU CRF 搜尋 |

## ⬇️ 安裝說明

### 預編譯版本下載

不希望安裝 Rust 工具鏈的使用者，可以從 **[Releases](https://github.com/nowaytouse/modern-format-boost/releases)** 頁面下載預編譯的執行檔。

```bash
# macOS/Linux 一鍵指令 (以 macOS ARM64 為例)
curl -LO https://github.com/nowaytouse/modern-format-boost/releases/latest/download/modern-format-boost-aarch64-apple-darwin.tar.gz
tar -xzf modern-format-boost-aarch64-apple-darwin.tar.gz
```

### 先決條件

| 工具 | 需要嗎？ | 目的 | 安裝命令 |
| :--- | :---: | :--- | :--- |
| **Rust** (1.75+) | ✅ | 構建與安裝 | `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \| sh` |
| **FFmpeg** (5.0+) | ✅ | 影片處理與指標 | `brew install ffmpeg` / `apt install ffmpeg` |
| **libjxl** | ✅ | JXL 編碼核心 | `brew install jpeg-xl` |
| **ExifTool** | ✅ | 元數據保存 | `brew install exiftool` |
| **ImageMagick** | ✅ | 圖像繞道處理 | `brew install imagemagick` |
| **libwebp** | ✅ | WebP 原生解碼 | `brew install webp` |
| **dovi_tool** | ✅ | 杜比視界 RPU 提取 | `cargo install dovi_tool` |
| **libheif** | ✅ | HEIC/HEIF 解碼 | `brew install libheif` |
| **hdr10plus_tool** | ✅ | HDR10+ 元數據提取 | `cargo install hdr10plus_tool` |

---

## ❓ FAQ

**1. JXL 格式目前的相容性如何？**  
macOS 14 (Sonoma) / iOS 17+、Chrome 91+ 以及 Firefox 128+ 已提供了原生支援。但目前 Apple 生態仍存在已知缺陷：
- **動圖預覽**：現代動圖格式（JXL/AV1/HEIF）在 macOS/iOS 原生相冊或 Finder 中往往無法直接播放動圖（顯示為靜態圖），尤其是 iCloud 同步後的文件。建議透過命令行工具或現代瀏覽器進行預覽。
- **縮略圖黑屏**：當 JXL 文件使用 **灰色 (Grayscale) ICC 配置文件** 時，Finder/iCloud 的縮略圖可能會顯示為純黑，但这並不影響文件本身，在瀏覽器中打開可正常顯示。
JXL 依然是目前進行位精確無損歸檔及高保真 HDR 儲存的最佳選擇。

**2. 為什麼 HDR10+ 動態影片會失效？**  
現已完美支援。我們透過 `hdr10plus_tool` 提取 SMPTE 2094-40 動態元數據並將其注入 `libx265` 的 `--dhdr10-info` 參數中。請確保已安裝該工具。

**3. 為什麼程式會自動跳過我的 WebP / AVIF / HEIC 圖像？**  
這些格式本身已屬於現代有損編碼。二次編碼會導致畫質代際損傷 (Generational Loss)，程式預設會跳過以保護品質。
**例外情況**：如果偵測到文件中包含 Apple/Google 高保真 **HDR Gainmap**，程式會將其合成輸出為 JXL；或者當動圖觸發了 **循環意圖** 優化機制時，也會進行相應處理。

---

## ⚖️ 授權

本專案採用 **MIT 授權**。
