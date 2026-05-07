# Modern Format Boost (Portuguese)

<p align="center">
  <img src="https://img.shields.io/badge/version-0.11.3-0969DA?style=for-the-badge&logo=rust&logoColor=white" alt="Versão">
  <img src="https://img.shields.io/badge/rust-2021_edition-E57324?style=for-the-badge&logo=rust&logoColor=white" alt="Rust">
  <img src="https://img.shields.io/badge/platform-macOS_%7C_Linux_%7C_Windows-8257E5?style=for-the-badge&logo=apple&logoColor=white" alt="Plataforma">
  <img src="https://img.shields.io/badge/license-MIT-00B265?style=for-the-badge" alt="Licença">
</p>

<p align="center">
  <strong>Mecanismo de otimização de mídia de última geração — qualidade superior com compressão máxima.</strong><br>
</p>

---

## 📖 Português (Portuguese)

## O que é o Modern Format Boost?

O **Modern Format Boost** é um motor de otimização de mídia de alto desempenho baseado em Rust. Ele converte formatos legados de imagem e vídeo (JPEG, PNG, H.264, VP9…) em codecs de última geração (**JPEG XL** para imagens, **HEVC/AV1** para vídeos) — alcançando reduções drásticas no tamanho do arquivo enquanto preserva ou até mesmo iguala bit a bit a qualidade original.

Pense nele como um "compressor inteligente" que **nunca degrada seus arquivos**:

- 📸 **Imagens**: JPEG → reconstrução sem perdas JXL (bit-exact, ~20% menor); PNG/WebP/TIFF/HEIC → JXL.
- 🎬 **Vídeos**: H.264/VP9/AV1 → HEVC com busca de qualidade acelerada por GPU.
- 🍎 **Ecossistema Apple primeiro**: Modo de compatibilidade total com Apple, detecção de Live Photo, manipulação de arquivos sidecar AAE.
- 🔒 **Guardião de metadados**: Preserva EXIF, XMP, perfis ICC, carimbos de data/hora de criação, xattrs do macOS e tags do Finder.
- ⚡ **Otimização de velocidade percebida**: Estratégia de classificação "Deep-First" — prioriza os níveis de diretório mais profundos primeiro, depois classifica por tamanho de arquivo e formato, para garantir um processamento em lote eficiente.
- 🎞️ **Metadados dinâmicos HDR10+**: Retenção total de metadados SMPTE 2094-40 via extração de sidecars e injeção SEI x265.
- 🌅 **Síntese de Gainmap HDR**: Sintetiza automaticamente buffers HDR lineares de 32 bits de alta fidelidade a partir de Gainmaps HEIC da Apple/Samsung/ISO, garantindo que o alcance dinâmico máximo seja preservado ao converter para JXL.
- **🔍 Consciência de metadados do fabricante**: Varredura inteligente de namespaces XMP específicos da Samsung/Google em arquivos HEIC para garantir a preservação máxima do contexto.

## ⚠️ Isenção de Responsabilidade e Notas Importantes

1. **Segurança de dados em primeiro lugar**: Para evitar qualquer perda potencial de dados, é altamente recomendável salvar os arquivos processados em um diretório separado (ex: usando `-o /caminho/para/output`) em vez de usar a conversão no local (`--in-place`), especialmente para mídias insubstituíveis.
2. **Software Beta**: Embora este programa tenha sido extensivamente testado e otimizado para evitar perda de qualidade ou dados, não há garantia de que seja 100% livre de bugs. Por favor, reporte quaisquer problemas no GitHub.
3. **Perspectiva de computação**: Embora otimizado para eficiência (especialmente em Apple Silicon série M), o processamento de lotes massivos no modo `--ultimate` ainda pode consumir tempo e recursos do sistema por um período prolongado.
4. **Maturidade das ferramentas**: As ferramentas unificadas (`img`, `vid`) utilizam por padrão a estratégia HEVC, que é atualmente mais madura e estável do que a estratégia AV1. Para tarefas de produção de alta confiabilidade, recomenda-se a estratégia HEVC (o padrão).

## 🔒 Privacidade e Integridade dos Dados

O **Modern Format Boost** é construído sobre uma arquitetura "Local-First", garantindo que seus ativos criativos permaneçam inteiramente sob seu controle.

- **Operação offline**: Processamento 100% offline. Sem telemetria, rastreamento de uso ou comunicações na nuvem. Os binários principais não contêm código relacionado à rede.
- **Runtime reforçado com Rust**: Construído com Rust para eliminar nativamente bugs de corrupção de memória.
- **Integração segura**: Todas as ferramentas externas (FFmpeg, cjxl) são invocadas via primitivas seguras e escapadas, evitando injeção de comandos.
- **Isolamento de caminhos**: A normalização avançada evita a travessia de diretórios e protege arquivos do sistema não relacionados.
- **Lista de bloqueio de caminhos do sistema**: Proteções integradas para diretórios sensíveis do sistema para evitar modificações acidentais de arquivos do SO.

<details>
<summary><b>🛠️ Detalhes técnicos: Como funciona — O Pipeline</b></summary>

### Lógica do Pipeline de Imagem

Cada arquivo passa por um pipeline de decisão em vários estágios:

- **Estágio 1 — Detecção inteligente**: Analisa tabelas JPEG DQT (detecção de gainmap UltraHDR), pedaços WebP VP8L e caixas AVIF `av1C` em nível binário.
- **Estágio 2 — Rota e Codificação**: JXL VarDCT para JPEG (bit-exact); modo Modular para fontes sem perdas (PNG, WebP/AVIF/HEIC/EXR/JP2 sem perdas).
- **Estágio 3 — Detour**: Formatos como TIFF/WebP/BMP/HEIC são pré-processados em PNGs temporários de 16 bits ou **OpenEXR de 32 bits** para garantir compatibilidade com `cjxl` sem perda de qualidade.
- **Estágio 4 — Síntese HDR HEIC**: Intercepta arquivos HEIC com Gainmaps (Apple/Google) e sintetiza buffers HDR de luz linear de 32 bits através de um fluxo intermédio **OpenEXR**, entregando saída JXL HDR real.
- **Estágio 5 — Loop Intent (v3)**: Mecanismo de árvore de decisão hierárquica de 7 camadas. Avalia o **Loop Closure**, **Motion Gini**, a **periodicidade** e a **fusão KNN** para identificar a intenção de loop (memes, stickers, loops).

</details>

### 🖥️ Runtime

![Runtime](../assets/runtime.png)

### As duas ferramentas unificadas

| Ferramenta | Propósito            | Codec de Destino                        |
| ---------- | -------------------- | --------------------------------------- |
| **`img`**  | Otimização de imagem | → JXL (estático) / HEVC / AV1 (animado) |
| **`vid`**  | Otimização de vídeo  | → HEVC / AV1                            |

## 📉 Exemplos de compressão reais

| Formato de entrada | Tamanho original | Formato de saída | Tamanho de saída | Economia | Método                  |
| :----------------- | :--------------- | :--------------- | :--------------- | :------- | :---------------------- |
| Paisagem JPEG      | 4.2 MB           | **JXL**          | 3.3 MB           | **~21%** | Reconstrução sem perdas |
| Screenshot PNG     | 2.5 MB           | **JXL**          | 1.1 MB           | **~56%** | Modular d=0.0           |
| Action Cam H.264   | 1.2 GB           | **HEVC**         | 480 MB           | **~60%** | Busca CRF GPU/CPU       |

### Pré-requisitos

| Ferramenta         | Necessário? | Propósito                         | Comando de Instalação                                             |
| :----------------- | :---------: | :-------------------------------- | :---------------------------------------------------------------- |
| **Rust** (1.75+)   |     ✅      | Construção e Instalação           | `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \| sh` |
| **FFmpeg** (5.0+)  |     ✅      | Processamento de vídeo e métricas | `brew install ffmpeg` / `apt install ffmpeg`                      |
| **libjxl**         |     ✅      | Núcleo de codificação JXL         | `brew install jpeg-xl`                                            |
| **ExifTool**       |     ✅      | Preservação de metadatos          | `brew install exiftool`                                           |
| **ImageMagick**    |     ✅      | Caminho de desvio de imagem       | `brew install imagemagick`                                        |
| **libwebp**        |     ✅      | Decodificação nativa de WebP      | `brew install webp`                                               |
| **dovi_tool**      |     ✅      | Extração de Dolby Vision RPU      | `cargo install dovi_tool`                                         |
| **libheif**        |     ✅      | Decodificação de HEIC/HEIF        | `brew install libheif`                                            |
| **hdr10plus_tool** |     ✅      | Extração de metadatos HDR10+      | `cargo install hdr10plus_tool`                                    |

---

## 🚀 Uso

### Início Rápido

```bash
# Conversão de caminho de imagens
img run /caminho/para/midia
# Conversão de caminho de vídeos
vid run /caminho/para/midia
```

---

## ❓ FAQ (Perguntas Frequentes)

**1. A compatibilidade do formato JXL é ampla?**  
O suporte nativo existe no macOS 14 (Sonoma) / iOS 17+, Chrome 91+ e Firefox 128+. No entanto, existem problemas conhecidos no ecossistema:

- **Animações**: Formatos animados modernos (JXL/AV1/HEIF) muitas vezes falham na pré-visualização como animações no aplicativo nativo Fotos do macOS/iOS ou no Finder (apenas estáticos), especialmente quando sincronizados via iCloud. Recomenda-se a pré-visualização através de ferramentas de linha de comando ou navegadores modernos.
- **Miniaturas**: Arquivos JXL que usam **perfis ICC em escala de cinza** podem aparecer como **miniaturas pretas** no Finder/iCloud, embora sejam renderizados perfeitamente quando abertos.  
  O JXL continua sendo o formato superior para arquivamento bit-exact e armazenamento HDR de alta fidelidade.

**2. Como o HDR10+ é tratado?**  
Totalmente suportado! Usamos o `hdr10plus_tool` para extrair os metadatos dinâmicos SMPTE 2094-40 e injetá-los de volta no fluxo HEVC via parâmetro `--dhdr10-info` do `libx265`. Certifique-se de que a ferramenta esteja instalada para este recurso.

**3. Por que pular WebP/AVIF/HEIC?**  
Esses formatos já são modernos e altamente comprimidos. A re-codificação causaria "perda geracional" (degradação da qualidade) com benefícios mínimos de tamanho.  
**Exceções**: A ferramenta _processará_ esses arquivos se detectar **HDR Gainmaps** de alta fidelidade para síntese no JXL, ou se um arquivo animado exigir otimização através do motor **Loop Intent (v3)** (que usa uma árvore de decisão hierárquica de 7 camadas para identificar memes, stickers e loops).

---

## ⚖️ Licença

Licenciado sob a **MIT License**.
