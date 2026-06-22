# Modern Format Boost

![Version](https://img.shields.io/badge/version-0.11.3-0969DA?style=for-the-badge&logo=rust&logoColor=white)
![Rust](<https://img.shields.io/badge/rust-2024_edition_(nightly)-E57324?style=for-the-badge&logo=rust&logoColor=white>)
![Platform](https://img.shields.io/badge/platform-macOS_%7C_Linux_%7C_Windows-8257E5?style=for-the-badge&logo=apple&logoColor=white)
![License](https://img.shields.io/badge/license-MIT-00B265?style=for-the-badge)

**Moteur d'optimisation multimédia de nouvelle génération — zéro perte de qualité, compression maximale.**

[English](../README.md) · [简体中文](README_ZH.md) · [繁體中文](README_ZH_TW.md) · [日本語](README_JA.md) · [한국어](README_KO.md) · [Español](README_ES.md) · [Français](README_FR.md) · [Português](README_PT.md) · [Русский](README_RU.md) · [العربية](README_AR.md)

## Qu'est-ce que Modern Format Boost ?

**Modern Format Boost** est un moteur d'optimisation multimédia haute performance basé sur Rust. Il divise le travail par domaine de média :

- `img` gère **uniquement les images statiques**
- `vid` gère les **vidéos et les médias animés**

Dans l'implémentation actuelle, les itinéraires typiques sont :

- 📸 **Images statiques (chemin CLI principal `img run`)** : Reconstruction sans perte JPEG → JXL ; PNG/TIFF/BMP et autres images fixes sans perte → JXL ; les images fixes modernes avec perte sont généralement ignorées ; les entrées animées ou ambiguës quant à l'animation sont ignorées.
- 🎬 **Vidéos** : H.264 et autres codecs non cibles passent par une recherche de qualité HEVC/AV1 ; le choix du codec/conteneur dépend de `--codec` et `--apple-compat`.
- 🎞️ **Médias animés** : Le routage des animations GIF/WebP/AVIF/APNG/HEIC/HEIF/JXL appartient à `vid` plus la politique partagée `loop_intent`.

Considérez-le comme un optimisateur conservateur qui préfère les résultats d'omission/ignorance honnêtes aux dommages silencieux sur la qualité :

- 🍎 **Écosystème Apple en priorité** : Mode de compatibilité Apple complète, détection de Live Photo, gestion des fichiers sidecar AAE.
- 🔒 **Gardien des métadonnées** : Préserve EXIF, XMP, les profils ICC, les horodatages de création, les xattrs macOS, les balises Finder.
- ⚡ **Optimisation de la vitesse perçue** : Stratégie de tri "Deep-First" — donne la priorité aux niveaux de répertoire les plus profonds, puis trie par taille de fichier et format, pour assurer un traitement par lots efficace et un débit maximal.
- 🎞️ **Métadonnées dynamiques HDR10+** : Conservation intégrale des métadonnées SMPTE 2094-40 via l'extraction de fichiers sidecar et l'injection SEI x265.
- 🌅 **Synthèse de gainmap HDR** : Synthétise automatiquement des tampons HDR linéaires 32 bits haute fidélité à partir des gainmaps HEIC Apple/Samsung/ISO, garantissant la préservation de la plage dynamique maximale lors de la conversion en JXL.
- **🔍 Sensibilisation aux métadonnées des fournisseurs** : Analyse intelligente des espaces de noms XMP spécifiques à Samsung/Google dans les fichiers HEIC pour assurer une préservation maximale du contexte.

## ⚠️ Avertissement et notes importantes

1. **La sécurité des données avant tout** : Pour éviter toute perte de données potentielle, il est fortement recommandé d'envoyer les fichiers traités vers un répertoire séparé (par exemple, en utilisant `-o /chemin/vers/sortie`) plutôt que d'utiliser la conversion sur place (`--in-place`), en particulier pour les médias irremplaçables.
2. **Logiciel bêta** : Bien que ce programme ait été largement testé, débogué et optimisé pour éviter toute perte de qualité ou de données (comme on peut le voir dans le journal des modifications), il n'est pas garanti qu'il soit exempt de bogues à 100 %. Veuillez signaler tout problème rencontré sur GitHub.
3. **Aperçu du calcul** : Bien qu'optimisé pour l'efficacité (en particulier sur Apple Silicon série M), le traitement de lots massifs en mode `--ultimate` peut toujours prendre du temps. Il occupera les ressources du système pendant une période prolongée ; veuillez planifier votre tâche en conséquence.
4. **Maturité de l'outil** : Les outils unifiés (`img`, `vid`) utilisent par défaut HEVC, qui est plus mature et stable que la stratégie AV1. Pour les tâches de production à haute fiabilité, HEVC (par défaut) est recommandé.

## 🔒 Confidentialité et intégrité des données

**Modern Format Boost** est construit sur une architecture "Local-First", garantissant que vos actifs créatifs restent entièrement sous votre contrôle.

- **Opération hors ligne** : Traitement 100 % hors ligne. Pas de télémétrie, de suivi d'utilisation ou d'appels vers le cloud. Les binaires de base ne contiennent aucun code lié au réseau.
- **Exécution sécurisée Rust** : Construit avec Rust pour éliminer nativement les bogues de corruption de mémoire (débordements de tampon, etc.).
- **Intégration sécurisée** : Tous les outils externes (FFmpeg, cjxl) sont invoqués via des primitives sécurisées et échappées — jamais via une exécution directe du shell — empêchant l'injection de commandes arbitraires.
- **Isolation des chemins** : La normalisation avancée empêche la traversée de répertoires et protège les fichiers système non liés.
- **Liste de blocage des chemins système** : Protections intégrées pour les répertoires système sensibles afin d'éviter les modifications accidentelles des fichiers du système d'exploitation.
- **Équilibrage dynamique des ressources** : Ajuste automatiquement les threads de traitement en fonction de la charge mémoire/CPU pour éviter les pannes système lors de tâches extrêmes.
- **Gardien complet des métadonnées** : Préservation stricte bit par bit des métadonnées EXIF, XMP, ICC et des horodatages du système de fichiers (btime/mtime).
- **Traitement sécurisé et isolation de session** :
  - **Zéro pollution de l'espace de travail** : Le suivi centralisé (`~/.mfb_progress/`) garde vos dossiers multimédias 100 % propres. Aucun fichier de métadonnées caché ne reste parmi vos photos/vidéos.
  - **Fichiers temporaires sans conflit** : Chaque fichier d'analyse intermédiaire (flux YUV, segments d'analyse) est identifié de manière unique par un UUID aléatoire. Cela empêche les collisions multi-instances et garantit une "Précision Chirurgicale" lors du nettoyage.
  - **Nettoyage au démarrage** : Qu'une tâche se termine avec succès ou soit reprise après une interruption, le système purge automatiquement toutes les données transitoires. Cette architecture "Auto-Nettoyante" garantit que votre disque reste exempt de restes de traitement abandonnés.
  - **Réinitialisation intelligente des points de contrôle** : Détecte automatiquement lorsqu'un utilisateur supprime manuellement le répertoire de sortie pour "recommencer", déclenchant une réinitialisation complète de l'état même en mode reprise.

## 🛠️ Technique approfondie : comment ça marche — Le pipeline

### Logique du pipeline d'images

Chaque fichier passe par un pipeline de décision en plusieurs étapes :

- **Étape 1 — Détection intelligente** : Analyse les tables DQT JPEG (détection de gainmap UltraHDR), les fragments WebP VP8L et les boîtes AVIF `av1C` au niveau binaire. Comprend désormais une **architecture sans dette** avec une conformité Clippy à 100 % et une analyse robuste des en-têtes `OpenEXR`/`JPEG 2000`.
- **Étape 2 — Routage et encodage** : JXL VarDCT pour JPEG (exact au bit près) ; mode modulaire pour les sources sans perte (PNG, WebP/AVIF/HEIC/EXR/JP2 sans perte).
- **Étape 3 — Chemin de détour** : Les formats comme TIFF/WebP/BMP/HEIC sont pré-traités en PNG 16 bits temporaires ou en **OpenEXR 32 bits** pour garantir la compatibilité `cjxl` sans perte de qualité (pipeline adapté 8/16/32 bits).
- **Étape 4 — Synthèse HEIC HDR** : Intercepte les fichiers HEIC avec gainmaps (Apple/Google) et synthétise des tampons HDR en lumière linéaire 32 bits via un pipeline d'escorte **OpenEXR** intermédiaire, fournissant une véritable sortie JXL HDR.
- **Étape 5 — Séparation statique/animée** : `img` rejette désormais strictement les actifs animés ou ambigus quant à l'animation. Les formats modernes animés sont délégués à `vid` au lieu d'être convertis dans le pipeline statique.
- **Étape 6 — Loop Intent v3** : La logique loop-intent partagée décide si le média animé doit rester de type GIF ou passer par le pipeline vidéo. La politique de livraison d'animation moderne compatible Apple est centralisée ici.

### Pipeline vidéo : recherche de saturation en trois phases

1. **Phase 1 : Recherche grossière GPU** : Recherche binaire sur les encodeurs matériels (VideoToolbox/NVENC) pour trouver le "point d'inflexion de la qualité".
2. **Phase 2 : Peaufinage CPU** : Mappe le CRF GPU à l'échelle `x265`. Utilise **Sprint & Backtrack** (double pas en cas de succès, réinitialisation à 0.1 en cas de dépassement).
3. **Phase 3 : Barrière de qualité 3D ultime** : Nécessite le passage simultané de VMAF-Y ≥ 86.0 (seuil de cohérence, relatif à la ligne de base dynamique), CAMBI ≤ 6.0 (banding) et PSNR-UV ≥ 30.0 dB (seuil de cohérence chromatique).
   - **Fusion Scoring** : Combine MS-SSIM + SSIM_All (poids 0.6/0.4) pour une analyse structurelle robuste.
   - **Chroma Guard** : Détecte automatiquement les petites résolutions qui feraient planter libvmaf MS-SSIM et repasse à un score Y uniquement pour garantir la fiabilité du traitement.
   - _Note : En mode `--ultimate`, la recherche ne se termine qu'après que **50 échantillons consécutifs** ne montrent aucun gain de qualité, garantissant une saturation absolue._

### Préservation des métadonnées et du HDR

- **HDR** : Préserve les primaires bt2020, TRC PQ/HLG et les métadonnées de l'écran de mastering.
- **Dolby Vision** : Extrait le RPU via `dovi_tool` et l'injecte dans x265 (conversion Profil 7 → 8.1).
- **macOS xattrs** : Préserve les balises Finder, la date d'ajout et les horodatages de création via `copyfile` et `setattrlist`.

### 🖥️ Temps d'exécution

![Runtime](../assets/runtime.png)

Temps d'exécution

### Les deux binaires

| Binaire   | Objectif                                   | Codec cible                   |
| --------- | ------------------------------------------ | ----------------------------- |
| **`img`** | Optimisation d'images statiques uniquement | → JXL / omission / ignorer    |
| **`vid`** | Optimisation vidéo et médias animés        | → HEVC / AV1 / GIF / omission |

Plus une **application macOS double-cliquable** (`Modern Format Boost.app`) pour le traitement par lots par glisser-déposer.

## 📉 Exemples de compression en situation réelle

| Format d'entrée     | Taille d'origine | Format de sortie | Taille de sortie | Économies | Méthode                                |
| :------------------ | :--------------- | :--------------- | :--------------- | :-------- | :------------------------------------- |
| Paysage JPEG        | 4.2 MB           | **JXL**          | 3.3 MB           | **~21%**  | Reconstruction de composant sans perte |
| Capture d'écran PNG | 2.5 MB           | **JXL**          | 1.1 MB           | **~56%**  | Modulaire d=0.0                        |
| Action Cam H.264    | 1.2 GB           | **HEVC**         | 480 MB           | **~60%**  | Recherche CRF GPU/CPU                  |
| WebP animé          | 15 MB            | **AV1 / HEVC**   | 1.8 MB           | **~88%**  | Transcodé en format vidéo              |

## 📊 Matrice de traitement

### Matrice de décision du format d'image

| Format d'entrée                              | Statique ? | Action dans `img run`           | Sortie         | Notes                                                |
| :------------------------------------------- | :--------: | :------------------------------ | :------------- | :--------------------------------------------------- |
| JPEG                                         |     ✅     | **Reconstruction sans perte**   | `.jxl`         | Exact au bit près `cjxl --lossless_jpeg=1`           |
| PNG / TIFF / BMP / autres fixes sans perte   |     ✅     | **Conversion sans perte**       | `.jxl`         | Peut utiliser le chemin de détour en premier         |
| WebP / AVIF / HEIC / HEIF (fixes sans perte) |     ✅     | **Convertir**                   | `.jxl`         | Les images fixes modernes sans perte sont autorisées |
| HEIC / HEIF avec gainmap                     |     ✅     | **Synthèse HDR**                | `.jxl`         | Le chemin gainmap synthétise le HDR linéaire         |
| Fixes anciens avec perte après validation    |     ✅     | **Conversion quasi sans perte** | `.jxl`         | Le chemin actuel par lots reste axé sur JXL          |
| Fixes WebP / AVIF / HEIC / HEIF avec perte   |     ✅     | **Omission**                    | garder origine | Éviter la perte générationnelle                      |
| Fixe JXL                                     |     ✅     | **Omission**                    | garder origine | Déjà optimal                                         |
| Toute image animée ou ambiguë                |     ❌     | **Ignorer**                     | aucune         | En dehors du domaine statique uniquement de `img`    |

### Note sur le routage `img`

Il existe aujourd'hui deux points d'entrée pour la conversion d'images dans le dépôt :

- `img run` / chemin CLI par lots dans `crates/img/src/main.rs`
- assistant de bibliothèque `smart_convert()` dans `crates/img/src/conversion_api.rs`

Ils ne sont **pas encore totalement alignés**.

- Le chemin CLI principal est actuellement orienté JXL pour les conversions statiques acceptées.
- L'ancien assistant API contient toujours une branche ciblant l'AVIF pour certains fichiers fixes avec perte non JPEG.
- Le CLI `img` analyse également `--codec`, mais dans le chemin par lots statique actuel, ce drapeau ne change **pas** matériellement les décisions de routage réelles.

Ce README documente d'abord le **comportement actuel du CLI/temps d'exécution**, car c'est ce que les utilisateurs rencontrent lors d'une utilisation normale par lots.

### Matrice de décision des médias animés

| Format d'entrée                                               | Propriétaire          | Action                  | Sortie                   | Notes                              |
| :------------------------------------------------------------ | :-------------------- | :---------------------- | :----------------------- | :--------------------------------- |
| GIF                                                           | `vid`                 | **Routage loop-intent** | `.gif` ou vidéo          | Chemin rapide GIF préservé         |
| WebP / AVIF / APNG / HEIC / HEIF / JXL animés                 | `vid`                 | **Routage loop-intent** | `.gif` / `.mov` / `.mp4` | `img` ignore ceux-là               |
| Animation moderne courte et silencieuse avec `--apple-compat` | `vid` + `loop_intent` | **Forcer GIF**          | `.gif`                   | Durée `<= 6s`                      |
| Animation moderne longue avec `--apple-compat`                | `vid` + `loop_intent` | **Pas de force GIF**    | cible vidéo              | Durée `>= 15s` reste de type vidéo |
| Animation moderne incertaine avec `--apple-compat`            | `vid` + `loop_intent` | **Forcer GIF**          | `.gif`                   | Repli de compatibilité             |

### Matrice de décision du codec vidéo

| Codec d'entrée                  | Mode normal             | Mode `--apple-compat`   | Notes                                          |
| :------------------------------ | :---------------------- | :---------------------- | :--------------------------------------------- |
| H.264 (AVC)                     | **Convertir**           | **Convertir**           | Pas d'omission préalable dans les deux modes   |
| VP9                             | **Omission**            | **Convertir en HEVC**   | Source incompatible Apple                      |
| AV1                             | **Omission**            | **Convertir en HEVC**   | Source incompatible Apple                      |
| VVC / AV2                       | **Omission**            | **Convertir en HEVC**   | Source incompatible Apple                      |
| HEVC (H.265)                    | **Omission**            | **Omission**            | Déjà une cible native Apple                    |
| ProRes / DNxHD / codecs anciens | **Convertir au besoin** | **Convertir au besoin** | La décision finale dépend toujours du résultat |

Les barrières de qualité et de taille s'appliquent toujours après le routage. En mode `--ultimate` et autres flux de correspondance de qualité, un itinéraire éligible à la conversion peut toujours se terminer par une omission si le fichier produit ne répond pas aux exigences de qualité/taille et qu'aucun repli autorisé ne s'applique.

### Stratégie de format HDR

| Type HDR          | Détection                                 | Stratégie de préservation                                                                        |
| :---------------- | :---------------------------------------- | :----------------------------------------------------------------------------------------------- |
| **HDR10**         | mastering_display + max_cll en side_data  | Métadonnées statiques entièrement préservées via les arguments FFmpeg                            |
| **HEIC Gainmap**  | Image auxiliaire HEIC (Apple/Samsung/ISO) | Synthétisé en HDR linéaire 32 bits -> JXL (True HDR)                                             |
| **UltraHDR JPEG** | JPEG APP1/APP2 + XMP (hdrgm:)             | Métadonnées préservées ; avertissement de perte de gainmap émis                                  |
| **HLG**           | color_trc = arib-std-b67                  | Primaires de couleur + TRC préservés                                                             |
| **Dolby Vision**  | DOVI side_data dans les flux/trames       | Extraction RPU via `dovi_tool` → injection x265 ; conversion Profil 7 → 8.1                      |
| **HDR10+**        | Métadonnées dynamiques ST2094-40          | Supporté via l'extraction sidecar `hdr10plus_tool` et l'injection x265 (conservation Profil A/B) |
| **SDR**           | Pas de marqueurs HDR                      | Traitement standard (yuv420p)                                                                    |

## ⬇️ Installation

### Binaires précompilés

Pour les utilisateurs qui ne souhaitent pas installer la chaîne d'outils Rust, vous pouvez télécharger les binaires précompilés à partir de la page
**[Releases](https://github.com/nowaytouse/modern-format-boost/releases)**.

```bash
# Exemple pour macOS ARM64
curl -LO https://github.com/nowaytouse/modern-format-boost/releases/latest/download/modern-format-boost-aarch64-apple-darwin.tar.gz
tar -xzf modern-format-boost-aarch64-apple-darwin.tar.gz

```

### Prérequis

| Outil                | Requis ?  | Usage                               | Commande d'installation                                                                     |
| :------------------- | :-------: | :---------------------------------- | :------------------------------------------------------------------------------------------ |
| **Rust** (nightly)   |    ✅     | Build et installation               | `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \| sh && rustup default nightly` |
| **FFmpeg** (5.0+)    |    ✅     | Traitement vidéo et métriques       | `brew install ffmpeg` / `apt install ffmpeg`                                                |
| **libjxl**           |    ✅     | Cœur d'encodage JXL                 | `brew install jpeg-xl`                                                                      |
| **ExifTool**         |    ✅     | Préservation des métadonnées        | `brew install exiftool`                                                                     |
| **ImageMagick**      |    ✅     | Pipeline de détour d'image          | `brew install imagemagick`                                                                  |
| **libwebp**          |    ✅     | Décodage natif WebP                 | `brew install webp`                                                                         |
| **libheif**          |    ✅     | Décodage HEIC/HEIF                  | `brew install libheif`                                                                      |
| **PostgreSQL** (12+) |    ✅     | Base de données de cache et qualité | `brew install postgresql pgvector` / `apt install postgresql`                               |
| **dovi_tool**        | Optionnel | Extraction de RPU Dolby Vision      | `cargo install dovi_tool`                                                                   |
| **hdr10plus_tool**   | Optionnel | Extraction de métadonnées HDR10+    | `cargo install hdr10plus_tool`                                                              |

#### macOS (Homebrew)

```bash
brew install ffmpeg jpeg-xl exiftool imagemagick webp libheif postgresql pgvector
```

#### Linux (Ubuntu/Debian)

```bash
sudo apt update && sudo apt install ffmpeg libimage-exiftool-perl imagemagick \
  webp libheif-dev postgresql postgresql-contrib postgresql-server-dev-all
# Sur Linux, pgvector doit être compilé et installé :
git clone --branch v0.5.1 https://github.com/pgvector/pgvector.git
cd pgvector
make && sudo make install
```

### 🗄️ Configuration de la Base de Données

Modern Format Boost utilise PostgreSQL (avec l'extension `pgvector`) comme moteur obligatoire de cache local et d'inférence de qualité. Les deux binaires `img` et `vid` se connectent à la base de données au démarrage et échoueront si le service n'est pas accessible.

#### 1. Démarrer le service PostgreSQL

- **macOS**: `brew services start postgresql`
- **Linux**: `sudo systemctl start postgresql`

#### 2. Créer la Base de Données

Le nom par défaut de la base de données est `modern_format_boost`. Créez-la avant de lancer les outils :

```bash
createdb modern_format_boost
```

Ou via SQL :

```sql
CREATE DATABASE modern_format_boost;
```

### Build à partir des sources

```bash
git clone https://github.com/nowaytouse/modern-format-boost.git
cd modern-format-boost
cargo build --release

```

## 🚀 Utilisation

### Démarrage rapide

```bash
# Conversion pour le chemin image
img run /chemin/vers/media
# Conversion pour le chemin vidéo
vid run /chemin/vers/media

# Pour utiliser la stratégie AV1 :
vid run --codec av1 /chemin/vers/media
```

### ⚡ Mode Rapide et Reprise Intelligente

Le **Mode Rapide** (`fastmode`) est conçu pour les flux de travail par glisser-déposer de l'interface utilisateur (`crates/dev/src/bin/drag_and_drop_processor.rs`). Il apporte des capacités de reprise de haute fiabilité :

- **Gestion de l'état `WorkingCopyMarker`** : Suit en toute sécurité l'état des processus partiels à travers les fermetures.
- **Détection de Sources Obsolètes** : Détecte automatiquement si les fichiers d'origine ont changé et force une nouvelle reconstruction, évitant les tentatives corrompues.
- **Protection Fail-Closed** : Une capture de contexte approfondie et la vérification `Blake3` garantissent aucune corruption de fichier lors des interruptions de `img run`.

### Options détaillées

- `--ultimate` : Recherche de qualité archive, **précision 0.01** (Haute qualité, temps de calcul élevé).
- `--apple-compat` : Active la compatibilité avec l'écosystème Apple (Live Photos/AAE). Par défaut sur le CLI ; `--no-apple-compat` le désactive.
- `--in-place` : Remplace les fichiers d'origine. **AVERTISSEMENT : IRRÉVERSIBLE.**
- `-o /dir` : Répertoire de sortie sécurisé (recommandé).
- `--verbose` : Affiche les journaux de traitement détaillés.
- `--no-recursive` : Ne descend pas dans les sous-répertoires.
- `--force-video` : Force le traitement des images animées comme de la vidéo, quel que soit le Loop Intent.

### Sous-commandes avancées

- `img cache-stats` : Affiche les statistiques du cache d'analyse SQLite.
- `vid strategy <file>` : Prévisualise la stratégie du pipeline pour un fichier spécifique.
- `img restore-timestamps` : Correction massive des dates de création basée sur des motifs de noms de fichiers (récupération de métadonnées).

### 💡 Note sur les instances multiples

**Modern Format Boost** prend nativement en charge l'exécution de plusieurs fenêtres/instances.

- **Traitement concurrent** : Permet d'exécuter plusieurs fenêtres pour gérer différents chemins de manière indépendante.
- **Note** : Veuillez adapter l'utilisation en fonction des performances d'E/S de votre matériel ; une concurrence excessive peut entraîner des conditions de concurrence dans le système de fichiers.

## 🏗️ Architecture

### CI/CD et Portes de Qualité

Modern Format Boost utilise un système strict de contrôle qualité pour garantir une architecture avec zéro dette technique :

- **Outillage Rust-first** : Les points d'entrée d'ingénierie sont des bins Rust sous `crates/dev/src/bin`; les originaux Python restent uniquement comme références de compatibilité jusqu'à confirmation de leur suppression sûre.
- **Vérification CI locale** : Avant de développer, assurez-vous d'utiliser `just fix-gate` ou `cargo run --locked -p dev --bin check_all -- --allow-non-nightly`. Il s'agit de la "Source Unique de Vérité" (SSOT) pour le formatage du code, l'analyse statique et les tests automatisés.
- **Durcissement et stabilité des tests** : "Fail Fast" a été désactivé pour collecter des informations de diagnostic complètes sur toutes les plateformes ; de plus, une capture de contexte profond a été ajoutée pour les états d'erreur d'image (comme les vérifications de restauration JPEG).

### Structure principale

- `crates/img/` : Optimiseur d'images statiques (`JXL` / omission / ignorer dans le chemin CLI actuel)
- `crates/vid/` : Optimiseur de vidéos et de médias animés (`HEVC` / `AV1` / `GIF`)
- `crates/foundation/` : Cœur intelligent (moteur hybride GPU/CPU, mapping HDR, métadonnées)
- `Modern Format Boost.app/` : Interface macOS glisser-déposer

## ❓ FAQ

**1. Le JXL est-il largement supporté ?**
Un support natif existe sous macOS 14+ / iOS 17+, Chrome 91+ et Firefox 128+. Cependant, il existe des problèmes connus dans l'écosystème :

- **Animations** : Les formats animés modernes (JXL/AV1/HEIF) ne s'affichent souvent pas comme des animations dans l'application Photos native de macOS/iOS ou le Finder (statique uniquement), surtout lorsqu'ils sont synchronisés via iCloud. Ils sont lus correctement dans les navigateurs modernes ou les outils spécialisés.
- **Vignettes** : Les fichiers JXL utilisant des **profils ICC en niveaux de gris** peuvent apparaître comme des **vignettes noires** dans le Finder/iCloud, bien qu'ils s'affichent parfaitement une fois ouverts.
  Le JXL reste le format supérieur pour l'archivage exact au bit près et le stockage HDR haute fidélité.

**2. Comment le HDR10+ est-il géré ?**
Entièrement supporté. Nous utilisons `hdr10plus_tool` pour extraire les métadonnées dynamiques SMPTE 2094-40 et les réinjecter dans le flux HEVC via le paramètre `--dhdr10-info` de `libx265`. Assurez-vous que l'outil est installé pour activer cette fonctionnalité.

**3. Pourquoi ignorer WebP/AVIF/HEIC ?**
Les images WebP/AVIF/HEIC/HEIF avec perte sont généralement ignorées car ce sont déjà des formats modernes avec perte, et les ré-encoder risquerait une perte générationnelle pour un bénéfice minime. Les exceptions importantes dans le code actuel sont :

- Les images fixes modernes sans perte peuvent toujours être converties en JXL.
- Les actifs gainmap HEIC/HEIF peuvent être synthétisés en JXL HDR.
- Les formats animés modernes ne sont pas gérés par `img` ; ils sont routés via `vid` et `loop_intent`.

---

## ⚖️ Licence

Sous **Licence MIT**.

## Dépendances d'exécution

Ce projet orchestre plusieurs géants de l'open source. Nous remercions leurs auteurs pour leurs contributions :

| Composant              | Licence    | Objectif                     |
| ---------------------- | ---------- | ---------------------------- |
| **FFmpeg**             | LGPL/GPL   | Traitement vidéo             |
| **libjxl** (cjxl/djxl) | BSD-3      | Encodage JPEG XL             |
| **ExifTool**           | Perl/GPL   | Préservation des métadonnées |
| **ImageMagick**        | Apache 2.0 | Chemin de détour d'image     |
| **SVT-AV1**            | BSD+Patent | Encodage AV1                 |
| **x265**               | GPL-2.0    | Encodage HEVC                |

Toutes les dépendances Rust sont gérées via `Cargo.toml` et relèvent de leurs licences open source respectives (MIT/Apache/BSD).
