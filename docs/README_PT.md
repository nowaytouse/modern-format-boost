# Modern Format Boost

![Version](https://img.shields.io/badge/version-0.11.3-0969DA?style=for-the-badge&logo=rust&logoColor=white)
![Rust](<https://img.shields.io/badge/rust-2024_edition_(nightly)-E57324?style=for-the-badge&logo=rust&logoColor=white>)
![Platform](https://img.shields.io/badge/platform-macOS_%7C_Linux_%7C_Windows-8257E5?style=for-the-badge&logo=apple&logoColor=white)
![License](https://img.shields.io/badge/license-MIT-00B265?style=for-the-badge)

**Motor de otimização de mídia de próxima geração — zero perda de qualidade, compressão máxima.**

[English](../README.md) · [简体中文](README_ZH.md) · [繁體中文](README_ZH_TW.md) · [日本語](README_JA.md) · [한국어](README_KO.md) · [Español](README_ES.md) · [Français](README_FR.md) · [Português](README_PT.md) · [Русский](README_RU.md) · [العربية](README_AR.md)

## O que é o Modern Format Boost?

**Modern Format Boost** é um motor de otimização de mídia de alto desempenho baseado em Rust. Ele divide o trabalho por domínio de mídia:

- `img` lida **apenas com imagens estáticas**
- `vid` lida com **vídeos e mídia animada**

Na implementação atual, as rotas típicas são:

- 📸 **Imagens estáticas (caminho principal da CLI `img run`)**: Reconstrução sem perdas JPEG → JXL; PNG/TIFF/BMP e outras fotos estáticas sem perdas → JXL; fotos estáticas modernas com perdas geralmente são puladas; entradas animadas ou com animação ambígua são ignoradas.
- 🎬 **Vídeos**: H.264 e outros codecs que não são alvo passam por uma busca de qualidade HEVC/AV1; a escolha do codec/contêiner depende de `--codec` e `--apple-compat`.
- 🎞️ **Mídia animada**: O roteamento de animação GIF/WebP/AVIF/APNG/HEIC/HEIF/JXL é de responsabilidade do `vid`, além da política compartilhada `loop_intent`.

Pense nele como um otimizador conservador que prefere resultados honestos de pular/ignorar em vez de danos silenciosos à qualidade:

- 🍎 **Ecossistema Apple em primeiro lugar**: Modo de compatibilidade total com Apple, detecção de Live Photo, manipulação de arquivos sidecar AAE.
- 🔒 **Guardião de metadados**: Preserva EXIF, XMP, perfis ICC, carimbos de data/hora de criação, xattrs do macOS, etiquetas do Finder.
- ⚡ **Otimização de Velocidade Percebida**: Estratégia de ordenação "Deep-First" — prioriza primeiro os níveis de diretório mais profundos, depois ordena por tamanho de arquivo e formato, para garantir um processamento em lote eficiente e taxa de transferência máxima.
- 🎞️ **Metadatos Dinâmicos HDR10+**: Retenção total de metadatos SMPTE 2094-40 via extração de sidecars e injeção SEI x265.
- 🌅 **Síntese de Gainmap HDR**: Sintetiza automaticamente buffers HDR lineares de 32 bits de alta fidelidade a partir de Gainmaps HEIC da Apple/Samsung/ISO, garantindo que a faixa dinâmica máxima seja preservada ao converter para JXL.
- **🔍 Consciência de Metadatos de Fornecedores**: Escaneamento inteligente de namespaces XMP específicos da Samsung/Google em arquivos HEIC para garantir a preservação máxima do contexto.

## ⚠️ Isenção de Responsabilidade e Notas Importantes

1. **Segurança de Dados Primeiro**: Para evitar qualquer perda potencial de dados, é altamente recomendável enviar os arquivos processados para um diretório separado (por exemplo, usando `-o /caminho/para/saida`) em vez de usar a conversão no local (`--in-place`), especialmente para mídias insubstituíveis.
2. **Software Beta**: Embora este programa tenha sido extensivamente testado, depurado e otimizado para evitar perda de qualidade ou de dados (conforme visto no registro de alterações), não há garantia de que esteja 100% livre de bugs. Por favor, relate quaisquer problemas encontrados no GitHub.
3. **Visão de Computação**: Embora otimizado para eficiência (especialmente em Apple Silicon série M), o processamento de lotes massivos no modo `--ultimate` ainda pode consumir tempo. Ele ocupará recursos do sistema por um período prolongado; por favor, planeje sua tarefa adequadamente.
4. **Maturidade da Ferramenta**: As ferramentas unificadas (`img`, `vid`) têm como padrão o HEVC, que é mais maduro e estável do que a estratégia AV1. Para tarefas de produção de alta confiabilidade, o HEVC (o padrão) é recomendado.

## 🔒 Privacidade e Integridade de Dados

O **Modern Format Boost** é construído em uma arquitetura "Local-First", garantindo que seus ativos criativos permaneçam inteiramente sob seu controle.

- **Operação Offline (Air-Gapped)**: Processamento 100% offline. Sem telemetria, rastreamento de uso ou comunicações com a nuvem. Os binários principais não contêm código relacionado à rede.
- **Runtime Protegido por Rust**: Construído com Rust para eliminar nativamente bugs de corrupção de memória (buffer overflows, etc.).
- **Integração Segura**: Todas as ferramentas externas (FFmpeg, cjxl) são invocadas via primitivas seguras e escapadas — nunca através de execução direta de shell — prevenindo injeção de comandos arbitrários.
- **Isolamento de Caminho**: Normalização avançada previne travessia de diretório e protege arquivos de sistema não relacionados.
- **Lista de Bloqueio de Caminhos de Sistema**: Proteções integradas para diretórios sensíveis do sistema para evitar modificações acidentais de arquivos do SO.
- **Balanceamento Dinâmico de Recursos**: Ajusta automaticamente as threads de processamento com base na carga de memória/CPU para evitar falhas do sistema durante tarefas extremas.
- **Guardião Abrangente de Metadatos**: Preservação estrita bit a bit de EXIF, XMP, ICC e carimbos de data/hora do sistema de arquivos (btime/mtime).
- **Processamento Seguro e Isolamento de Sessão**:
  - **Zero Poluição de Espaço de Trabalho**: O rastreamento centralizado (`~/.mfb_progress/`) mantém suas pastas de mídia 100% limpas. Nenhum arquivo de metadados oculto permanece entre suas fotos/vídeos.
  - **Arquivos Temporários Sem Conflito**: Cada arquivo de análise intermediário (fluxos YUV, segmentos de análise) é identificado de forma exclusiva com um UUID aleatório. Isso evita colisões entre várias instâncias e garante "Precisão Cirúrgica" durante a limpeza.
  - **Limpeza ao Iniciar**: Quer uma tarefa seja concluída com sucesso ou retomada após uma interrupção, o sistema limpa automaticamente todos os dados transitórios. Esta arquitetura de "Auto-Limpeza" garante que seu disco permaneça livre de sobras de processamento abandonadas.
  - **Redefinição Inteligente de Checkpoint**: Detecta automaticamente quando um usuário exclui manualmente o diretório de saída para "recomeçar", acionando uma redefinição total de estado mesmo no modo de retomada.

## 🛠️ Técnico Profundo: Como Funciona — O Pipeline

### Lógica do Pipeline de Imagem

Cada arquivo passa por um pipeline de decisão em vários estágios:

- **Estágio 1 — Detecção Inteligente**: Analisa tabelas DQT de JPEG (detecção de gainmap UltraHDR), chunks VP8L de WebP e caixas `av1C` de AVIF em nível binário. Agora apresenta a **Zero-Debt Architecture** com 100% de conformidade com o Clippy e análise robusta de cabeçalhos `OpenEXR`/`JPEG 2000`.
- **Estágio 2 — Rota e Codificação**: JXL VarDCT para JPEG (bit-exact); modo Modular para fontes sem perdas (PNG, WebP/AVIF/HEIC/EXR/JP2 sem perdas).
- **Estágio 3 — Caminho de Desvio**: Formatos como TIFF/WebP/BMP/HEIC são pré-processados em PNGs temporários de 16 bits ou **OpenEXR de 32 bits** para garantir compatibilidade com o `cjxl` sem perda de qualidade (pipeline correspondente a 8/16/32 bits).
- **Estágio 4 — Síntese HEIC HDR**: Intercepta arquivos HEIC com Gainmaps (Apple/Google) e sintetiza buffers HDR de luz linear de 32 bits via um pipeline de escolta **OpenEXR** intermediário, entregando saída JXL HDR real.
- **Estágio 5 — Divisão Estática/Animada**: O `img` agora rejeita estritamente ativos animados ou com animação ambígua. Formatos modernos animados são delegados ao `vid` em vez de serem convertidos dentro do pipeline estático.
- **Estágio 6 — Loop Intent v3**: A lógica compartilhada de loop-intent decide se a mídia animada deve permanecer como GIF ou prosseguir pelo pipeline de vídeo. A política de entrega de animação moderna compatível com Apple é centralizada aqui.

### Pipeline de Vídeo: Busca de Saturação em Três Fases

1. **Fase 1: Busca Grosseira por GPU**: Busca binária em codificadores de hardware (VideoToolbox/NVENC) para encontrar o "joelho da qualidade".
2. **Fase 2: Ajuste Fino por CPU**: Mapeia o CRF da GPU para a escala do `x265`. Usa **Sprint & Backtrack** (passo duplo no sucesso, redefinição para 0.1 no excesso).
3. **Fase 3: Portão de Qualidade 3D Final**: Requer aprovação simultânea de VMAF-Y ≥ 86.0 (piso de sanidade, relativo à linha de base dinâmica), CAMBI ≤ 6.0 (banding) e PSNR-UV ≥ 30.0 dB (piso de sanidade de croma).
   - **Pontuação de Fusão**: Combina MS-SSIM + SSIM_All (peso 0.6/0.4) para uma análise estrutural robusta.
   - **Chroma Guard**: Detecta automaticamente resoluções pequenas que fariam o libvmaf MS-SSIM falhar e alterna para pontuação apenas em Y para garantir a confiabilidade do processamento.
   - _Nota: No modo `--ultimate`, a busca só termina após **50 amostras consecutivas** mostrarem ganho de qualidade zero, garantindo saturação absoluta._

### Preservação de Metadatos e HDR

- **HDR**: Preserva primárias bt2020, TRC PQ/HLG e metadatos de Mastering Display.
- **Dolby Vision**: Extrai RPU via `dovi_tool` e injeta no x265 (conversão de Perfil 7 → 8.1).
- **xattrs do macOS**: Preserva Etiquetas do Finder, Data de Adição e carimbos de data/hora de criação via `copyfile` e `setattrlist`.

### 🖥️ Tempo de execução

![Runtime](../assets/runtime.png)

Tempo de execução

### Os Dois Binários

| Binário   | Propósito                            | Codec Alvo                 |
| --------- | ------------------------------------ | -------------------------- |
| **`img`** | Apenas otimização de imagem estática | → JXL / pular / ignorar    |
| **`vid`** | Otimização de vídeo e mídia animada  | → HEVC / AV1 / GIF / pular |

Além de um **aplicativo macOS de clique duplo** (`Modern Format Boost.app`) para processamento em lote por arrastar e soltar.

## 📉 Exemplos de Compressão no Mundo Real

| Formato de Entrada  | Tamanho Original | Formato de Saída | Tamanho de Saída | Economia | Método                                |
| :------------------ | :--------------- | :--------------- | :--------------- | :------- | :------------------------------------ |
| Paisagem JPEG       | 4.2 MB           | **JXL**          | 3.3 MB           | **~21%** | Reconstrução de componente sem perdas |
| Captura de Tela PNG | 2.5 MB           | **JXL**          | 1.1 MB           | **~56%** | Modular d=0.0                         |
| Action Cam H.264    | 1.2 GB           | **HEVC**         | 480 MB           | **~60%** | Busca de CRF por GPU/CPU              |
| WebP Animado        | 15 MB            | **AV1 / HEVC**   | 1.8 MB           | **~88%** | Transcodificado para formato de vídeo |

## 📊 Matriz de Processamento

### Matriz de Decisão de Formato de Imagem

| Formato de Entrada                             | ¿Estático? | Ação no `img run`              | Saída       | Notas                                          |
| :--------------------------------------------- | :--------: | :----------------------------- | :---------- | :--------------------------------------------- |
| JPEG                                           |     ✅     | **Reconstrução sem perdas**    | `.jxl`      | Bit-exact `cjxl --lossless_jpeg=1`             |
| PNG/TIFF/BMP/outras fotos sem perdas           |     ✅     | **Conversão sem perdas**       | `.jxl`      | Pode usar o caminho de desvio primeiro         |
| WebP/AVIF/HEIC/HEIF (foto estática sem perdas) |     ✅     | **Converter**                  | `.jxl`      | Fotos estáticas modernas sem perdas permitidas |
| HEIC/HEIF com Gainmap                          |     ✅     | **Síntese HDR**                | `.jxl`      | Caminho Gainmap sintetiza HDR linear           |
| Fotos antigas com perdas após validação        |     ✅     | **Conversão quase sem perdas** | `.jxl`      | Caminho de lote atual permanece focado em JXL  |
| WebP/AVIF/HEIC/HEIF com perdas                 |     ✅     | **Pular**                      | manter orig | Evitar perda geracional                        |
| JXL estático                                   |     ✅     | **Pular**                      | manter orig | Já está otimizado                              |
| Qualquer imagem animada ou ambígua             |     ❌     | **Ignorar**                    | nenhuma     | Fora do domínio apenas estático do `img`       |

### Nota de Roteamento do `img`

Existem dois pontos de entrada de conversão de imagem no repositório hoje:

- `img run` / caminho CLI de lote em `crates/img/src/main.rs`
- ajudante de biblioteca `smart_convert()` em `crates/img/src/conversion_api.rs`

Eles **não estão totalmente alinhados** agora.

- O caminho principal da CLI é atualmente orientado para JXL para conversões estáticas aceitas.
- O ajudante de API mais antigo ainda contém um ramo visando AVIF para algumas fotos estáticas com perdas não JPEG.
- A CLI do `img` também analisa `--codec`, mas no caminho de lote estático atual essa flag **não** altera materialmente as decisões reais de roteamento.

Este README documenta o **comportamento atual da CLI/runtime primeiro**, porque é isso que os usuários encontram no uso normal em lote.

### Matriz de Decisão de Mídia Animada

| Formato de Entrada                                     | Proprietário          | Ação                 | Saída                    | Notas                                 |
| :----------------------------------------------------- | :-------------------- | :------------------- | :----------------------- | :------------------------------------ |
| GIF                                                    | `vid`                 | **Rota loop-intent** | `.gif` ou vídeo          | Caminho rápido de GIF preservado      |
| WebP/AVIF/APNG/HEIC/HEIF/JXL animados                  | `vid`                 | **Rota loop-intent** | `.gif` / `.mov` / `.mp4` | O `img` ignora estes                  |
| Animação moderna curta silenciosa com `--apple-compat` | `vid` + `loop_intent` | **Forçar GIF**       | `.gif`                   | Duração `<= 6s`                       |
| Animação moderna longa com `--apple-compat`            | `vid` + `loop_intent` | **Não forçar GIF**   | alvo de vídeo            | Duração `>= 15s` permanece como vídeo |
| Animação moderna incerta com `--apple-compat`          | `vid` + `loop_intent` | **Forçar GIF**       | `.gif`                   | Fallback de compatibilidade           |

### Matriz de Decisão de Codec de Vídeo

| Codec de Entrada                | Modo Normal                | Modo `--apple-compat`      | Notas                                                       |
| :------------------------------ | :------------------------- | :------------------------- | :---------------------------------------------------------- |
| H.264 (AVC)                     | **Converter**              | **Converter**              | Não pré-pulado em nenhum dos modos                          |
| VP9                             | **Pular**                  | **Converter para HEVC**    | Fonte incompatível com Apple                                |
| AV1                             | **Pular**                  | **Converter para HEVC**    | Fonte incompatível com Apple                                |
| VVC / AV2                       | **Pular**                  | **Converter para HEVC**    | Fonte incompatível com Apple                                |
| HEVC (H.265)                    | **Pular**                  | **Pular**                  | Já é um alvo nativo da Apple                                |
| ProRes / DNxHD / codecs antigos | **Converter conforme nec** | **Converter conforme nec** | A decisão final de manter/pular ainda depende da otimização |

Os portões de qualidade e tamanho ainda se aplicam após o roteamento. No modo `--ultimate` e outros fluxos de correspondência de qualidade, uma rota elegível para conversão ainda pode terminar como pulada se o arquivo produzido falhar nos requisitos de qualidade/tamanho e nenhum fallback de melhor esforço permitido for aplicado.

### Estratégia de Formato HDR

| Tipo HDR          | Detecção                                 | Estratégia de Preservação                                                                               |
| :---------------- | :--------------------------------------- | :------------------------------------------------------------------------------------------------------ |
| **HDR10**         | mastering_display + max_cll em side_data | Metadatos estáticos totalmente preservados via argumentos do FFmpeg                                     |
| **HEIC Gainmap**  | Imagem auxiliar HEIC (Apple/Samsung/ISO) | Sintetizado para HDR linear de 32 bits -> JXL (True HDR)                                                |
| **UltraHDR JPEG** | JPEG APP1/APP2 + XMP (hdrgm:)            | Metadatos preservados; aviso de perda de gainmap emitido                                                |
| **HLG**           | color_trc = arib-std-b67                 | Primárias de cor + TRC preservados                                                                      |
| **Dolby Vision**  | DOVI side_data em streams/frames         | Extração de RPU via `dovi_tool` → injeção no x265; Conversão de Perfil 7 → 8.1                          |
| **HDR10+**        | metadatos dinâmicos ST2094-40            | Suportado via extração de sidecar `hdr10plus_tool` e injeção no x265 (retenção de metadatos Perfil A/B) |
| **SDR**           | Sem marcadores HDR                       | Processamento padrão (yuv420p)                                                                          |

## ⬇️ Instalação

### Binários Pré-compilados

Para usuários que não desejam instalar o toolchain do Rust, você pode baixar binários pré-compilados na página de
**[Releases](https://github.com/nowaytouse/modern-format-boost/releases)**.

```bash
# Exemplo para macOS ARM64
curl -LO https://github.com/nowaytouse/modern-format-boost/releases/latest/download/modern-format-boost-aarch64-apple-darwin.tar.gz
tar -xzf modern-format-boost-aarch64-apple-darwin.tar.gz

```

### Pré-requisitos

| Ferramenta           | Necessário? | Finalidade                          | Comando de instalação                                                                       |
| :------------------- | :---------: | :---------------------------------- | :------------------------------------------------------------------------------------------ |
| **Rust** (nightly)   |     ✅      | Compilação e instalação             | `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \| sh && rustup default nightly` |
| **FFmpeg** (5.0+)    |     ✅      | Processamento de vídeo e métricas   | `brew install ffmpeg` / `apt install ffmpeg`                                                |
| **libjxl**           |     ✅      | Núcleo de codificação JXL           | `brew install jpeg-xl`                                                                      |
| **ExifTool**         |     ✅      | Preservação de metadatos            | `brew install exiftool`                                                                     |
| **ImageMagick**      |     ✅      | Pipeline de desvio de imagem        | `brew install imagemagick`                                                                  |
| **libwebp**          |     ✅      | Decodificação nativa WebP           | `brew install webp`                                                                         |
| **libheif**          |     ✅      | Decodificação HEIC/HEIF             | `brew install libheif`                                                                      |
| **PostgreSQL** (12+) |     ✅      | Banco de dados de cache e qualidade | `brew install postgresql pgvector` / `apt install postgresql`                               |
| **dovi_tool**        |  Opcional   | Extração de RPU Dolby Vision        | `cargo install dovi_tool`                                                                   |
| **hdr10plus_tool**   |  Opcional   | Extração de metadatos HDR10+        | `cargo install hdr10plus_tool`                                                              |

#### macOS (Homebrew)

```bash
brew install ffmpeg jpeg-xl exiftool imagemagick webp libheif postgresql pgvector
```

#### Linux (Ubuntu/Debian)

```bash
sudo apt update && sudo apt install ffmpeg libimage-exiftool-perl imagemagick \
  webp libheif-dev postgresql postgresql-contrib postgresql-server-dev-all
# No Linux, o pgvector deve ser compilado e instalado:
git clone --branch v0.5.1 https://github.com/pgvector/pgvector.git
cd pgvector
make && sudo make install
```

### 🗄️ Configuração do Banco de Dados

O Modern Format Boost utiliza o PostgreSQL (com a extensão `pgvector`) como mecanismo obrigatório de cache local e inferência de qualidade. Ambos os binários `img` e `vid` conectam-se ao banco de dados na inicialização e falharão se o serviço estiver inacessível.

#### 1. Iniciar o Serviço PostgreSQL

- **macOS**: `brew services start postgresql`
- **Linux**: `sudo systemctl start postgresql`

#### 2. Criar o Banco de Dados

O nome padrão do banco de dados é `modern_format_boost`. Crie-o antes de executar as ferramentas:

```bash
createdb modern_format_boost
```

Ou via SQL:

```sql
CREATE DATABASE modern_format_boost;
```

### Build da Fonte

```bash
git clone https://github.com/nowaytouse/modern-format-boost.git
cd modern-format-boost
cargo build --release

```

## 🚀 Uso

### Início Rápido

```bash
# Conversão de caminho de imagem
img run /caminho/para/mídia
# Conversão de caminho de vídeo
vid run /caminho/para/mídia

# Para usar a estratégia AV1:
vid run --codec av1 /caminho/para/mídia

```

### ⚡ Modo Rápido e Retomada Inteligente

O **Modo Rápido** (`fastmode`) é adaptado para fluxos de trabalho de interface de arrastar e soltar (`crates/dev/src/bin/drag_and_drop_processor.rs`). Ele traz capacidades de retomada de alta confiabilidade:

- **Gerenciamento de Estado `WorkingCopyMarker`**: Rastreia com segurança o status de processos parciais através de fechamentos.
- **Detecção de Fontes Obsoletas**: Detecta automaticamente se os arquivos originais mudaram e força uma nova reconstrução, evitando novas tentativas sujas.
- **Proteção Fail-Closed**: Captura de contexto profundo e verificação `Blake3` garantem zero corrupção de arquivos durante interrupções de `img run`.

### Opções Detalhadas

- `--ultimate`: Busca com **precisão de 0.01** de nível de arquivamento (Alta qualidade, alto custo de tempo).
- `--apple-compat`: Ativa a compatibilidade com o ecossistema Apple (Live Photos/AAE). O padrão da CLI é ativado; `--no-apple-compat` o desativa.
- `--in-place`: Substitui os arquivos originais. **AVISO: IRREVERSÍVEL.**
- `-o /dir`: Diretório de saída seguro. (Recomendado)
- `--verbose`: Mostra logs de processamento detalhados.
- `--no-recursive`: Não desce em subdiretórios.
- `--force-video`: Força o tratamento de imagens animadas como vídeo, independentemente do Loop Intent.

### Subcomandos Avançados

- `img cache-stats`: Visualiza estatísticas do cache de análise SQLite.
- `vid strategy <arquivo>`: Pré-visualiza a estratégia do pipeline para un arquivo específico.
- `img restore-timestamps`: Correção em lote de datas de criação com base em padrões de nome de arquivo (recuperação de metadatos).

### 💡 Nota sobre Multi-Instância

O **Modern Format Boost** suporta nativamente a execução de várias janelas/instâncias.

- **Processamento Concorrente**: Permite executar várias janelas para lidar com caminhos diferentes de forma independente.
- **Nota**: Por favor, escale de acordo com o desempenho de E/S do seu hardware; concorrência excessiva pode causar condições de corrida no sistema de arquivos.

## 🏗️ Arquitetura

### CI/CD e Portões de Qualidade

O Modern Format Boost usa um rigoroso sistema de controle de qualidade para garantir uma arquitetura com zero dívida técnica:

- **Ferramentas Rust-first**: Os entrypoints de engenharia são bins Rust em `crates/dev/src/bin`; os originais Python ficam apenas como referências de compatibilidade até a confirmação de remoção segura.
- **Verificação de CI Local**: Antes de desenvolver, certifique-se de usar `just fix-gate` ou `cargo run --locked -p dev --bin check_all -- --allow-non-nightly`. Esta é a "Única Fonte de Verdade" (SSOT) para formatação de código, análise estática e testes automatizados.
- **Fortalecimento e Estabilidade de Testes**: O "Fail Fast" foi desativado para coletar informações de diagnóstico abrangentes em várias plataformas; também foi adicionada captura de contexto profundo para estados de erro de imagem (como verificações de restauração de JPEG).

### Estrutura Principal

- `crates/img/`: Otimizador de imagem estática (`JXL` / pular / ignorar no caminho CLI atual)
- `crates/vid/`: Otimizador de vídeo e mídia animada (`HEVC` / `AV1` / `GIF`)
- `crates/foundation/`: Cérebro central (motor híbrido GPU/CPU, mapeamento HDR, metadatos)
- `Modern Format Boost.app/`: Interface de arrastar e soltar do macOS

## ❓ FAQ

**1. O JXL é amplamente suportado?**
O suporte nativo existe no macOS 14+ / iOS 17+, Chrome 91+ e Firefox 128+. No entanto, existem problemas conhecidos no ecossistema:

- **Animações**: Formatos animados modernos (JXL/AV1/HEIF) muitas vezes falham na visualização como animações no aplicativo nativo Fotos do macOS/iOS ou no Finder (apenas estático), especialmente quando sincronizados via iCloud. Eles funcionam corretamente em navegadores modernos ou ferramentas especializadas.
- **Miniaturas**: Arquivos JXL que usam **perfis ICC em tons de cinza** podem aparecer como **miniaturas pretas** no Finder/iCloud, embora sejam renderizados perfeitamente quando abertos.
  O JXL continua sendo o formato superior para arquivamento exato de bits e armazenamento HDR de alta fidelidade.

**2. Como o HDR10+ é tratado?**
Totalmente suportado. Usamos o `hdr10plus_tool` para extrair metadatos dinâmicos SMPTE 2094-40 e injetá-los de volta no fluxo HEVC via parâmetro `--dhdr10-info` do `libx265`. Certifique-se de que a ferramenta esteja instalada para habilitar este recurso.

**3. Por que pular WebP/AVIF/HEIC?**
WebP/AVIF/HEIC/HEIF estáticos com perdas geralmente são pulados porque já são formatos modernos com perdas, e codificá-los novamente arriscaria perda geracional para um benefício pequeno. Exceções importantes no código atual são:

- fotos estáticas modernas sem perdas ainda podem ser convertidas para JXL
- ativos de gainmap HEIC/HEIF podem ser sintetizados em JXL HDR
- formatos modernos animados não são tratados pelo `img`; eles são roteados através do `vid` e `loop_intent`

---

## ⚖️ Licença

Licenciado sob a **Licença MIT**.

## Dependências de Execução

Este projeto orquestra vários gigantes do código aberto. Agradecemos aos seus autores por suas contribuições:

| Componente             | Licença    | Propósito                   |
| ---------------------- | ---------- | --------------------------- |
| **FFmpeg**             | LGPL/GPL   | Processamento de vídeo      |
| **libjxl** (cjxl/djxl) | BSD-3      | Codificação JPEG XL         |
| **ExifTool**           | Perl/GPL   | Preservação de metadatos    |
| **ImageMagick**        | Apache 2.0 | Caminho de desvio de imagem |
| **SVT-AV1**            | BSD+Patent | Codificação AV1             |
| **x265**               | GPL-2.0    | Codificação HEVC            |

Todas as dependências do Rust são gerenciadas via `Cargo.toml` e caem sob suas respectivas licenças de código aberto (MIT/Apache/BSD).
