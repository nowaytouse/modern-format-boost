# Modern Format Boost

Premium media optimizer with intelligent quality matching and format conversion.

## Features

- **Smart Conversion**: Auto-detects optimal format (HEIC/JXL for images, HEVC for videos)
- **Quality Matching**: Preserves visual quality while reducing file size
- **Metadata Preservation**: Keeps all EXIF, timestamps, and macOS attributes
- **iCloud Compatibility**: Auto-fixes JXL containers for Photos.app
- **No Data Loss**: Copies unsupported files, merges XMP sidecars

## v7.10 - JXL Container Fix

Automatically converts JXL ISOBMFF containers to bare codestream format for iCloud Photos compatibility.

- Detects container format JXL files
- Extracts codestream without re-encoding (preserves quality and size)
- Maintains all metadata and timestamps
- Creates backups before modification (`.container.backup`)
- Original files preserved as backups (can be cleaned up after verification)

### Backup Management

After conversion, original container files are saved with `.container.backup` extension.

To remove backups after verifying converted files work:
```bash
./scripts/cleanup_jxl_backups.sh /path/to/directory
```

## Usage

Drag folder onto `Modern Format Boost.app` or use scripts:

```bash
# Process directory
./scripts/drag_and_drop_processor.sh /path/to/media

# Fix JXL containers only
./scripts/fix_jxl_containers.sh /path/to/jxl/files
```

## Requirements

- macOS (Apple Silicon or Intel)
- Homebrew packages: `jpeg-xl`, `exiftool`
- Rust toolchain for building

## Build

```bash
./scripts/smart_build.sh
```

## License

See LICENSE file for details.
