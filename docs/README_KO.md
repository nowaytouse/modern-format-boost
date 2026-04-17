# Modern Format Boost (Korean)

<p align="center">
  <img src="https://img.shields.io/badge/version-0.11.2-0969DA?style=for-the-badge&logo=rust&logoColor=white" alt="버전">
  <img src="https://img.shields.io/badge/rust-2021_edition-E57324?style=for-the-badge&logo=rust&logoColor=white" alt="Rust">
  <img src="https://img.shields.io/badge/platform-macOS_%7C_Linux_%7C_Windows-8257E5?style=for-the-badge&logo=apple&logoColor=white" alt="플랫폼">
  <img src="https://img.shields.io/badge/license-MIT-00B265?style=for-the-badge" alt="라이선스">
</p>

<p align="center">
  <strong>차세대 미디어 최적화 엔진 — 화질 저하 없는 최대 압축 구현.</strong><br>
</p>

---

## 📖 한국어 (Korean)

## Modern Format Boost란 무엇인가요?

**Modern Format Boost**는 Rust 기반의 고성능 미디어 최적화 엔진입니다. 기존의 이미지 및 비디오 형식(JPEG, PNG, H.264, VP9…)을 최신 코덱(이미지는 **JPEG XL**, 비디오는 **HEVC/AV1**)으로 변환합니다. 원본 화질을 그대로 유지하거나 비트 단위로 일치(bit-exact)시키면서 파일 크기를 획기적으로 줄여줍니다.

미디어를 **절대 열화시키지 않는 "스마트 압축기"**라고 생각하시면 됩니다.

- 📸 **이미지**: JPEG → JXL 무손실 재구축(비트 일치, 약 20% 절감). PNG/WebP/TIFF/HEIC → JXL.
- 🎬 **비디오**: H.264/VP9/AV1 → HEVC(GPU 가속 품질 탐색).
- 🍎 **Apple 생태계 우선**: Apple 완전 호환 모드, Live Photo 감지, AAE 사이드카 처리.
- 🔒 **메타데이터 보호**: EXIF, XMP, ICC 프로필, 생성 시간, macOS 확장 속성(xattrs), Finder 태그를 완벽하게 보존.
- ⚡ **체감 속도 최적화**: "Deep-First" 정렬 전략 — 깊은 계층의 디렉토리를 우선 처리하고 파일 크기와 형식별로 정렬하여 효율적인 배치 작업과 최대 처리량을 보장합니다.
- 🎞️ **HDR10+ 동적 메타데이터**: 사이드카 추출 및 x265 SEI 주입을 통해 SMPTE 2094-40 메타데이터를 완벽하게 유지.
- 🌅 **HDR 게인맵 합성**: Apple/Samsung/ISO HEIC 게인맵으로부터 고정밀 32비트 선형 HDR 버퍼를 자동 합성하여 JXL 변환 시 최대 동적 범위를 보존합니다.
- **🔍 벤더 메타데이터 인식**: HEIC 파일 내 Samsung/Google 전용 XMP 네임스페이스를 지능적으로 스캔하여 컨텍스트를 최대한 보존합니다.

## ⚠️ 주의사항 및 중요 안내

1. **데이터 안전 우선**: 데이터 손실을 방지하기 위해, 특히 소중한 미디어의 경우 덮어쓰기 변환(`--in-place`)보다는 별도의 디렉토리에 출력(`-o /path/to/output`)하는 것을 강력히 권장합니다.
2. **베타 소프트웨어**: 본 프로그램은 화질이나 데이터 손실을 방지하기 위해 광범위하게 테스트되고 최적화되었지만, 100% 버그가 없음을 보장하지는 않습니다. 문제가 발생하면 GitHub에 보고해 주세요.
3. **컴퓨팅 자원 안내**: Apple Silicon M 시리즈 등에서 효율화되었지만, `--ultimate` 모드에서의 대규모 배치 작업은 시간이 오래 걸리고 시스템 자원을 장기간 점유할 수 있습니다. 계획적으로 실행해 주세요.
4. **도구 성숙도**: 통합된 도구(`img`, `vid`)는 기본적으로 HEVC 전략을 사용하며, 이는 현재 AV1 전략보다 더 성숙하고 안정적입니다. 높은 신뢰성이 요구되는 작업에는 HEVC 전략(기본값)을 권장합니다.

## 🔒 개인정보 및 데이터 무결성

**Modern Format Boost**는 "로컬 우선" 아키텍처로 구축되어 창의적인 자산이 전적으로 사용자의 통제 하에 있도록 보장합니다.

- **오프라인 동작**: 100% 오프라인 처리. 텔레메트리, 사용량 추적, 클라우드 핑이 전혀 없습니다.
- **Rust 기반 보안**: 메모리 오염 버그(버퍼 오버플로 등)를 네이티브하게 제거하는 Rust로 구축되었습니다.
- **안전한 통합**: FFmpeg, cjxl 등 모든 외부 도구는 직접적인 쉘 실행이 아닌 안전한 프리미티브를 통해 호출되어 명령 주입을 방지합니다.
- **경로 격리**: 고급 정규화를 통해 디렉토리 트래버설을 방지하고 무관한 시스템 파일을 보호합니다.
- **시스템 경로 차단 목록**: 시스템 중요 디렉토리에 대한 실수에 의한 변경을 방지하는 실드 기능 내장.
- **동적 리소스 균형**: 메모리/CPU 부하에 따라 처리 스레드를 자동 조정하여 과도한 작업 시 시스템 충돌을 방지합니다.
- **포괄적 메타데이터 관리**: EXIF, XMP, ICC, 파일 시스템 타임스탬프(btime/mtime)를 비트 단위로 엄격히 보존.

## 🛠️ 기술적 세부 사항: 작동 원리

### 이미지 파이프라인

각 파일은 다단계 결정 파이프라인을 통과합니다.

- **1단계 — 스마트 감지**: JPEG DQT 테이블(UltraHDR 게인맵 감지), WebP VP8L 청크, AVIF `av1C` 박스를 바이너리 레벨에서 분석합니다.
- **2단계 — 경로 및 인코딩**: JPEG는 JXL VarDCT(비트 일치), 무손실 소스(PNG 등)는 Modular 모드를 사용합니다.
- **3단계 — 우회 경로**: TIFF/BMP/HEIC 등은 화질 손실 방지를 위해 임시로 16비트 PNG 또는 32비트 OpenEXR로 변환되어 처리됩니다.
- **4단계 — HEIC HDR 합성**: 게인맵(Apple/Google)이 포함된 HEIC 파일을 가로채어 중간 **OpenEXR** 파이프라인을 통해 32비트 선형광 HDR 버퍼를 합성하여 진정한 HDR JXL 출력을 제공합니다.
- **5단계 — 루프 의도 (Loop Intent v3)**: 최신 7계층 계층적 의사결정 나무 모델을 채택했습니다. **Loop Closure (루프 폐쇄도)**, **Motion Gini (운동 변동성)**, **주기성 분석** 및 **KNN 가중치 융합**을 종합적으로 평가하여 이미지나 비디오의 루프 의도(밈, 스티커, 루프 소재)를 지능적으로 식별합니다.

### 비디오 파이프라인: 3단계 포화 탐색

1. **1단계: GPU 대략 탐색**: 하드웨어 인코더(VideoToolbox/NVENC)를 통한 이진 탐색으로 "품질의 굴곡점"을 찾습니다.
2. **2단계: CPU 미세 조정**: GPU CRF를 `x265` 스케일에 매핑합니다. Sprint & Backtrack 전략을 사용합니다.
3. **3단계: 궁극의 3D 품질 게이트**: VMAF-Y ≥ 92.0, CAMBI ≤ 6.0, PSNR-UV ≥ 34.0 dB를 동시에 통과해야 합니다.
   - **퓨전 스코어링**: MS-SSIM + SSIM_All을 결합하여 견고한 구조 분석을 수행합니다.
   - _참고: `--ultimate` 모드에서는 50개 샘플 연속으로 품질 향상이 없을 때만 탐색을 종료하여 절대적인 포화를 보장합니다._

### 🖥️ 실행 화면

![Runtime](../assets/runtime.png)

### 2개의 통합 도구

| 도구      | 용도          | 타겟 코덱                                  |
| --------- | ------------- | ------------------------------------------ |
| **`img`** | 이미지 최적화 | → JXL (정지화상) / HEVC / AV1 (애니메이션) |
| **`vid`** | 비디오 최적화 | → HEVC / AV1                               |

또한 드래그 앤 드롭으로 배치 처리가 가능한 **macOS 앱**(`Modern Format Boost.app`)이 제공됩니다.

## 📉 실제 압축 예시

| 입력 형식    | 원본 크기 | 출력 형식 | 출력 크기 | 절감률   | 방법                   |
| :----------- | :-------- | :-------- | :-------- | :------- | :--------------------- |
| 풍경 JPEG    | 4.2 MB    | **JXL**   | 3.3 MB    | **~21%** | 무손실 컴포넌트 재구축 |
| 스크린샷 PNG | 2.5 MB    | **JXL**   | 1.1 MB    | **~56%** | Modular d=0.0          |
| 액션캠 H.264 | 1.2 GB    | **HEVC**  | 480 MB    | **~60%** | GPU/CPU CRF 탐색       |

## ⬇️ 설치 방법

### 빌드된 바이너리 다운로드

Rust 환경 구축을 원치 않는 사용자는 **[Releases](https://github.com/nowaytouse/modern-format-boost/releases)** 페이지에서 빌드된 바이너리를 다운로드할 수 있습니다.

```bash
# macOS/Linux용 (예: macOS ARM64)
curl -LO https://github.com/nowaytouse/modern-format-boost/releases/latest/download/modern-format-boost-aarch64-apple-darwin.tar.gz
tar -xzf modern-format-boost-aarch64-apple-darwin.tar.gz
```

### 필수 조건

| 도구               | 필수? | 용도                   | 설치 명령                                                         |
| :----------------- | :---: | :--------------------- | :---------------------------------------------------------------- |
| **Rust** (1.75+)   |  ✅   | 빌드 및 설치           | `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \| sh` |
| **FFmpeg** (5.0+)  |  ✅   | 비디오 처리 및 메트릭  | `brew install ffmpeg` / `apt install ffmpeg`                      |
| **libjxl**         |  ✅   | JXL 인코딩 코어        | `brew install jpeg-xl`                                            |
| **ExifTool**       |  ✅   | 메타데이터 보존        | `brew install exiftool`                                           |
| **ImageMagick**    |  ✅   | 이미지 우회 경로       | `brew install imagemagick`                                        |
| **libwebp**        |  ✅   | WebP 네이티브 디코딩   | `brew install webp`                                               |
| **dovi_tool**      |  ✅   | Dolby Vision RPU 추출  | `cargo install dovi_tool`                                         |
| **libheif**        |  ✅   | HEIC/HEIF 디코드       | `brew install libheif`                                            |
| **hdr10plus_tool** |  ✅   | HDR10+ 메타데이터 추출 | `cargo install hdr10plus_tool`                                    |

---

## ❓ FAQ

**1. JXL 형식의 현재 호환성은 어떻습니까?**  
macOS 14 (Sonoma) / iOS 17+, Chrome 91+, Firefox 128+에서 네이티브 지원을 제공합니다. 하지만 Apple 생태계에는 알려진 제한 사항이 있습니다:

- **애니메이션 미리보기**: JXL/AV1/HEIF와 같은 최신 애니메이션 형식은 macOS/iOS 기본 사진 앱이나 Finder에서 애니메이션으로 재생되지 않고 정지화면으로 표시되는 경우가 많으며, 특히 iCloud 동기화 후에 두드러집니다. 미리보기에는 명령줄 도구나 최신 브라우저를 사용하는 권장합니다.
- **썸네일 검은 화면**: JXL 파일이 **그레이스케일(Grayscale) ICC 프로필**을 사용하는 경우 Finder/iCloud 썸네일이 검게 표시될 수 있으나, 이는 파일 자체의 문제는 아니며 브라우저 등에서 열면 정상적으로 표시됩니다.
  JXL은 비트 정확도의 무손실 아카이빙 및 고충실도 HDR 저장에 있어 여전히 최상의 선택입니다.

**2. HDR10+ 동적 메타데이터는 어떻게 처리됩니까?**  
완벽하게 지원됩니다. `hdr10plus_tool`을 사용하여 SMPTE 2094-40 동적 메타데이터를 추출하고 `libx265`의 `--dhdr10-info` 파라미터를 통해 HEVC 스트림에 주입합니다. 이 기능을 사용하려면 도구가 설치되어 있는지 확인하십시오.

**3. 왜 WebP / AVIF / HEIC 등의 최신 형식은 건너뛰나요?**  
이러한 형식은 이미 현대적인 손실 압축이 적용되어 있습니다. 다시 인코딩하면 화질의 세대 손실(Generational Loss)이 발생하므로 프로그램은 품질 보호를 위해 기본적으로 이를 건너뜁니다.
**예외**: Apple/Google의 고충실도 **HDR Gainmap**이 감지되면 JXL로 합성 출력됩니다. 또한 애니메이션 파일이 **루프 의도 (Loop Intent)** 최적화 메커니즘을 트리거하는 경우에도 적절히 처리됩니다.

---

## ⚖️ 라이선스

**MIT 라이선스** 하에 제공됩니다.
