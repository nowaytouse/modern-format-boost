<p align="center">
  <img src="https://img.shields.io/badge/versión-0.11.2-0969DA?style=for-the-badge&logo=rust&logoColor=white" alt="Versión">
  <img src="https://img.shields.io/badge/rust-edición_2021-E57324?style=for-the-badge&logo=rust&logoColor=white" alt="Rust">
  <img src="https://img.shields.io/badge/plataforma-macOS_%7C_Linux_%7C_Windows-8257E5?style=for-the-badge&logo=apple&logoColor=white" alt="Plataforma">
  <img src="https://img.shields.io/badge/licencia-MIT-00B265?style=for-the-badge" alt="Licencia">
</p>

<h1 align="center">Modern Format Boost</h1>

<p align="center">
  <strong>Motor de optimización de medios de próxima generación: cero pérdida de calidad, máxima compresión.</strong><br>
</p>

---

# 📖 Español (Spanish)

## ¿Qué es Modern Format Boost?

**Modern Format Boost** es un motor de optimización de medios de alto rendimiento basado en Rust. Convierte formatos de imagen y video heredados (JPEG, PNG, H.264, VP9...) en códecs de vanguardia (**JPEG XL** para imágenes, **HEVC/AV1** para videos), logrando reducciones drásticas del tamaño de archivo mientras preserva o incluso iguala exactamente (bit-exact) la calidad original.

Piénselo como un "compresor inteligente" que **nunca degrada sus archivos**:

- 📸 **Imágenes**: JPEG → reconstrucción sin pérdida JXL (exacta a nivel de bits, ~20% más pequeña); PNG/WebP/TIFF/HEIC → JXL.
- 🎬 **Videos**: H.264/VP9/AV1 → HEVC con búsqueda de calidad acelerada por GPU.
- 🍎 **Ecosistema Apple primero**: Modo de compatibilidad total con Apple, detección de Live Photos, manejo de archivos sidecar AAE.
- 🔒 **Guardián de metadatos**: Preserva EXIF, XMP, perfiles ICC, marcas de tiempo de creación, xattrs de macOS y etiquetas de Finder.
- ⚡ **Optimización de velocidad percibida**: Estrategia de clasificación "Deep-First": prioriza los niveles de directorio más profundos primero, luego clasifica por tamaño y formato de archivo para asegurar un procesamiento por lotes eficiente.
- 🎞️ **Metadatos dinámicos HDR10+**: Retención total de metadatos SMPTE 2094-40 mediante extracción de sidecars e inyección SEI x265.
- 🌅 **Síntesis de Gainmap HDR**: Sintetiza automáticamente búferes HDR lineales de 32 bits de alta fidelidad a partir de Gainmaps HEIC de Apple/Samsung/ISO, asegurando que se preserve el rango dinámico máximo al convertir a JXL.
- **🔍 Conocimiento de metadatos del fabricante**: Escaneo inteligente de espacios de nombres XMP específicos de Samsung/Google en archivos HEIC para asegurar la máxima preservación del contexto.

## ⚠️ Descargo de responsabilidad y notas importantes

1. **La seguridad de los datos es lo primero**: Para evitar cualquier posible pérdida de datos, se recomienda encarecidamente guardar los archivos procesados en un directorio separado (por ejemplo, usando `-o /ruta/al/output`) en lugar de usar la conversión en el lugar (`--in-place`), especialmente para medios irremplazables.
2. **Software Beta**: Aunque este programa ha sido probado, depurado y optimizado extensamente para evitar pérdidas de calidad o datos (como se ve en el historial de cambios), no se garantiza que esté 100% libre de errores. Informe cualquier problema que encuentre en GitHub.
3. **Perspectiva de computación**: Aunque está optimizado para la eficiencia (especialmente en Apple Silicon serie M), procesar lotes masivos en modo `--ultimate` puede llevar mucho tiempo y ocupar recursos del sistema por un período prolongado; planifique su tarea en consecuencia.
4. **Madurez de las herramientas**: Las herramientas basadas en HEVC (`img-hevc`, `vid-hevc`) son actualmente más maduras y estables que las basadas en AV1 (`img-av1`, `vid-av1`). Para tareas de producción de alta fiabilidad, se recomiendan las herramientas HEVC.

## 🔒 Privacidad e integridad de los datos

**Modern Format Boost** está construido sobre una arquitectura "Local-First", asegurando que sus activos creativos permanezcan bajo su control total.

- **Operación sin conexión**: Procesamiento 100% fuera de línea. Sin telemetría, seguimiento de uso ni conexiones a la nube. Los binarios principales no contienen código relacionado con la red.
- **Tiempo de ejecución reforzado con Rust**: Construido con Rust para eliminar de forma nativa errores de corrupción de memoria (desbordamientos de búfer, etc.).
- **Integración segura**: Todas las herramientas externas (FFmpeg, cjxl) se invocan a través de primitivas seguras y escapadas, nunca mediante ejecución directa en shell, evitando la inyección de comandos arbitrarios.
- **Aislamiento de rutas**: La normalización avanzada evita el recorrido de directorios y protege los archivos del sistema no relacionados.
- **Lista de bloqueo de rutas del sistema**: Escudos integrados para directorios sensibles del sistema para evitar modificaciones accidentales de archivos del SO.
- **Equilibrio dinámico de recursos**: Ajusta automáticamente los hilos de procesamiento según la carga de memoria/CPU para evitar fallos del sistema durante tareas extremas.
- **Custodio integral de metadatos**: Preservación estricta bit a bit de EXIF, XMP, ICC y marcas de tiempo del sistema de archivos (btime/mtime).
- **Procesamiento seguro y aislamiento de sesiones**:
  - **Cero contaminación del espacio de trabajo**: El seguimiento centralizado (`~/.mfb_progress/`) mantiene sus carpetas de medios 100% limpias. No quedan archivos de metadatos ocultos entre sus fotos/videos.
  - **Archivos temporales sin conflictos**: Cada archivo de análisis intermedio se identifica de forma única con un UUID aleatorio. Esto evita colisiones de múltiples instancias y asegura una "precisión quirúrgica" durante la limpieza.
  - **Limpieza al inicio**: Ya sea que una tarea se complete con éxito o se reanude después de una interrupción, el sistema purga automáticamente todos los datos transitorios. Esta arquitectura de "autolimpieza" asegura que su disco permanezca libre de residuos de procesamiento abandonados.
- **Reinicio inteligente de puntos de control**: Detecta automáticamente cuando un usuario elimina manualmente el directorio de salida para "empezar de nuevo", activando un reinicio completo del estado incluso en modo de reanudación.

<details>
<summary><b>🛠️ Técnico profundo: Cómo funciona — El flujo de trabajo</b></summary>

### Lógica del flujo de imágenes

Cada archivo pasa por un flujo de decisión de múltiples etapas:

- **Etapa 1 — Detección inteligente**: Analiza tablas DQT de JPEG (detección de gainmap UltraHDR), fragmentos VP8L de WebP y cajas `av1C` de AVIF a nivel binario. Ahora cuenta con una **arquitectura de deuda cero** con cumplimiento 100% de Clippy y análisis robusto de encabezados `OpenEXR`/`JPEG 2000`.
- **Etapa 2 — Ruta y codificación**: JXL VarDCT para JPEG (exacto a nivel de bits); modo modular para fuentes sin pérdida (PNG, WebP/AVIF/HEIC/EXR/JP2 sin pérdida).
- **Etapa 3 — Desvío**: Los formatos como TIFF/WebP/BMP/HEIC se preprocesan en PNG temporales de 16 bits o **OpenEXR de 32 bits** para asegurar la compatibilidad con `cjxl` sin pérdida de calidad.
- **Etapa 4 — Síntesis HDR de HEIC**: Intercepta archivos HEIC con Gainmaps (Apple/Google) y sintetiza búferes HDR de luz lineal de 32 bits a través de un flujo intermedio de **OpenEXR**, entregando una salida HDR JXL real.
- **Etapa 5 — Meme Score v3**: Evalúa GIFs animados (Nitidez 40%, Resolución 18%, Duración 20%) para decidir entre la conversión a video o mantenerlo como GIF.

### Flujo de video: Búsqueda de saturación en tres fases

1. **Fase 1: Búsqueda gruesa en GPU**: Búsqueda binaria en codificadores de hardware (VideoToolbox/NVENC) para encontrar el "punto de inflexión de calidad".
2. **Fase 2: Ajuste fino en CPU**: Mapea el CRF de la GPU a la escala de `x265`. Usa **Sprint & Backtrack** (paso doble al tener éxito, reinicio a 0.1 al excederse).
3. **Fase 3: Puerta de calidad 3D definitiva**: Requiere superar simultáneamente VMAF-Y ≥ 92.0, CAMBI ≤ 6.0 (banding) y PSNR-UV ≥ 34.0 dB.
   - **Puntuación de fusión**: Combina MS-SSIM + SSIM_All (peso 0.6/0.4) para un análisis estructural robusto.
   - **Protección de croma**: Detecta automáticamente resoluciones pequeñas que harían fallar el MS-SSIM de libvmaf y recurre a la puntuación solo en Y para asegurar la fiabilidad del procesamiento.
   - _Nota: En modo `--ultimate`, la búsqueda solo termina después de que **50 muestras consecutivas** muestren una ganancia de calidad nula, asegurando la saturación absoluta._

### Preservación de metadatos y HDR

- **HDR**: Preserva primarios bt2020, PQ/HLG TRC y metadatos de Mastering Display.
- **Dolby Vision**: Extrae RPU a través de `dovi_tool` e inyecta en x265 (conversión de Perfil 7 → 8.1).
- **macOS xattrs**: Preserva etiquetas de Finder, fecha de adición y marcas de tiempo de creación mediante `copyfile` y `setattrlist`.
</details>

### 🖥️ Tiempo de ejecución

![Tiempo de ejecución](../assets/runtime.png)

<p align="center">Tiempo de ejecución</p>

### Los cuatro binarios

| Binario        | Propósito                | Códec de destino                  |
| -------------- | ------------------------ | --------------------------------- |
| **`img-hevc`** | Optimización de imágenes | → JXL (estático) / HEVC (animado) |
| **`img-av1`**  | Optimización de imágenes | → JXL (estático) / AV1 (animado)  |
| **`vid-hevc`** | Optimización de video    | → HEVC / H.265                    |
| **`vid-av1`**  | Optimización de video    | → AV1 / SVT-AV1                   |

Además de una **aplicación macOS de doble clic** (`Modern Format Boost.app`) para el procesamiento por lotes mediante arrastrar y soltar.

## 📉 Ejemplos de compresión del mundo real

| Formato de entrada | Tamaño original | Formato de salida | Tamaño de salida | Ahorro   | Método                     |
| :----------------- | :-------------- | :---------------- | :--------------- | :------- | :------------------------- |
| Paisaje JPEG       | 4.2 MB          | **JXL**           | 3.3 MB           | **~21%** | Reconstrucción sin pérdida |
| Captura PNG        | 2.5 MB          | **JXL**           | 1.1 MB           | **~56%** | Modular d=0.0              |
| Action Cam H.264   | 1.2 GB          | **HEVC**          | 480 MB           | **~60%** | Búsqueda CRF GPU/CPU       |
| WebP animado       | 15 MB           | **AV1 / HEVC**    | 1.8 MB           | **~88%** | Transcodificado a video    |

## 📊 Matriz de procesamiento

### Matriz de decisión de formato de imagen

| Formato de entrada | ¿Sin pérdida? | ¿Animado? | Acción                         | Salida        | Método                                     |
| :----------------- | :-----------: | :-------: | :----------------------------- | :------------ | :----------------------------------------- |
| JPEG               |       —       |    No     | **Reconstrucción sin pérdida** | `.jxl`        | `cjxl` VarDCT (bit-exact)                  |
| PNG                |      ✅       |    No     | **Conversión sin pérdida**     | `.jxl`        | `cjxl` Modular `d=0.0`                     |
| PNG (indexado)     |      ❌       |    No     | **Calidad igualada**           | `.jxl`        | d=0.001                                    |
| WebP               |      ✅       |    No     | **Desvío → sin pérdida**       | `.jxl`        | dwebp → JXL d=0.0                          |
| WebP               |      ❌       |    No     | **Omitir**                     | (mantener)    | Evitar pérdida generacional                |
| WebP               |       —       |    ✅     | **Meme Score**                 | `.mov`/`.gif` | HEVC/AV1 o mantener GIF                    |
| AVIF               |      ✅       |    No     | **Conversión sin pérdida**     | `.jxl`        | d=0.0                                      |
| AVIF               |      ❌       |    No     | **Omitir**                     | (mantener)    | Evitar pérdida generacional                |
| HEIC/HEIF          |      ✅       |    No     | **Desvío → sin pérdida**       | `.jxl`        | `sips`/`magick` → PNG → d=0.0              |
| HEIC/HEIF          |      ❌       |    No     | **Síntesis HDR**               | `.jxl`        | Si existe Gainmap -> 32-bit EXR -> JXL     |
| HEIC/HEIF          |      ❌       |    No     | **Omitir**                     | (mantener)    | HEIC estándar: evitar pérdida generacional |
| TIFF               |      ✅       |    No     | **Desvío → sin pérdida**       | `.jxl`        | `magick -depth 16` → PNG → d=0.0           |
| TIFF               |      ❌       |    No     | **Calidad igualada**           | `.jxl`        | magick → JXL d=0.001                       |
| BMP                |      ✅       |    No     | **Desvío → sin pérdida**       | `.jxl`        | `magick` → PNG → d=0.0                     |
| GIF                |       —       |    ✅     | **Meme Score**                 | `.mov`/`.gif` | HEVC/AV1 o mantener GIF                    |
| GIF                |       —       |    No     | **Extracción de fotogramas**   | `.jxl`        | ffmpeg → JXL                               |
| JXL                |       —       |    No     | **Omitir**                     | (mantener)    | Ya es óptimo                               |

### Matriz de decisión de códec de video

| Códec de entrada |   Compresión    | Acción                          | Salida        | Codificador               |
| :--------------- | :-------------: | :------------------------------ | :------------ | :------------------------ |
| H.264 (AVC)      |   Con pérdida   | **Exploración CRF**             | `.mp4` HEVC   | GPU → x265/SVT-AV1        |
| H.264            |   Sin pérdida   | **Codificación sin pérdida**    | `.mkv` HEVC   | x265/SVT-AV1 sin pérdida  |
| VP9              |   Con pérdida   | **Exploración CRF**             | `.mp4` HEVC   | GPU → x265/SVT-AV1        |
| AV1              |   Con pérdida   | **Exploración CRF**             | `.mp4` HEVC   | GPU → x265/SVT-AV1        |
| HEVC (H.265)     |   Cualquiera    | **Omitir**                      | (mantener)    | Ya es el códec de destino |
| ProRes           | Con/Sin pérdida | **Exploración CRF/sin pérdida** | `.mp4`/`.mkv` | x265                      |

## ⬇️ Instalación

### Binarios precompilados

Para los usuarios que no deseen instalar las herramientas de Rust, pueden descargar los binarios precompilados desde la página de **[Releases](https://github.com/nowaytouse/modern-format-boost/releases)**.

```bash
# Comando para macOS/Linux (ejemplo para macOS ARM64)
curl -LO https://github.com/nowaytouse/modern-format-boost/releases/latest/download/modern-format-boost-aarch64-apple-darwin.tar.gz
tar -xzf modern-format-boost-aarch64-apple-darwin.tar.gz
```

### Requisitos previos

| Herramienta        | ¿Requerida? | Propósito                  | Comando de instalación                                     |
| ------------------ | :---------: | -------------------------- | ---------------------------------------------------------- | --- |
| **Rust** (1.75+)   |     ✅      | Compilación e instalación  | `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh` |
| **FFmpeg** (5.0+)  |     ✅      | Procesamiento de video     | `brew install ffmpeg`                                      |
| **libjxl**         |     ✅      | Núcleo de codificación JXL | `brew install jpeg-xl`                                     |
| **ExifTool**       |     ✅      | Preservación de metadatos  | `brew install exiftool`                                    |
| **ImageMagick**    |     ✅      | Conversión de formatos     | `brew install imagemagick`                                 |
| **libwebp**        |     ✅      | Decodificación WebP        | `brew install webp`                                        |
| **dovi_tool**      |     ✅      | Extracción de Dolby Vision | `cargo install dovi_tool`                                  |
| **libheif**        |     ✅      | Decodificación HEIC/HEIF   | `brew install libheif`                                     |
| **hdr10plus_tool** |     ✅      | Extracción de HDR10+       | `cargo install hdr10plus_tool`                             |

## 🚀 Uso

### Inicio rápido

```bash
# Conversión de ruta de imágenes
img-hevc run /ruta/a/los/medios
# Conversión de ruta de videos
vid-hevc run /ruta/a/los/medios
```

---

# ⚖️ Licencia

Licenciado bajo la **Licencia MIT**.
