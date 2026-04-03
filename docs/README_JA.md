<p align="center">
  <img src="https://img.shields.io/badge/version-0.11.2-0969DA?style=for-the-badge&logo=rust&logoColor=white" alt="バージョン">
  <img src="https://img.shields.io/badge/rust-2021_edition-E57324?style=for-the-badge&logo=rust&logoColor=white" alt="Rust">
  <img src="https://img.shields.io/badge/platform-macOS_%7C_Linux_%7C_Windows-8257E5?style=for-the-badge&logo=apple&logoColor=white" alt="プラットフォーム">
  <img src="https://img.shields.io/badge/license-MIT-00B265?style=for-the-badge" alt="ライセンス">
</p>

<h1 align="center">Modern Format Boost</h1>

<p align="center">
  <strong>次世代メディア最適化エンジン — 画質劣化ゼロ、最大圧縮を実現。</strong><br>
</p>

---

# 📖 日本語 (Japanese)

## Modern Format Boost とは？

**Modern Format Boost** は、Rust製の高性能メディア最適化エンジンです。古い画像・動画形式（JPEG, PNG, H.264, VP9…）を最新のコーデック（画像は **JPEG XL**、動画は **HEVC/AV1**）に変換します。元の画質を維持、あるいはビット単位で一致（bit-exact）させながら、ファイルサイズを劇的に削減します。

メディアを**決して劣化させない「スマートな圧縮機」**と考えてください。

- 📸 **画像**: JPEG → JXL（可逆再構築、ビット一致、約20%削減）。PNG/WebP/TIFF/HEIC → JXL。
- 🎬 **動画**: H.264/VP9/AV1 → HEVC（GPU加速による最適品質探索）。
- 🍎 **Appleエコシステム優先**: Apple完全互換モード、Live Photo検出、AAEサイドカー対応。
- 🔒 **メタデータの守護者**: EXIF, XMP, ICCプロファイル, 作成日時, macOS拡張属性 (xattrs), Finderタグを完全に保持。
- ⚡ **知覚速度の最適化**: 「ディープファースト」ソート戦略。深い階層のディレクトリを優先し、ファイルサイズと形式でソートすることで、効率的なバッチ処理と最大のスループットを実現します。
- 🎞️ **HDR10+ 動的メタデータ**: サイドカー抽出と x265 SEI インジェクションにより、SMPTE 2094-40 メタデータを完全に保持。
- 🌅 **HDR ゲインマップ合成**: Apple/Samsung/ISO の HEIC ゲインマップから、高精度な32bitリニアHDRバッファを自動合成。JXL 変換時に最大のダイナミックレンジを維持します。
- **🔍 ベンダーメタデータ認識**: HEIC ファイル内の Samsung/Google 固有の XMP ネームスペースをインテリジェントにスキャンし、コンテキストを最大限に保持します。

## ⚠️ 免責事項と重要な注意点

1. **データ安全第一**: データの損失を避けるため、特にかけがえのないメディアについては、上書き変換（`--in-place`）ではなく、別のディレクトリに出力（`-o /path/to/output`）することを強く推奨します。
2. **ベータ版ソフトウェア**: 本プログラムは画質やデータの損失を防ぐために広範囲にテストおよび最適化されていますが、100%バグがないことを保証するものではありません。問題が発生した場合は GitHub で報告してください。
3. **計算リソースについて**: Apple Silicon Mシリーズなどで効率化されていますが、`--ultimate` モードでの大規模なバッチ処理には時間がかかり、システムリソースを長時間占有します。計画的に実行してください。
4. **ツールの成熟度**: 現在、HEVCベースのツール（`img-hevc`, `vid-hevc`）の方が AV1ベース（`img-av1`, `vid-av1`）よりも成熟しており、安定しています。信頼性が求められる作業には HEVC ツールを推奨します。

## 🔒 プライバシーとデータの完全性

**Modern Format Boost** は「ローカルファースト」のアーキテクチャを採用しており、クリエイティブな資産を完全に制御下に置くことができます。

- **オフライン動作**: 100% オフライン処理。テレメトリ、利用状況追跡、クラウドへの通信は一切行いません。
- **Rust による安全性**: メモリ破壊バグ（バッファオーバーフロー等）をネイティブに排除する Rust で構築されています。
- **安全な統合**: FFmpeg や cjxl などの外部ツールは、生のシェル実行ではなく、エスケープされた安全なプリミティブを介して呼び出されるため、コマンド注入を防ぎます。
- **パスの隔離**: 高度な正規化により、ディレクトリトラバーサルを防ぎ、無関係なシステムファイルを保護します。
- **システムパス・ブロックリスト**: システムの重要ディレクトリに対する偶発的な変更を防ぐシールド機能を内蔵。
- **動的リソース調整**: メモリ/CPU負荷に基づいて処理スレッドを自動調整し、過酷なタスク実行時のシステムクラッシュを防ぎます。
- **包括的なメタデータ管理**: EXIF, XMP, ICC, ファイルシステムタイムスタンプ (btime/mtime) をビット単位で厳密に保持。

## 🛠️ 技術的な詳細：動作の仕組み

### 画像パイプライン

各ファイルは多段階の判定パイプラインを通過します。

- **ステージ1 — スマート検出**: JPEG DQTテーブル（UltraHDRゲインマップ検出）、WebP VP8Lチャンク、AVIF `av1C`ボックスをバイナリレベルで解析。
- **ステージ2 — ルートとエンコード**: JPEG は JXL VarDCT（ビット一致）、可逆ソース（PNG等）は Modular モードを使用。
- **ステージ3 — 回避経路**: TIFF/BMP/HEIC 等は、画質損失を防ぐために一時的に16bit PNGまたは32bit OpenEXRに変換して処理されます。

### 動画パイプライン：三段階の飽和探索

1. **フェーズ1: GPU 粗探索**: ハードウェアエンコーダ（VideoToolbox/NVENC）による二分探索で「品質の膝（曲がり角）」を特定。
2. **フェーズ2: CPU 微調整**: GPU の CRF を `x265` スケールにマッピング。Sprint & Backtrack 戦略を採用。
3. **フェーズ3: 究極の3D品質ゲート**: VMAF-Y ≥ 92.0, CAMBI ≤ 6.0, PSNR-UV ≥ 34.0 dB の同時クリアを要求。
   - **フュージョンスコア**: MS-SSIM + SSIM_All を組み合わせた堅牢な構造解析。
   - _注意: `--ultimate` モードでは、50サンプル連続で品質向上が見られない場合のみ探索を終了し、絶対的な飽和を保証します。_

### 🖥️ 実行画面

![Runtime](../assets/runtime.png)

### 4つのバイナリ

| バイナリ       | 用途       | ターゲットコーデック                   |
| -------------- | ---------- | -------------------------------------- |
| **`img-hevc`** | 画像最適化 | → JXL (静止画) / HEVC (アニメーション) |
| **`img-av1`**  | 画像最適化 | → JXL (静止画) / AV1 (アニメーション)  |
| **`vid-hevc`** | 動画最適化 | → HEVC / H.265                         |
| **`vid-av1`**  | 動画最適化 | → AV1 / SVT-AV1                        |

さらに、ドラッグ＆ドロップで処理可能な **macOS用アプリ** (`Modern Format Boost.app`) も付属しています。

## 📉 圧縮例（実測値）

| 入力形式               | 元のサイズ | 出力形式 | 出力サイズ | 削減率   | 方法                     |
| :--------------------- | :--------- | :------- | :--------- | :------- | :----------------------- |
| 風景 JPEG              | 4.2 MB     | **JXL**  | 3.3 MB     | **~21%** | 可逆コンポーネント再構築 |
| スクリーンショット PNG | 2.5 MB     | **JXL**  | 1.1 MB     | **~56%** | Modular d=0.0            |
| アクションカメラ H.264 | 1.2 GB     | **HEVC** | 480 MB     | **~60%** | GPU/CPU CRF 探索         |

## ⬇️ インストール

### プリコンパイル済みバイナリ

Rust の環境構築が不要な方は、**[Releases](https://github.com/nowaytouse/modern-format-boost/releases)** ページからダウンロードしてください。

```bash
# macOS/Linux 用（例: macOS ARM64）
curl -LO https://github.com/nowaytouse/modern-format-boost/releases/latest/download/modern-format-boost-aarch64-apple-darwin.tar.gz
tar -xzf modern-format-boost-aarch64-apple-darwin.tar.gz
```

---

# ⚖️ ライセンス

**MIT ライセンス**の下で公開されています。
