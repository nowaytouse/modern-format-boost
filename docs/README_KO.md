# Modern Format Boost

![Version](https://img.shields.io/badge/version-0.11.3-0969DA?style=for-the-badge&logo=rust&logoColor=white)
![Rust](<https://img.shields.io/badge/rust-2024_edition_(nightly)-E57324?style=for-the-badge&logo=rust&logoColor=white>)
![Platform](https://img.shields.io/badge/platform-macOS_%7C_Linux_%7C_Windows-8257E5?style=for-the-badge&logo=apple&logoColor=white)
![License](https://img.shields.io/badge/license-MIT-00B265?style=for-the-badge)

**차세대 미디어 최적화 엔진 — 화질 손실 제로, 최대 압축.**

[English](../README.md) · [简体中文](README_ZH.md) · [繁體中文](README_ZH_TW.md) · [日本語](README_JA.md) · [한국어](README_KO.md) · [Español](README_ES.md) · [Français](README_FR.md) · [Português](README_PT.md) · [Русский](README_RU.md) · [العربية](README_AR.md)

## Modern Format Boost란 무엇인가요?

**Modern Format Boost**는 Rust 기반의 고성능 미디어 최적화 엔진입니다. 미디어 도메인에 따라 작업을 분리합니다:

- `img`: **정적 이미지만** 처리
- `vid`: **동영상 및 애니메이션 미디어** 처리

현재 구현에서의 일반적인 경로는 다음과 같습니다:

- 📸 **정적 이미지 (`img run` 메인 CLI 경로)**: JPEG → JXL 무손실 재구축; PNG/TIFF/BMP 및 기타 무손실 정적 이미지 → JXL; 손실 압축된 최신 정적 이미지는 일반적으로 건너뜁니다. 애니메이션 또는 애니메이션 여부가 불분명한 입력은 무시됩니다.
- 🎬 **동영상**: H.264 및 기타 비대상 코덱은 HEVC/AV1 품질 검색을 거칩니다. 코덱/컨테이너 선택은 `--codec` 및 `--apple-compat` 옵션에 따라 달라집니다.
- 🎞️ **애니메이션 미디어**: GIF/WebP/AVIF/APNG/HEIC/HEIF/JXL 애니메이션 라우팅은 `vid`와 공유된 `loop_intent` 정책에 의해 관리됩니다.

이 엔진은 보이지 않는 품질 저하보다는 정직한 건너뛰기/무시 결과를 선호하는 보수적인 최적화 도구입니다:

- 🍎 **Apple 생태계 우선**: 완전한 Apple 호환 모드, Live Photo 감지, AAE 사이드카 처리를 지원합니다.
- 🔒 **메타데이터 보호**: EXIF, XMP, ICC 프로필, 생성 시간, macOS xattrs, Finder 태그를 보존합니다.
- ⚡ **체감 속도 최적화**: "Deep-First" 정렬 전략을 사용하여 하위 디렉터리 레벨을 우선 처리한 다음 파일 크기와 형식별로 정렬하여 효율적인 배치 작업과 최대 처리량을 보장합니다.
- 🎞️ **HDR10+ 동적 메타데이터**: 사이드카 추출 및 x265 SEI 주입을 통해 SMPTE 2094-40 메타데이터를 완벽하게 유지합니다.
- 🌅 **HDR 게인맵 합성**: Apple/Samsung/ISO HEIC 게인맵에서 고충실도 32비트 리니어 HDR 버퍼를 자동으로 합성하여 JXL 변환 시 최대 동적 범위를 보존합니다.
- **🔍 벤더 메타데이터 인식**: HEIC 파일 내 Samsung/Google 전용 XMP 네임스페이스를 지능적으로 스캔하여 최대 맥락 보존을 실현합니다.

## ⚠️ 면책 조항 및 중요 참고 사항

1. **데이터 안전 우선**: 잠재적인 데이터 손실을 방지하기 위해 원본 파일을 덮어쓰는 방식(`--in-place`)보다는 별도의 디렉터리에 출력하는 방식(예: `-o /path/to/output`)을 강력히 권장합니다. 특히 대체 불가능한 미디어의 경우 더욱 주의하십시오.
2. **베타 소프트웨어**: 이 프로그램은 화질이나 데이터 손실을 방지하기 위해 광범위하게 테스트, 디버깅 및 최적화되었지만(변경 로그 참조), 100% 버그가 없음을 보장하지는 않습니다. 문제가 발생하면 GitHub에 보고해 주세요.
3. **컴퓨팅 부하**: 효율성(특히 Apple Silicon M 시리즈)을 위해 최적화되었지만, `--ultimate` 모드에서 대량의 배치를 처리하는 데는 여전히 시간이 많이 소요될 수 있습니다. 장시간 시스템 리소스를 점유하므로 작업 계획을 적절히 세우시기 바랍니다.
4. **도구의 성숙도**: 통합 도구(`img`, `vid`)의 기본값은 HEVC이며, 이는 AV1 전략보다 더 성숙하고 안정적입니다. 높은 신뢰성이 필요한 실무 작업에는 HEVC(기본값)를 권장합니다.

## 🔒 개인정보 보호 및 데이터 무결성

**Modern Format Boost**는 "로컬 우선(Local-First)" 아키텍처로 구축되어 창작 자산이 완전히 사용자의 통제 하에 머물도록 보장합니다.

- **오프라인 동작**: 100% 오프라인 처리. 텔레메트리, 사용량 추적 또는 클라우드 핑이 없습니다. 핵심 바이너리에는 네트워크 관련 코드가 전혀 포함되어 있지 않습니다.
- **Rust 강화 런타임**: Rust로 빌드되어 메모리 손상 버그(버퍼 오버플로 등)를 네이티브 수준에서 제거합니다.
- **보안 통합**: 모든 외부 도구(FFmpeg, cjxl)는 원시 셸 실행이 아닌 안전하게 이스케이프된 프리미티브를 통해 호출되어 임의 명령 주입을 방지합니다.
- **경로 격리**: 고급 정규화를 통해 디렉터리 트래버설을 방지하고 관련 없는 시스템 파일을 보호합니다.
- **시스템 경로 차단 목록**: 민감한 시스템 디렉터리에 대한 보호 기능이 내장되어 있어 실수로 OS 파일을 수정하는 것을 방지합니다.
- **동적 리소스 균형 조정**: 메모리/CPU 부하에 따라 처리 스레드를 자동으로 조정하여 극한의 작업 중 시스템 충돌을 방지합니다.
- **포괄적인 메타데이터 관리**: EXIF, XMP, ICC 및 파일 시스템 타임스탬프(btime/mtime)를 엄격하게 비트 단위로 보존합니다.
- **보안 처리 및 세션 격리**:
  - **작업 공간 오염 제로**: 중앙 집중식 추적(`~/.mfb_progress/`)을 통해 미디어 폴더를 100% 깨끗하게 유지합니다. 사진/동영상 사이에 숨겨진 메타데이터 파일이 남지 않습니다.
  - **충돌 없는 임시 파일**: 모든 중간 분석 파일(YUV 스트림, 분석 세그먼트)은 무작위 UUID로 고유하게 식별됩니다. 이는 다중 인스턴스 간의 충돌을 방지하고 정리 시 "정밀한 정확성"을 보장합니다.
  - **시작 시 정리**: 작업이 성공적으로 완료되었든 중단 후 재개되었든 시스템은 모든 일시적 데이터를 자동으로 삭제합니다. 이 "자가 정리" 아키텍처는 디스크에 버려진 처리 잔해물이 남지 않도록 합니다.
  - **지능형 체크포인트 초기화**: 사용자가 수동으로 출력 디렉터리를 삭제하여 "처음부터 다시 시작"하는 것을 자동으로 감지하여 재개 모드에서도 전체 상태 초기화를 트리거합니다.

## 🛠️ 기술 심층 분석: 작동 원리 — 파이프라인

### 이미지 파이프라인 로직

모든 파일은 다단계 의사 결정 파이프라인을 거칩니다:

- **1단계 — 스마트 감지**: 이진 레벨에서 JPEG DQT 테이블(UltraHDR 게인맵 감지), WebP VP8L 청크 및 AVIF `av1C` 박스를 분석합니다. 현재 100% Clippy 준수 및 견고한 `OpenEXR`/`JPEG 2000` 헤더 파싱을 갖춘 **Zero-Debt 아키텍처**를 특징으로 합니다.
- **2단계 — 경로 지정 및 인코딩**: JPEG의 경우 JXL VarDCT(비트 단위 일치), 무손실 소스(PNG, 무손실 WebP/AVIF/HEIC/EXR/JP2)의 경우 Modular 모드를 사용합니다.
- **3단계 — 우회 경로**: TIFF/WebP/BMP/HEIC와 같은 형식은 화질 손실 없이 `cjxl` 호환성을 보장하기 위해 임시 16비트 PNG 또는 **32비트 OpenEXR**로 전처리됩니다(8/16/32비트 일치 파이프라인).
- **4단계 — HEIC HDR 합성**: 게인맵(Apple/Google)이 있는 HEIC 파일을 가로채서 중간 **OpenEXR** 에스코트 파이프라인을 통해 32비트 리니어 라이트 HDR 버퍼를 합성하여 진정한 HDR JXL 출력을 제공합니다.
- **5단계 — 정적/애니메이션 분리**: `img`는 이제 애니메이션 또는 애니메이션 여부가 불분명한 자산을 거부합니다. 애니메이션된 최신 형식은 정적 파이프라인 내에서 변환되는 대신 `vid`로 위임됩니다.
- **6단계 — Loop Intent v3**: 공유된 loop-intent 로직이 애니메이션 미디어를 GIF처럼 유지할지 또는 비디오 파이프라인으로 진행할지 결정합니다. Apple 호환 최신 애니메이션 전송 정책이 여기에 통합되어 있습니다.

### 비디오 파이프라인: 3단계 포화 검색

1. **1단계: GPU 대략적 검색**: 하드웨어 인코더(VideoToolbox/NVENC)에서 이진 검색을 수행하여 "품질 굴곡점(quality knee)"을 찾습니다.
2. **2단계: CPU 미세 조정**: GPU CRF를 `x265` 스케일에 매핑합니다. "Sprint & Backtrack"(성공 시 두 배 단계, 오버슈트 시 0.1로 초기화) 방식을 사용합니다.
3. **3단계: 궁극의 3D 품질 게이트**: VMAF-Y ≥ 86.0(동적 기준 상대 하한선), CAMBI ≤ 6.0(밴딩), PSNR-UV ≥ 30.0 dB(크로마 하한선)의 동시 통과를 요구합니다.
   - **퓨전 스코어링**: MS-SSIM + SSIM_All (0.6/0.4 가중치)을 결합하여 견고한 구조 분석을 수행합니다.
   - **크로마 가드**: libvmaf MS-SSIM을 충돌시킬 수 있는 작은 해상도를 자동으로 감지하고 Y 전용 스코어링으로 대체하여 처리 신뢰성을 확보합니다.
   - _참고: `--ultimate` 모드에서는 **50회 연속 샘플**에서 품질 향상이 제로로 나타날 때까지 검색을 종료하지 않아 절대적인 포화를 보장합니다._

### 메타데이터 및 HDR 보존

- **HDR**: bt2020 프라이머리, PQ/HLG TRC 및 Mastering Display 메타데이터를 보존합니다.
- **Dolby Vision**: `dovi_tool`을 통해 RPU를 추출하고 x265에 주입합니다(Profile 7 → 8.1 변환).
- **macOS xattrs**: `copyfile` 및 `setattrlist`를 통해 Finder 태그, 추가된 날짜 및 생성 시간을 보존합니다.

### 🖥️ 런타임

![Runtime](../assets/runtime.png)

런타임

### 두 개의 바이너리

| 바이너리  | 용도                               | 대상 코덱                     |
| --------- | ---------------------------------- | ----------------------------- |
| **`img`** | 정적 이미지 최적화 전용            | → JXL / 건너뛰기 / 무시       |
| **`vid`** | 동영상 및 애니메이션 미디어 최적화 | → HEVC / AV1 / GIF / 건너뛰기 |

여기에 드래그 앤 드롭 방식으로 배치 처리가 가능한 **macOS 앱**(`Modern Format Boost.app`)이 추가로 제공됩니다.

## 📉 실제 압축 사례

| 입력 형식       | 원본 크기 | 출력 형식      | 출력 크기 | 절감률   | 방법                       |
| :-------------- | :-------- | :------------- | :-------- | :------- | :------------------------- |
| 풍경 JPEG       | 4.2 MB    | **JXL**        | 3.3 MB    | **~21%** | 무손실 컴포넌트 재구축     |
| 스크린샷 PNG    | 2.5 MB    | **JXL**        | 1.1 MB    | **~56%** | Modular d=0.0              |
| 액션캠 H.264    | 1.2 GB    | **HEVC**       | 480 MB    | **~60%** | GPU/CPU CRF 검색           |
| 애니메이션 WebP | 15 MB     | **AV1 / HEVC** | 1.8 MB    | **~88%** | 동영상 형식으로 트랜스코딩 |

## 📊 처리 매트릭스

### 이미지 형식 결정 매트릭스

| 입력 형식                                  | 정적? | `img run`에서의 동작 | 출력      | 참고                                    |
| :----------------------------------------- | :---: | :------------------- | :-------- | :-------------------------------------- |
| JPEG                                       |  ✅   | **무손실 재구축**    | `.jxl`    | 비트 단위 일치 `cjxl --lossless_jpeg=1` |
| PNG / TIFF / BMP / 기타 무손실 정적 이미지 |  ✅   | **무손실 변환**      | `.jxl`    | 먼저 우회 경로를 사용할 수 있음         |
| WebP / AVIF / HEIC / HEIF (무손실 정적)    |  ✅   | **변환**             | `.jxl`    | 무손실 최신 정적 이미지는 허용됨        |
| 게인맵이 포함된 HEIC / HEIF                |  ✅   | **HDR 합성**         | `.jxl`    | 게인맵 경로가 리니어 HDR을 합성함       |
| 정적 검증 후의 레거시 손실 정적 이미지     |  ✅   | **거의 무손실 변환** | `.jxl`    | 현재 `img run` 배치 경로는 JXL 중심임   |
| 손실 압축된 WebP / AVIF / HEIC / HEIF 정적 |  ✅   | **건너뛰기**         | 원본 유지 | 세대 손실 방지                          |
| JXL 정적 이미지                            |  ✅   | **건너뛰기**         | 원본 유지 | 이미 최적화됨                           |
| 모든 애니메이션 또는 불분명한 이미지       |  ❌   | **무시**             | 없음      | `img` 정적 전용 도메인 외부             |

### `img` 라우팅 참고 사항

현재 저장소에는 두 개의 이미지 변환 진입점이 있습니다:

- `img run` / `crates/img/src/main.rs`의 배치 CLI 경로
- `crates/img/src/conversion_api.rs`의 라이브러리 헬퍼 `smart_convert()`

이들은 현재 **완전히 일치하지 않습니다**.

- 메인 CLI 경로는 현재 허용된 정적 변환에 대해 JXL 중심입니다.
- 오래된 헬퍼 API에는 일부 비 JPEG 손실 정적 이미지에 대한 AVIF 대상 브랜치가 여전히 포함되어 있습니다.
- `img` CLI도 `--codec`을 파싱하지만, 현재의 정적 배치 경로에서 해당 플래그는 실제 라우팅 결정에 실질적인 영향을 주지 않습니다.

이 README는 사용자가 일반적인 배치 사용 시 접하게 되는 **현재 CLI/런타임 동작을 우선적으로** 설명합니다.

### 애니메이션 미디어 결정 매트릭스

| 입력 형식                                           | 소유자                | 동작                 | 출력                     | 참고                              |
| :-------------------------------------------------- | :-------------------- | :------------------- | :----------------------- | :-------------------------------- |
| GIF                                                 | `vid`                 | **Loop-intent 경로** | `.gif` 또는 동영상       | GIF 빠른 경로 보존                |
| 애니메이션 WebP / AVIF / APNG / HEIC / HEIF / JXL   | `vid`                 | **Loop-intent 경로** | `.gif` / `.mov` / `.mp4` | `img`는 이를 무시함               |
| `--apple-compat`을 사용한 짧은 무음 최신 애니메이션 | `vid` + `loop_intent` | **GIF 강제**         | `.gif`                   | 기간 `<= 6초`                     |
| `--apple-compat`을 사용한 긴 최신 애니메이션        | `vid` + `loop_intent` | **GIF 강제 안 함**   | 동영상 대상              | 기간 `>= 15초`는 동영상 형태 유지 |
| `--apple-compat`을 사용한 불확실한 최신 애니메이션  | `vid` + `loop_intent` | **GIF 강제**         | `.gif`                   | 호환성을 위한 폴백                |

### 비디오 코덱 결정 매트릭스

| 입력 코덱                      | 일반 모드            | `--apple-compat` 모드 | 참고                                                  |
| :----------------------------- | :------------------- | :-------------------- | :---------------------------------------------------- |
| H.264 (AVC)                    | **변환**             | **변환**              | 두 모드 모두에서 사전 건너뛰기 안 됨                  |
| VP9                            | **건너뛰기**         | **HEVC로 변환**       | Apple 호환되지 않는 소스                              |
| AV1                            | **건너뛰기**         | **HEVC로 변환**       | Apple 호환되지 않는 소스                              |
| VVC / AV2                      | **건너뛰기**         | **HEVC로 변환**       | Apple 호환되지 않는 소스                              |
| HEVC (H.265)                   | **건너뛰기**         | **건너뛰기**          | 이미 Apple 네이티브 대상                              |
| ProRes / DNxHD / 레거시 코덱들 | **필요에 따라 변환** | **필요에 따라 변환**  | 최종 유지/건너뛰기는 여전히 최적화 결과에 따라 달라짐 |

라우팅 후에도 품질 및 크기 게이트가 여전히 적용됩니다. `--ultimate` 및 기타 품질 일치 흐름에서 변환 자격이 있는 경로는 생성된 파일이 품질/크기 요구 사항을 충족하지 못하고 허용된 최선(best-effort) 폴백이 적용되지 않는 경우 여전히 건너뛰기로 끝날 수 있습니다.

### HDR 형식 전략

| HDR 유형          | 감지                                    | 보존 전략                                                                             |
| :---------------- | :-------------------------------------- | :------------------------------------------------------------------------------------ |
| **HDR10**         | side_data의 mastering_display + max_cll | FFmpeg 인수를 통해 정적 메타데이터 완전 보존                                          |
| **HEIC 게인맵**   | HEIC 보조 이미지 (Apple/Samsung/ISO)    | 32비트 리니어 HDR로 합성 -> JXL (진정한 HDR)                                          |
| **UltraHDR JPEG** | JPEG APP1/APP2 + XMP (hdrgm:)           | 메타데이터 보존; 게인맵 손실 경고 발생                                                |
| **HLG**           | color_trc = arib-std-b67                | 색상 프라이머리 + TRC 보존                                                            |
| **Dolby Vision**  | 스트림/프레임 내 DOVI side_data         | `dovi_tool`을 통한 RPU 추출 → x265 주입; Profile 7 → 8.1 변환                         |
| **HDR10+**        | ST2094-40 동적 메타데이터               | `hdr10plus_tool` 사이드카 추출 및 x265 주입을 통해 지원 (Profile A/B 메타데이터 유지) |
| **SDR**           | HDR 마커 없음                           | 표준 처리 (yuv420p)                                                                   |

## ⬇️ 설치

### 사전 컴파일된 바이너리

Rust 툴체인을 설치하고 싶지 않은 사용자는
**[Releases](https://github.com/nowaytouse/modern-format-boost/releases)** 페이지에서
사전 컴파일된 바이너리를 다운로드할 수 있습니다.

```bash
# macOS/Linux 한 줄 명령어 (macOS ARM64 예시)
curl -LO https://github.com/nowaytouse/modern-format-boost/releases/latest/download/modern-format-boost-aarch64-apple-darwin.tar.gz
tar -xzf modern-format-boost-aarch64-apple-darwin.tar.gz

```

### 필수 조건

| 도구                 | 필수? | 용도                           | 설치 명령                                                                                   |
| :------------------- | :---: | :----------------------------- | :------------------------------------------------------------------------------------------ |
| **Rust** (nightly)   |  ✅   | 빌드 및 설치                   | `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \| sh && rustup default nightly` |
| **FFmpeg** (5.0+)    |  ✅   | 비디오 처리 및 메트릭 계산     | `brew install ffmpeg` / `apt install ffmpeg`                                                |
| **libjxl**           |  ✅   | JXL 인코딩 코어                | `brew install jpeg-xl`                                                                      |
| **ExifTool**         |  ✅   | 메타데이터 보존                | `brew install exiftool`                                                                     |
| **ImageMagick**      |  ✅   | 이미지 우회 경로               | `brew install imagemagick`                                                                  |
| **libwebp**          |  ✅   | WebP 네이티브 디코딩           | `brew install webp`                                                                         |
| **libheif**          |  ✅   | HEIC/HEIF 디코딩               | `brew install libheif`                                                                      |
| **PostgreSQL** (12+) |  ✅   | 캐시 및 품질 특성 데이터베이스 | `brew install postgresql pgvector` / `apt install postgresql`                               |
| **dovi_tool**        | 선택  | Dolby Vision RPU 추출          | `cargo install dovi_tool`                                                                   |
| **hdr10plus_tool**   | 선택  | HDR10+ 메타데이터 추출         | `cargo install hdr10plus_tool`                                                              |

#### macOS (Homebrew)

```bash
brew install ffmpeg jpeg-xl exiftool imagemagick webp libheif postgresql pgvector
```

#### Linux (Ubuntu/Debian)

```bash
sudo apt update && sudo apt install ffmpeg libimage-exiftool-perl imagemagick \
  webp libheif-dev postgresql postgresql-contrib postgresql-server-dev-all
# Linux에서는 pgvector 확장 기능을 컴파일하고 설치해야 합니다:
git clone --branch v0.5.1 https://github.com/pgvector/pgvector.git
cd pgvector
make && sudo make install
```

### 🗄️ 데이터베이스 설정

Modern Format Boost는 필수 로컬 캐시 및 품질 추론 엔진으로 PostgreSQL(`pgvector` 확장 기능 포함)을 사용합니다. `img` 및 `vid` 바이너리는 시작할 때 데이터베이스에 연결되며, 데이터베이스 서비스에 연결할 수 없으면 오류가 발생하고 프로그램이 종료됩니다.

#### 1. PostgreSQL 서비스 시작

- **macOS**: `brew services start postgresql`
- **Linux**: `sudo systemctl start postgresql`

#### 2. 데이터베이스 생성

기본 데이터베이스 이름은 `modern_format_boost`입니다. 도구를 실행하기 전에 데이터베이스를 생성하십시오:

```bash
createdb modern_format_boost
```

또는 SQL을 통해:

```sql
CREATE DATABASE modern_format_boost;
```

### 소스에서 빌드

```bash
git clone https://github.com/nowaytouse/modern-format-boost.git
cd modern-format-boost
cargo build --release

```

## 🚀 사용법

### 빠른 시작

```bash
# 이미지 경로 변환
img run /path/to/media
# 비디오 경로 변환
vid run /path/to/media

# AV1 전략을 사용하려면:
vid run --codec av1 /path/to/media
```

### ⚡ 고속 모드 및 스마트 재개

**고속 모드** (`fastmode`)는 드래그 앤 드롭 UI 워크플로 (`crates/dev/src/bin/drag_and_drop_processor.rs`)에 맞춰 조정되어 고신뢰성 재개 기능을 제공합니다.

- **`WorkingCopyMarker` 상태 관리**: 중단된 부분 처리 상태를 안전하게 추적합니다.
- **오래된 소스 파일 감지**: 원본 파일이 변경되었는지 자동으로 감지하고 새로운 빌드를 강제하여, 잘못된 재시도를 방지합니다.
- **Fail-Closed 보호**: 심층 컨텍스트 캡처 및 `Blake3` 검증으로 `img run` 중단 시에도 파일 손상이 절대 발생하지 않도록 보장합니다.

### 상세 옵션

- `--ultimate`: 아카이브 등급의 **0.01 정밀도** 검색 (고품질, 높은 시간 소요).
- `--apple-compat`: Apple 생태계 호환성(Live Photos/AAE)을 활성화합니다. CLI 기본값은 켜짐이며, `--no-apple-compat`으로 비활성화할 수 있습니다.
- `--in-place`: 원본 파일을 교체합니다. **경고: 되돌릴 수 없습니다.**
- `-o /dir`: 안전한 출력 디렉터리. (권장)
- `--verbose`: 상세 처리 로그를 표시합니다.
- `--no-recursive`: 하위 디렉터리로 들어가지 않습니다.
- `--force-video`: Loop Intent와 관계없이 애니메이션 이미지를 비디오로 강제 처리합니다.

### 고급 하위 명령

- `img cache-stats`: SQLite 분석 캐시 통계를 확인합니다.
- `vid strategy <file>`: 특정 파일에 대한 파이프라인 전략을 미리 봅니다.
- `img restore-timestamps`: 파일 이름 패턴을 기반으로 생성 날짜를 일괄 수정합니다(메타데이터 복구).

### 💡 다중 인스턴스 참고 사항

**Modern Format Boost**는 여러 창/인스턴스 실행을 네이티브하게 지원합니다.

- **병렬 처리**: 서로 다른 경로를 독립적으로 처리하기 위해 여러 창을 실행할 수 있습니다.
- **참고**: 하드웨어 I/O 성능에 따라 확장하십시오. 과도한 동시성은 파일 시스템 레이스 컨디션을 유발할 수 있습니다.

## 🏗️ 아키텍처

### CI/CD 및 테스트 게이트 시스템

Modern Format Boost는 코어 아키텍처의 기술 부채 제로를 보장하기 위해 엄격한 품질 게이트 시스템을 사용합니다.

- **Rust-first 개발 도구 체인**: 엔지니어링 entrypoint는 `crates/dev/src/bin` 아래의 Rust bin입니다. Python 원본은 안전 삭제가 확인될 때까지 호환성 참조로만 유지됩니다.
- **로컬 CI 검증**: 개발 전에는 항상 `just fix-gate` 또는 `cargo run --locked -p dev --bin check_all -- --allow-non-nightly`를 사용하여 검사해야 합니다. 이는 코드 포맷팅, 정적 분석 및 자동화된 테스트의 "단일 진실 공급원"(SSOT)입니다.
- **테스트 강화 및 안정성**: 여러 플랫폼에서 포괄적인 진단 정보를 수집하기 위해 "Fail Fast"를 비활성화했습니다. 또한 이미지(예: JPEG 복원 검증) 오류 상태에 대한 심층 컨텍스트 캡처를 추가했습니다.

### 핵심 구조

- `crates/img/`: 정적 이미지 최적화 도구 (현재 CLI 경로에서는 `JXL` / 건너뛰기 / 무시)
- `crates/vid/`: 동영상 및 애니메이션 미디어 최적화 도구 (`HEVC` / `AV1` / `GIF`)
- `crates/foundation/`: 핵심 두뇌 (GPU/CPU 하이브리드 엔진, HDR 매핑, 메타데이터)
- `Modern Format Boost.app/`: macOS 드래그 앤 드롭 UI

## ❓ FAQ

**1. JXL은 널리 지원되나요?**
macOS 14+ / iOS 17+, Chrome 91+, Firefox 128+에서 네이티브 지원됩니다. 그러나 다음과 같은 알려진 생태계 문제가 있습니다:

- **애니메이션**: 최신 애니메이션 형식(JXL/AV1/HEIF)은 특히 iCloud를 통해 동기화될 때 네이티브 macOS/iOS 사진 앱 또는 Finder에서 애니메이션으로 미리 보기에 실패(정적 이미지만 표시)하는 경우가 많습니다. 최신 브라우저나 전용 도구에서는 올바르게 재생됩니다.
- **썸네일**: **그레이스케일 ICC 프로파일**을 사용하는 JXL 파일은 열었을 때는 완벽하게 렌더링되지만, Finder/iCloud에서 **검은색 썸네일**로 나타날 수 있습니다.
  JXL은 비트 단위 일치 아카이브 및 고충실도 HDR 저장을 위한 우수한 형식으로 남아 있습니다.

**2. HDR10+는 어떻게 처리되나요?**
완벽하게 지원됩니다. `hdr10plus_tool`을 사용하여 SMPTE 2094-40 동적 메타데이터를 추출하고 `libx265`의 `--dhdr10-info` 매개변수를 통해 HEVC 스트림에 다시 주입합니다. 이 기능을 활성화하려면 도구가 설치되어 있는지 확인하십시오.

**3. 왜 WebP/AVIF/HEIC를 건너뛰나요?**
정적 손실 압축 WebP/AVIF/HEIC/HEIF는 이미 현대적인 손실 형식이며, 이를 다시 인코딩하면 작은 이점에 비해 세대 손실(generational loss)의 위험이 있으므로 일반적으로 건너뜁니다. 현재 코드에서 중요한 예외는 다음과 같습니다:

- 무손실 최신 정적 이미지는 여전히 JXL로 변환될 수 있습니다.
- HEIC/HEIF 게인맵 자산은 HDR JXL로 합성될 수 있습니다.
- 애니메이션된 최신 형식은 `img`에서 처리되지 않고 `vid` 및 `loop_intent`를 통해 라우팅됩니다.

---

## ⚖️ 라이선스

**MIT 라이선스** 하에 배포됩니다.

## 런타임 종속성

이 프로젝트는 여러 오픈 소스 거인들을 조율합니다. 그들의 기여에 감사드립니다:

| 구성 요소              | 라이선스   | 용도                     |
| ---------------------- | ---------- | ------------------------ |
| **FFmpeg**             | LGPL/GPL   | 비디오 처리 및 지표 측정 |
| **libjxl** (cjxl/djxl) | BSD-3      | JPEG XL 인코딩           |
| **ExifTool**           | Perl/GPL   | 메타데이터 보존          |
| **ImageMagick**        | Apache 2.0 | 이미지 우회 경로         |
| **SVT-AV1**            | BSD+Patent | AV1 인코딩               |
| **x265**               | GPL-2.0    | HEVC 인코딩              |

모든 Rust 종속성은 `Cargo.toml`을 통해 관리되며 각각의 오픈 소스 라이선스(MIT/Apache/BSD)를 따릅니다.
