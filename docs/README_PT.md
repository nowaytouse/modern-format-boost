<p align="center">
  <img src="https://img.shields.io/badge/versão-0.11.2-0969DA?style=for-the-badge&logo=rust&logoColor=white" alt="Versão">
  <img src="https://img.shields.io/badge/rust-edição_2021-E57324?style=for-the-badge&logo=rust&logoColor=white" alt="Rust">
  <img src="https://img.shields.io/badge/plataforma-macOS_%7C_Linux_%7C_Windows-8257E5?style=for-the-badge&logo=apple&logoColor=white" alt="Plataforma">
  <img src="https://img.shields.io/badge/licença-MIT-00B265?style=for-the-badge" alt="Licença">
</p>

<h1 align="center">Modern Format Boost</h1>

<p align="center">
  <strong>Motor de otimização de mídia de última geração — perda de qualidade zero, compressão máxima.</strong><br>
  <em>下一代媒体优化引擎 — 画质零損失，体積最大圧縮。</em>
</p>

---
# 📖 Português (Portuguese)

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
4. **Maturidade das ferramentas**: As ferramentas baseadas em HEVC (`img-hevc`, `vid-hevc`) são atualmente mais maduras e estáveis do que as baseadas em AV1 (`img-av1`, `vid-av1`). Para tarefas de produção de alta confiabilidade, recomendam-se as ferramentas HEVC.

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
- **Estágio 4 — Síntese HDR HEIC**: Intercepta arquivos HEIC com Gainmaps (Apple/Google) e sintetiza buffers HDR de luz linear de 32 bits via um pipeline **OpenEXR** intermediário, entregando saída JXL HDR real.
</details>

### 🖥️ Runtime
![Runtime](assets/runtime.png)

### Os quatro binários

| Binário | Propósito | Codec de Destino |
|--------|---------|-------------|
| **`img-hevc`** | Otimização de imagem | → JXL (estático) / HEVC (animado) |
| **`img-av1`** | Otimização de imagem | → JXL (estático) / AV1 (animado) |
| **`vid-hevc`** | Otimização de vídeo | → HEVC / H.265 |
| **`vid-av1`** | Otimização de vídeo | → AV1 / SVT-AV1 |

## 📉 Exemplos de compressão reais

| Formato de entrada | Tamanho original | Formato de saída | Tamanho de saída | Economia | Método |
|:---|:---|:---|:---|:---|:---|
| Paisagem JPEG | 4.2 MB | **JXL** | 3.3 MB | **~21%** | Reconstrução sem perdas |
| Screenshot PNG | 2.5 MB | **JXL** | 1.1 MB | **~56%** | Modular d=0.0 |
| Action Cam H.264 | 1.2 GB | **HEVC** | 480 MB | **~60%** | Busca CRF GPU/CPU |

## ⬇️ Instalação

### Binários pré-compilados
Para usuários que não desejam instalar o Rust, baixe os binários pré-compilados na página de **[Releases](https://github.com/nowaytouse/modern-format-boost/releases)**.

```bash
# macOS/Linux (exemplo para macOS ARM64)
curl -LO https://github.com/nowaytouse/modern-format-boost/releases/latest/download/modern-format-boost-aarch64-apple-darwin.tar.gz
tar -xzf modern-format-boost-aarch64-apple-darwin.tar.gz
```

---
# ⚖️ Licença
Licenciado sob a **MIT License**.
