# foundation Layout

`foundation` is grouped by domain under `src/`:

- `infra/`: core infra (errors, casting, logging, safety, path/thread/system).
- `convert/`: conversion and gate pipeline.
- `image/`: image detection, quality, JXL, loop/live-photo logic.
- `video/`: ffmpeg/ffprobe/explorer/video quality path.
- `quality/`: scoring, matching, regression, verifier.
- `media/`: metadata/date/hdr/xmp helpers.
- `db/`: quality DB + multi-scenario storage/query.
- `train/`: training runtime modules.
- `ui/`: progress and terminal UI.
- `algo/`: algorithm audit/runtime/seal internals.
- `tooling/`: command-builder wrappers.

Compatibility note: public paths stay exposed from crate root via `lib.rs` re-exports.
