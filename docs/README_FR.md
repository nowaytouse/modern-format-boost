<p align="center">
  <img src="https://img.shields.io/badge/version-0.11.2-0969DA?style=for-the-badge&logo=rust&logoColor=white" alt="Version">
  <img src="https://img.shields.io/badge/rust-édition_2021-E57324?style=for-the-badge&logo=rust&logoColor=white" alt="Rust">
  <img src="https://img.shields.io/badge/plateforme-macOS_%7C_Linux_%7C_Windows-8257E5?style=for-the-badge&logo=apple&logoColor=white" alt="Plateforme">
  <img src="https://img.shields.io/badge/licence-MIT-00B265?style=for-the-badge" alt="Licence">
</p>

<h1 align="center">Modern Format Boost</h1>

<p align="center">
  <strong>Moteur d'optimisation multimédia de nouvelle génération — zéro perte de qualité, compression maximale.</strong><br>
  <em>下一代媒体优化引擎 — 画质零损失，体积最大压缩。</em>
</p>

---
# 📖 Français (French)

## Qu'est-ce que Modern Format Boost ?

**Modern Format Boost** est un moteur d'optimisation multimédia haute performance basé sur Rust. Il convertit les anciens formats d'image et de vidéo (JPEG, PNG, H.264, VP9…) vers des codecs de pointe (**JPEG XL** pour les images, **HEVC/AV1** pour les vidéos) — réalisant des réductions spectaculaires de la taille des fichiers tout en préservant ou même en égalant bit à bit la qualité originale.

Considérez-le comme un "compresseur intelligent" qui **ne dégrade jamais vos médias** :

- 📸 **Images** : JPEG → reconstruction sans perte JXL (identique au bit près, ~20 % plus petit) ; PNG/WebP/TIFF/HEIC → JXL.
- 🎬 **Vidéos** : H.264/VP9/AV1 → HEVC avec recherche de qualité accélérée par GPU.
- 🍎 **Écosystème Apple d'abord** : Mode de compatibilité Apple complet, détection de Live Photo, gestion des fichiers sidecar AAE.
- 🔒 **Gardien des métadonnées** : Préserve EXIF, XMP, profils ICC, horodatages de création, xattrs macOS, tags Finder.
- ⚡ **Optimisation de la vitesse perçue** : Stratégie de tri "Deep-First" — donne la priorité aux niveaux de répertoire les plus profonds, puis trie par taille de fichier et format, pour garantir un traitement par lots efficace et un débit maximal.
- 🎞️ **Métadonnées dynamiques HDR10+** : Rétention complète des métadonnées SMPTE 2094-40 via l'extraction de sidecars et l'injection SEI x265.
- 🌅 **Synthèse de Gainmap HDR** : Synthétise automatiquement des tampons HDR linéaires 32 bits haute fidélité à partir des Gainmaps HEIC Apple/Samsung/ISO, garantissant que la plage dynamique maximale est préservée lors de la conversion en JXL.
- **🔍 Sensibilité aux métadonnées des constructeurs** : Analyse intelligente des espaces de noms XMP spécifiques à Samsung/Google dans les fichiers HEIC pour assurer une préservation maximale du contexte.

## ⚠️ Avertissements et Notes Importantes

1. **La sécurité des données avant tout** : Pour éviter toute perte potentielle de données, il est fortement recommandé d'exporter les fichiers traités vers un répertoire séparé (ex: via `-o /chemin/vers/output`) plutôt que d'utiliser la conversion sur place (`--in-place`), surtout pour les médias irremplaçables.
2. **Logiciel en phase bêta** : Bien que ce programme ait été largement testé, débogué et optimisé pour éviter toute perte de qualité ou de données, il n'est pas garanti sans bug à 100 %. Veuillez signaler tout problème rencontré sur GitHub.
3. **Aperçu des performances** : Bien qu'optimisé pour l'efficacité (particulièrement sur Apple Silicon série M), le traitement de lots massifs en mode `--ultimate` peut prendre du temps et occuper les ressources système pendant une période prolongée.
4. **Maturité des outils** : Les outils basés sur HEVC (`img-hevc`, `vid-hevc`) sont actuellement plus matures et stables que les outils basés sur AV1 (`img-av1`, `vid-av1`). Pour les tâches de production haute fiabilité, les outils HEVC sont recommandés.

## 🔒 Confidentialité et Intégrité des Données

**Modern Format Boost** est conçu sur une architecture "Local-First", garantissant que vos actifs créatifs restent entièrement sous votre contrôle.

- **Opération hors ligne** : Traitement 100 % hors ligne. Pas de télémétrie, de suivi d'utilisation ou de pings vers le cloud. Les binaires principaux ne contiennent aucun code lié au réseau.
- **Exécution sécurisée par Rust** : Construit avec Rust pour éliminer nativement les bugs de corruption de mémoire (débordements de tampon, etc.).
- **Intégration sécurisée** : Tous les outils externes (FFmpeg, cjxl) sont invoqués via des primitives sécurisées et échappées — jamais via une exécution directe du shell — empêchant toute injection de commande arbitraire.
- **Isolation des chemins** : La normalisation avancée empêche la traversée de répertoires et protège les fichiers système non liés.
- **Liste de blocage des chemins système** : Protections intégrées pour les répertoires système sensibles afin d'éviter les modifications accidentelles de fichiers de l'OS.
- **Équilibrage dynamique des ressources** : Ajuste automatiquement les threads de traitement en fonction de la charge mémoire/CPU pour éviter les plantages système lors de tâches extrêmes.
- **Gardien complet des métadonnées** : Préservation stricte bit par bit des EXIF, XMP, ICC et des horodatages du système de fichiers (btime/mtime).
- **Traitement sécurisé et isolation des sessions** :
  - **Zéro pollution de l'espace de travail** : Le suivi centralisé (`~/.mfb_progress/`) garde vos dossiers multimédias 100 % propres. Aucun fichier de métadonnées caché ne subsiste.
  - **Fichiers temporaires sans conflit** : Chaque fichier d'analyse intermédiaire est identifié de manière unique par un UUID aléatoire. Cela évite les collisions multi-instances et assure une "précision chirurgicale" lors du nettoyage.
  - **Nettoyage au démarrage** : Qu'une tâche se termine avec succès ou reprenne après une interruption, le système purge automatiquement toutes les données transitoires.
- **Réinitialisation intelligente des points de contrôle** : Détecte automatiquement lorsqu'un utilisateur supprime manuellement le répertoire de sortie pour "repartir à zéro", déclenchant une réinitialisation complète de l'état même en mode reprise.

<details>
<summary><b>🛠️ Détails techniques : Comment ça marche — Le Pipeline</b></summary>

### Logique du pipeline d'images
Chaque fichier passe par un pipeline de décision en plusieurs étapes :
- **Étape 1 — Détection intelligente** : Analyse les tables JPEG DQT (détection de gainmap UltraHDR), les morceaux WebP VP8L et les boîtes AVIF `av1C` au niveau binaire.
- **Étape 2 — Routage et encodage** : JXL VarDCT pour le JPEG (bit-exact) ; mode modulaire pour les sources sans perte (PNG, WebP/AVIF/HEIC/EXR/JP2 sans perte).
- **Étape 3 — Chemin détourné** : Les formats comme TIFF/WebP/BMP/HEIC sont prétraités en PNG temporaires 16 bits ou en **OpenEXR 32 bits** pour garantir la compatibilité avec `cjxl` sans perte de qualité.
- **Étape 4 — Synthèse HDR HEIC** : Intercepte les fichiers HEIC avec Gainmaps (Apple/Google) et synthétise des tampons HDR en lumière linéaire 32 bits via un pipeline **OpenEXR** intermédiaire, fournissant une véritable sortie JXL HDR.
- **Étape 5 — Meme Score v3** : Évalue les GIF animés pour décider entre la conversion vidéo ou le maintien en GIF.

### Pipeline vidéo : Recherche de saturation en trois phases
1. **Phase 1 : Recherche grossière GPU** : Recherche binaire sur les encodeurs matériels (VideoToolbox/NVENC) pour trouver le "point d'inflexion de la qualité".
2. **Phase 2 : Ajustement fin CPU** : Mappe le CRF GPU à l'échelle `x265`. Utilise **Sprint & Backtrack** (double pas sur succès, retour à 0.1 sur dépassement).
3. **Phase 3 : Porte de qualité 3D ultime** : Nécessite un passage simultané de VMAF-Y ≥ 92.0, CAMBI ≤ 6.0 et PSNR-UV ≥ 34.0 dB.
   - **Fusion Scoring** : Combine MS-SSIM + SSIM_All pour une analyse structurelle robuste.
   - *Note : En mode `--ultimate`, la recherche ne s'arrête qu'après **50 échantillons consécutifs** sans gain de qualité.*

### Préservation HDR et Métadonnées
- **HDR** : Préserve les primaires bt2020, PQ/HLG TRC et les métadonnées Mastering Display.
- **Dolby Vision** : Extrait le RPU via `dovi_tool` et l'injecte dans x265 (conversion Profile 7 → 8.1).
- **macOS xattrs** : Préserve les tags Finder et les dates de création via `copyfile`.
</details>

### 🖥️ Interface
![Interface](assets/runtime.png)
<p align="center">Interface</p>

### Les quatre binaires

| Binaire | Objectif | Codec cible |
|--------|---------|-------------|
| **`img-hevc`** | Optimisation d'image | → JXL (statique) / HEVC (animé) |
| **`img-av1`** | Optimización d'image | → JXL (statique) / AV1 (animé) |
| **`vid-hevc`** | Optimisation vidéo | → HEVC / H.265 |
| **`vid-av1`** | Optimisation vidéo | → AV1 / SVT-AV1 |

### 📉 Exemples de compression réelle

| Format d'entrée | Taille originale | Format de sortie | Taille de sortie | Économie | Méthode |
|:---|:---|:---|:---|:---|:---|
| Paysage JPEG | 4.2 MB | **JXL** | 3.3 MB | **~21%** | Reconstruction sans perte |
| Capture PNG | 2.5 MB | **JXL** | 1.1 MB | **~56%** | Modular d=0.0 |
| Action Cam H.264 | 1.2 GB | **HEVC** | 480 MB | **~60%** | Recherche CRF GPU/CPU |

## ⬇️ Installation

### Binaires pré-compilés
Vous pouvez télécharger les binaires pré-compilés depuis la page **[Releases](https://github.com/nowaytouse/modern-format-boost/releases)**.

```bash
# Pour macOS/Linux (exemple macOS ARM64)
curl -LO https://github.com/nowaytouse/modern-format-boost/releases/latest/download/modern-format-boost-aarch64-apple-darwin.tar.gz
tar -xzf modern-format-boost-aarch64-apple-darwin.tar.gz
```

---
# ⚖️ Licence
Sous **Licence MIT**.
