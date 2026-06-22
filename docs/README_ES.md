# Modern Format Boost

![Version](https://img.shields.io/badge/version-0.11.3-0969DA?style=for-the-badge&logo=rust&logoColor=white)
![Rust](<https://img.shields.io/badge/rust-2024_edition_(nightly)-E57324?style=for-the-badge&logo=rust&logoColor=white>)
![Platform](https://img.shields.io/badge/platform-macOS_%7C_Linux_%7C_Windows-8257E5?style=for-the-badge&logo=apple&logoColor=white)
![License](https://img.shields.io/badge/license-MIT-00B265?style=for-the-badge)

**Motor de optimización de medios de próxima generación — cero pérdida de calidad, máxima compresión.**

[English](../README.md) · [简体中文](README_ZH.md) · [繁體中文](README_ZH_TW.md) · [日本語](README_JA.md) · [한국어](README_KO.md) · [Español](README_ES.md) · [Français](README_FR.md) · [Português](README_PT.md) · [Русский](README_RU.md) · [العربية](README_AR.md)

## ¿Qué es Modern Format Boost?

**Modern Format Boost** es un motor de optimización de medios de alto rendimiento basado en Rust. Divide el trabajo por dominio de medios:

- `img` maneja **solo imágenes estáticas**
- `vid` maneja **videos y medios animados**

En la implementación actual, las rutas típicas son:

- 📸 **Imágenes estáticas (ruta CLI principal `img run`)**: Reconstrucción sin pérdida de JPEG → JXL; PNG/TIFF/BMP y otras imágenes estáticas sin pérdida → JXL; las imágenes estáticas modernas con pérdida suelen omitirse; las entradas animadas o con ambigüedad de animación se ignoran.
- 🎬 **Videos**: H.264 y otros códecs que no son el objetivo pasan por una búsqueda de calidad HEVC/AV1; la elección del códec/contenedor depende de `--codec` y `--apple-compat`.
- 🎞️ **Medios animados**: El enrutamiento de animaciones GIF/WebP/AVIF/APNG/HEIC/HEIF/JXL es propiedad de `vid` más la política compartida `loop_intent`.

Piense en ello como un optimizador conservador que prefiere resultados honestos de omitir/ignorar en lugar de daños silenciosos a la calidad:

- 🍎 **Ecosistema Apple primero**: Modo de compatibilidad total con Apple, detección de Live Photo, manejo de archivos sidecar AAE.
- 🔒 **Guardián de metadatos**: Preserva EXIF, XMP, perfiles ICC, marcas de tiempo de creación, xattrs de macOS, etiquetas de Finder.
- ⚡ **Optimización de velocidad percibida**: Estrategia de clasificación "Deep-First": prioriza primero los niveles de directorio más profundos, luego clasifica por tamaño de archivo y formato para garantizar un procesamiento por lotes eficiente y el máximo rendimiento.
- 🎞️ **Metadatos dinámicos HDR10+**: Retención completa de metadatos SMPTE 2094-40 mediante extracción de archivos sidecar e inyección SEI x265.
- 🌅 **Síntesis de Gainmap HDR**: Sintetiza automáticamente búferes HDR lineales de 32 bits de alta fidelidad a partir de Gainmaps HEIC de Apple/Samsung/ISO, garantizando que se preserve el rango dinámico máximo al convertir a JXL.
- **🔍 Conocimiento de metadatos del proveedor**: Escaneo inteligente de espacios de nombres XMP específicos de Samsung/Google en archivos HEIC para garantizar la máxima preservación del contexto.

## ⚠️ Descargo de responsabilidad y notas importantes

1. **La seguridad de los datos es lo primero**: Para evitar cualquier posible pérdida de datos, se recomienda encarecidamente enviar los archivos procesados a un directorio separado (por ejemplo, usando `-o /ruta/al/destino`) en lugar de usar la conversión en el lugar (`--in-place`), especialmente para medios irremplazables.
2. **Software Beta**: Si bien este programa ha sido ampliamente probado, depurado y optimizado para evitar la pérdida de calidad o datos (como se ve en el registro de cambios), no se garantiza que esté 100% libre de errores. Informe cualquier problema que encuentre en GitHub.
3. **Información sobre el procesamiento**: Aunque está optimizado para la eficiencia (especialmente en Apple Silicon serie M), procesar lotes masivos en modo `--ultimate` aún puede llevar mucho tiempo. Ocupará recursos del sistema durante un período prolongado; planifique su tarea en consecuencia.
4. **Madurez de la herramienta**: Las herramientas unificadas (`img`, `vid`) usan HEVC de forma predeterminada, que es más maduro y estable que la estrategia AV1. Para tareas de producción de alta confiabilidad, se recomienda HEVC (el valor predeterminado).

## 🔒 Privacidad e Integridad de Datos

**Modern Format Boost** está construido sobre una arquitectura "Local-First", lo que garantiza que sus activos creativos permanezcan completamente bajo su control.

- **Operación sin conexión (Air-Gapped)**: Procesamiento 100% fuera de línea. Sin telemetría, seguimiento de uso o llamadas a la nube. Los binarios principales no contienen código relacionado con la red.
- **Tiempo de ejecución reforzado con Rust**: Construido con Rust para eliminar de forma nativa los errores de corrupción de memoria (desbordamientos de búfer, etc.).
- **Integración segura**: Todas las herramientas externas (FFmpeg, cjxl) se invocan a través de primitivas seguras y escapadas —nunca a través de la ejecución directa de shell— evitando la inyección de comandos arbitrarios.
- **Aislamiento de rutas**: La normalización avanzada evita el recorrido de directorios y protege los archivos del sistema no relacionados.
- **Lista de bloqueo de rutas del sistema**: Escudos integrados para directorios sensibles del sistema para evitar modificaciones accidentales de archivos del sistema operativo.
- **Equilibrio dinámico de recursos**: Ajusta automáticamente los hilos de procesamiento según la carga de memoria/CPU para evitar fallos del sistema durante tareas extremas.
- **Custodio integral de metadatos**: Preservación estricta bit a bit de EXIF, XMP, ICC y marcas de tiempo del sistema de archivos (btime/mtime).
- **Procesamiento seguro y aislamiento de sesiones**:
  - **Cero contaminación del espacio de trabajo**: El seguimiento centralizado (`~/.mfb_progress/`) mantiene sus carpetas de medios 100% limpias. No quedan archivos de metadatos ocultos entre sus fotos/videos.
  - **Archivos temporales sin conflictos**: Cada archivo de análisis intermedio (flujos YUV, segmentos de análisis) se identifica de forma única con un UUID aleatorio. Esto evita colisiones entre múltiples instancias y garantiza una "Precisión quirúrgica" durante la limpieza.
  - **Limpieza al iniciar (Scrub-on-Start)**: Ya sea que una tarea se complete con éxito o se reanude después de una interrupción, el sistema purga automáticamente todos los datos transitorios. Esta arquitectura de "Auto-limpieza" garantiza que su disco permanezca libre de restos de procesamiento abandonados.
  - **Restablecimiento inteligente de puntos de control**: Detecta automáticamente cuando un usuario elimina manualmente el directorio de salida para "empezar de nuevo", activando un restablecimiento completo del estado incluso en modo de reanudación.

## 🛠️ Aspectos Técnicos Profundos: Cómo Funciona — El Flujo de Trabajo (Pipeline)

### Lógica del Flujo de Trabajo de Imágenes

Cada archivo pasa por un flujo de decisiones de múltiples etapas:

- **Etapa 1 — Detección inteligente**: Analiza las tablas DQT de JPEG (detección de gainmap UltraHDR), fragmentos VP8L de WebP y cajas `av1C` de AVIF a nivel binario. Ahora cuenta con **Zero-Debt Architecture** con 100% de cumplimiento de Clippy y un análisis robusto de encabezados `OpenEXR`/`JPEG 2000`.
- **Etapa 2 — Ruta y Codificación**: JXL VarDCT para JPEG (exacto en bits); modo Modular para fuentes sin pérdida (PNG, WebP/AVIF/HEIC/EXR/JP2 sin pérdida).
- **Etapa 3 — Ruta de desvío (Detour Pathway)**: Los formatos como TIFF/WebP/BMP/HEIC se preprocesan en PNG temporales de 16 bits u **OpenEXR de 32 bits** para garantizar la compatibilidad con `cjxl` sin pérdida de calidad (flujo de trabajo adaptado a 8/16/32 bits).
- **Etapa 4 — Síntesis HEIC HDR**: Intercepta archivos HEIC con Gainmaps (Apple/Google) y sintetiza búferes HDR de luz lineal de 32 bits a través de un flujo intermedio de acompañamiento **OpenEXR**, entregando una salida JXL HDR real.
- **Etapa 5 — División Estática/Animada**: `img` ahora rechaza estrictamente los activos animados o con ambigüedad de animación. Los formatos modernos animados se delegan a `vid` en lugar de convertirse dentro del flujo estático.
- **Etapa 6 — Loop Intent v3**: La lógica compartida de loop-intent decide si los medios animados deben permanecer como GIF o proceder a través del flujo de video. La política de entrega de animaciones modernas compatibles con Apple se centraliza aquí.

### Flujo de Trabajo de Video: Búsqueda de Saturación en Tres Fases

1. **Fase 1: Búsqueda gruesa por GPU**: Búsqueda binaria en codificadores de hardware (VideoToolbox/NVENC) para encontrar el "punto de inflexión de calidad".
2. **Fase 2: Ajuste fino por CPU**: Mapea el CRF de la GPU a la escala `x265`. Utiliza **Sprint & Backtrack** (paso doble en caso de éxito, restablecimiento a 0.1 en caso de exceso).
3. **Fase 3: Puerta de calidad 3D definitiva**: Requiere el paso simultáneo de VMAF-Y ≥ 86.0 (piso de cordura, relativo a la línea base dinámica), CAMBI ≤ 6.0 (banding) y PSNR-UV ≥ 30.0 dB (piso de cordura de croma).
   - **Puntuación de fusión**: Combina MS-SSIM + SSIM_All (peso 0.6/0.4) para un análisis estructural robusto.
   - **Chroma Guard**: Detecta automáticamente resoluciones pequeñas que harían fallar libvmaf MS-SSIM y recurre a la puntuación solo en Y para garantizar la confiabilidad del procesamiento.
   - _Nota: En modo `--ultimate`, la búsqueda solo termina después de que **50 muestras consecutivas** muestren cero ganancia de calidad, garantizando la saturación absoluta._

### Preservación de Metadatos y HDR

- **HDR**: Preserva las primarias bt2020, TRC PQ/HLG y los metadatos de la pantalla de masterización.
- **Dolby Vision**: Extrae RPU a través de `dovi_tool` e inyecta en x265 (conversión de Perfil 7 → 8.1).
- **xattrs de macOS**: Preserva etiquetas de Finder, fecha de adición y marcas de tiempo de creación a través de `copyfile` y `setattrlist`.

### 🖥️ Tiempo de ejecución

![Runtime](../assets/runtime.png)

Tiempo de ejecución

### Los dos binarios

| Binario   | Propósito                               | Códec objetivo              |
| --------- | --------------------------------------- | --------------------------- |
| **`img`** | Solo optimización de imágenes estáticas | → JXL / omitir / ignorar    |
| **`vid`** | Optimización de video y medios animados | → HEVC / AV1 / GIF / omitir |

Además de una **aplicación de macOS de doble clic** (`Modern Format Boost.app`) para el procesamiento por lotes mediante arrastrar y soltar.

## 📉 Ejemplos de compresión en el mundo real

| Formato de entrada  | Tamaño original | Formato de salida | Tamaño de salida | Ahorro   | Método                                    |
| :------------------ | :-------------- | :---------------- | :--------------- | :------- | :---------------------------------------- |
| JPEG de paisaje     | 4.2 MB          | **JXL**           | 3.3 MB           | **~21%** | Reconstrucción de componentes sin pérdida |
| PNG de captura      | 2.5 MB          | **JXL**           | 1.1 MB           | **~56%** | Modular d=0.0                             |
| H.264 de Action Cam | 1.2 GB          | **HEVC**          | 480 MB           | **~60%** | Búsqueda CRF por GPU/CPU                  |
| WebP animado        | 15 MB           | **AV1 / HEVC**    | 1.8 MB           | **~88%** | Transcodificado a formato de video        |

## 📊 Matriz de procesamiento

### Matriz de decisión de formato de imagen

| Formato de entrada                               | ¿Estático? | Acción en `img run`             | Salida        | Notas                                               |
| :----------------------------------------------- | :--------: | :------------------------------ | :------------ | :-------------------------------------------------- |
| JPEG                                             |     ✅     | **Reconstrucción sin pérdida**  | `.jxl`        | Exacto en bits `cjxl --lossless_jpeg=1`             |
| PNG / TIFF / BMP / otras imágenes sin pérdida    |     ✅     | **Conversión sin pérdida**      | `.jxl`        | Puede usar la ruta de desvío primero                |
| WebP / AVIF / HEIC / HEIF (estática sin pérdida) |     ✅     | **Convertir**                   | `.jxl`        | Se permiten imágenes estáticas modernas sin pérdida |
| HEIC / HEIF con Gainmap                          |     ✅     | **Síntesis HDR**                | `.jxl`        | La ruta Gainmap sintetiza HDR lineal                |
| Imágenes legadas con pérdida tras validación     |     ✅     | **Conversión casi sin pérdida** | `.jxl`        | La ruta actual de lotes se centra en JXL            |
| WebP / AVIF / HEIC / HEIF con pérdida            |     ✅     | **Omitir**                      | mantiene orig | Evitar pérdida generacional                         |
| JXL estático                                     |     ✅     | **Omitir**                      | mantiene orig | Ya es óptimo                                        |
| Cualquier imagen animada o ambigua               |     ❌     | **Ignorar**                     | ninguna       | Fuera del dominio de solo estática de `img`         |

### Nota sobre el enrutamiento de `img`

Existen dos puntos de entrada para la conversión de imágenes en el repositorio hoy en día:

- `img run` / ruta CLI por lotes en `crates/img/src/main.rs`
- ayudante de biblioteca `smart_convert()` en `crates/img/src/conversion_api.rs`

**No están completamente alineados** en este momento.

- La ruta CLI principal está actualmente orientada a JXL para conversiones estáticas aceptadas.
- El antiguo ayudante de la API aún contiene una rama orientada a AVIF para algunas imágenes estáticas sin pérdida que no son JPEG.
- El CLI de `img` también analiza `--codec`, pero en la ruta de lotes estáticos actual ese flag **no** cambia materialmente las decisiones de enrutamiento reales.

Este README documenta primero el **comportamiento actual del CLI/tiempo de ejecución**, porque eso es lo que los usuarios encuentran en el uso normal por lotes.

### Matriz de decisión de medios animados

| Formato de entrada                                      | Propietario           | Acción               | Salida                   | Notas                                  |
| :------------------------------------------------------ | :-------------------- | :------------------- | :----------------------- | :------------------------------------- |
| GIF                                                     | `vid`                 | **Ruta loop-intent** | `.gif` o video           | Ruta rápida de GIF preservada          |
| WebP / AVIF / APNG / HEIC / HEIF / JXL animados         | `vid`                 | **Ruta loop-intent** | `.gif` / `.mov` / `.mp4` | `img` ignora estos                     |
| Animación moderna corta silenciosa con `--apple-compat` | `vid` + `loop_intent` | **Forzar GIF**       | `.gif`                   | Duración `<= 6s`                       |
| Animación moderna larga con `--apple-compat`            | `vid` + `loop_intent` | **No forzar GIF**    | video objetivo           | Duración `>= 15s` permanece como video |
| Animación moderna incierta con `--apple-compat`         | `vid` + `loop_intent` | **Forzar GIF**       | `.gif`                   | Fallback de compatibilidad             |

### Matriz de decisión de códec de video

| Códec de entrada                | Modo normal             | Modo `--apple-compat`   | Notas                                                |
| :------------------------------ | :---------------------- | :---------------------- | :--------------------------------------------------- |
| H.264 (AVC)                     | **Convertir**           | **Convertir**           | No se omite previamente en ningún modo               |
| VP9                             | **Omitir**              | **Convertir a HEVC**    | Fuente incompatible con Apple                        |
| AV1                             | **Omitir**              | **Convertir a HEVC**    | Fuente incompatible con Apple                        |
| VVC / AV2                       | **Omitir**              | **Convertir a HEVC**    | Fuente incompatible con Apple                        |
| HEVC (H.265)                    | **Omitir**              | **Omitir**              | Ya es un objetivo nativo de Apple                    |
| ProRes / DNxHD / códecs legados | **Convertir si es nec** | **Convertir si es nec** | Mantener/omitir final aún depende de la optimización |

Los límites de calidad y tamaño aún se aplican después del enrutamiento. En `--ultimate` y otros flujos de coincidencia de calidad, una ruta que es elegible para la conversión aún puede terminar como omitida si el archivo producido falla en los requisitos de calidad/tamaño y no se aplica ningún fallback permitido.

### Estrategia de formato HDR

| Tipo HDR          | Detección                                | Estrategia de preservación                                                                     |
| :---------------- | :--------------------------------------- | :--------------------------------------------------------------------------------------------- |
| **HDR10**         | mastering_display + max_cll en side_data | Metadatos estáticos totalmente preservados mediante argumentos FFmpeg                          |
| **HEIC Gainmap**  | Imagen auxiliar HEIC (Apple/Samsung/ISO) | Sintetizado a HDR lineal de 32 bits -> JXL (True HDR)                                          |
| **UltraHDR JPEG** | JPEG APP1/APP2 + XMP (hdrgm:)            | Metadatos preservados; se emite advertencia de pérdida de gainmap                              |
| **HLG**           | color_trc = arib-std-b67                 | Primarias de color + TRC preservados                                                           |
| **Dolby Vision**  | DOVI side_data en flujos/cuadros         | Extracción de RPU vía `dovi_tool` → inyección x265; Conversión de Perfil 7 → 8.1               |
| **HDR10+**        | ST2094-40 metadatos dinámicos            | Soportado vía extracción `hdr10plus_tool` e inyección x265 (retención de metadatos Perfil A/B) |
| **SDR**           | Sin marcadores HDR                       | Procesamiento estándar (yuv420p)                                                               |

## ⬇️ Instalación

### Binarios precompilados

Para los usuarios que no deseen instalar el conjunto de herramientas de Rust, pueden descargar binarios precompilados desde la página de
**[Releases](https://github.com/nowaytouse/modern-format-boost/releases)**.

```bash
# Ejemplo para macOS ARM64
curl -LO https://github.com/nowaytouse/modern-format-boost/releases/latest/download/modern-format-boost-aarch64-apple-darwin.tar.gz
tar -xzf modern-format-boost-aarch64-apple-darwin.tar.gz

```

### Requisitos previos

| Herramienta          | ¿Requerido? | Propósito                         | Comando de instalación                                                                      |
| :------------------- | :---------: | :-------------------------------- | :------------------------------------------------------------------------------------------ |
| **Rust** (nightly)   |     ✅      | Construcción e instalación        | `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \| sh && rustup default nightly` |
| **FFmpeg** (5.0+)    |     ✅      | Procesamiento de video y métricas | `brew install ffmpeg` / `apt install ffmpeg`                                                |
| **libjxl**           |     ✅      | Núcleo de codificación JXL        | `brew install jpeg-xl`                                                                      |
| **ExifTool**         |     ✅      | Preservación de metadatos         | `brew install exiftool`                                                                     |
| **ImageMagick**      |     ✅      | Ruta de desvío de imagen          | `brew install imagemagick`                                                                  |
| **libwebp**          |     ✅      | Decodificación nativa WebP        | `brew install webp`                                                                         |
| **libheif**          |     ✅      | Decodificación HEIC/HEIF          | `brew install libheif`                                                                      |
| **PostgreSQL** (12+) |     ✅      | Base de datos de caché y calidad  | `brew install postgresql pgvector` / `apt install postgresql`                               |
| **dovi_tool**        |  Opcional   | Extracción de RPU Dolby Vision    | `cargo install dovi_tool`                                                                   |
| **hdr10plus_tool**   |  Opcional   | Extracción de metadatos HDR10+    | `cargo install hdr10plus_tool`                                                              |

#### macOS (Homebrew)

```bash
brew install ffmpeg jpeg-xl exiftool imagemagick webp libheif postgresql pgvector
```

#### Linux (Ubuntu/Debian)

```bash
sudo apt update && sudo apt install ffmpeg libimage-exiftool-perl imagemagick \
  webp libheif-dev postgresql postgresql-contrib postgresql-server-dev-all
# En Linux, pgvector debe ser compilado e instalado:
git clone --branch v0.5.1 https://github.com/pgvector/pgvector.git
cd pgvector
make && sudo make install
```

### 🗄️ Configuración de la Base de Datos

Modern Format Boost utiliza PostgreSQL (con la extensión `pgvector`) como motor obligatorio de caché local e inferencia de calidad. Ambos binarios, `img` y `vid`, se conectan a la base de datos al iniciarse y fallarán si el servicio no está accesible.

#### 1. Iniciar servicio PostgreSQL

- **macOS**: `brew services start postgresql`
- **Linux**: `sudo systemctl start postgresql`

#### 2. Crear la Base de Datos

El nombre predeterminado de la base de datos es `modern_format_boost`. Créela antes de ejecutar las herramientas:

```bash
createdb modern_format_boost
```

O mediante SQL:

```sql
CREATE DATABASE modern_format_boost;
```

### Construir desde la fuente

```bash
git clone https://github.com/nowaytouse/modern-format-boost.git
cd modern-format-boost
cargo build --release

```

## 🚀 Uso

### Inicio rápido

```bash
# Conversión de ruta de imagen
img run /ruta/a/los/medios
# Conversión de ruta de video
vid run /ruta/a/los/medios

# Para usar la estrategia AV1:
vid run --codec av1 /ruta/a/los/medios
```

### ⚡ Modo Rápido y Reanudación Inteligente

El **Modo Rápido** (`fastmode`) está adaptado para flujos de trabajo de interfaz de usuario de arrastrar y soltar (`crates/dev/src/bin/drag_and_drop_processor.rs`). Ofrece capacidades de reanudación de alta confiabilidad:

- **Gestión de estado `WorkingCopyMarker`**: Rastrea de forma segura el estado de procesos parciales en los cierres.
- **Detección de Fuentes Obsoletas**: Detecta automáticamente si los archivos originales han cambiado y fuerza una reconstrucción nueva, evitando reintentos sucios.
- **Protección Fail-Closed**: Captura de contexto profundo y verificación de `Blake3` garantizan cero corrupción de archivos durante escenarios interrumpidos de `img run`.

### Opciones detalladas

- `--ultimate`: Búsqueda de grado de archivo con **precisión de 0.01** (Alta calidad, alto costo de tiempo).
- `--apple-compat`: Habilita la compatibilidad con el ecosistema Apple (Live Photos/AAE). El valor predeterminado del CLI es activado; `--no-apple-compat` lo desactiva.
- `--in-place`: Reemplaza los archivos originales. **ADVERTENCIA: IRREVERSIBLE.**
- `-o /dir`: Directorio de salida seguro. (Recomendado)
- `--verbose`: Muestra registros detallados del procesamiento.
- `--no-recursive`: No desciende a subdirectorios.
- `--force-video`: Fuerza el tratamiento de imágenes animadas como video independientemente del Loop Intent.

### Subcomandos avanzados

- `img cache-stats`: Ver estadísticas de la caché de análisis SQLite.
- `vid strategy <archivo>`: Previsualizar la estrategia del flujo para un archivo específico.
- `img restore-timestamps`: Corrección masiva de fechas de creación basadas en patrones de nombre de archivo (recuperación de metadatos).

### 💡 Nota sobre múltiples instancias

**Modern Format Boost** admite de forma nativa la ejecución de múltiples ventanas/instancias.

- **Procesamiento concurrente**: Permite ejecutar múltiples ventanas para manejar diferentes rutas de forma independiente.
- **Nota**: Ajuste según el rendimiento de E/S de su hardware; la concurrencia excesiva puede causar condiciones de carrera en el sistema de archivos.

## 🏗️ Arquitectura

### CI/CD y Puertas de Calidad

Modern Format Boost utiliza un estricto sistema de control de calidad para garantizar una arquitectura con cero deuda técnica:

- **Herramientas Rust-first**: Los entrypoints de ingeniería son bins Rust bajo `crates/dev/src/bin`; los originales Python se conservan solo como referencias de compatibilidad hasta confirmar su eliminación segura.
- **Verificación de CI local**: Antes de desarrollar, asegúrese de usar `just fix-gate` o `cargo run --locked -p dev --bin check_all -- --allow-non-nightly`. Esta es la "Fuente Única de Verdad" (SSOT) para el formateo de código, análisis estático y pruebas automatizadas.
- **Endurecimiento y Estabilidad de Pruebas**: "Fail Fast" está deshabilitado para recopilar información de diagnóstico completa en todas las plataformas; además, se ha agregado una captura de contexto profundo para estados de error de imagen (como las afirmaciones de restauración de JPEG).

### Estructura principal

- `crates/img/`: Optimizador de imágenes estáticas (`JXL` / omitir / ignorar en la ruta CLI actual)
- `crates/vid/`: Optimizador de video y medios animados (`HEVC` / `AV1` / `GIF`)
- `crates/foundation/`: Cerebro central (motor híbrido GPU/CPU, mapeo HDR, metadatos)
- `Modern Format Boost.app/`: Interfaz de usuario macOS para arrastrar y soltar

## ❓ FAQ

**1. ¿Es JXL ampliamente compatible?**
El soporte nativo existe en macOS 14+ / iOS 17+, Chrome 91+ y Firefox 128+. Sin embargo, existen problemas conocidos en el ecosistema:

- **Animaciones**: Los formatos animados modernos (JXL/AV1/HEIF) a menudo no se previsualizan como animaciones en la aplicación nativa de Fotos de macOS/iOS o en el Finder (solo estática), especialmente cuando se sincronizan a través de iCloud. Se reproducen correctamente en navegadores modernos o herramientas especializadas.
- **Miniaturas**: Los archivos JXL que usan **perfiles ICC en escala de grises** pueden aparecer como **miniaturas negras** en Finder/iCloud, aunque se rendericen perfectamente al abrirlos.
  JXL sigue siendo el formato superior para el archivo exacto en bits y el almacenamiento HDR de alta fidelidad.

**2. ¿Cómo se maneja HDR10+?**
Totalmente compatible. Usamos `hdr10plus_tool` para extraer metadatos dinámicos SMPTE 2094-40 e inyectarlos de nuevo en el flujo HEVC a través del parámetro `--dhdr10-info` de `libx265`. Asegúrese de que la herramienta esté instalada para habilitar esta función.

**3. ¿Por qué omitir WebP/AVIF/HEIC?**
Las imágenes WebP/AVIF/HEIC/HEIF con pérdida suelen omitirse porque ya son formatos modernos con pérdida, y volver a codificarlos correría el riesgo de pérdida generacional para un beneficio pequeño. Las excepciones importantes en el código actual son:

- Las imágenes estáticas modernas sin pérdida aún pueden convertirse a JXL.
- Los activos de gainmap HEIC/HEIF pueden sintetizarse en HDR JXL.
- Los formatos animados modernos no son manejados por `img`; se enrutan a través de `vid` y `loop_intent`.

---

## ⚖️ Licencia

Licenciado bajo la **Licencia MIT**.

## Dependencias del tiempo de ejecución

Este proyecto orquestas varios gigantes del código abierto. Agradecemos a sus autores por sus contribuciones:

| Componente             | Licencia   | Propósito                 |
| ---------------------- | ---------- | ------------------------- |
| **FFmpeg**             | LGPL/GPL   | Procesamiento de video    |
| **libjxl** (cjxl/djxl) | BSD-3      | Codificación JPEG XL      |
| **ExifTool**           | Perl/GPL   | Preservación de metadatos |
| **ImageMagick**        | Apache 2.0 | Ruta de desvío de imagen  |
| **SVT-AV1**            | BSD+Patent | Codificación AV1          |
| **x265**               | GPL-2.0    | Codificación HEVC         |

Todas las dependencias de Rust se gestionan a través de `Cargo.toml` y se rigen por sus respectivas licencias de código abierto (MIT/Apache/BSD).
