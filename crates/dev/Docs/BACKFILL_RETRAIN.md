# Backfill & Retrain Runbook

Steps performed by the agent:

1. Backfill `directory_meme_score` and `directory_meme_hint`:
   - Command:
     MFB_PG_CONNSTR="host=localhost dbname=modern_format_boost" \
     ./.venv_training/bin/python crates/dev/scripts/backfill_directory_scores.py

   - Notes: The script will `ALTER TABLE samples` to add the `directory_meme_score` (DOUBLE PRECISION, default 0.5) and `directory_meme_hint` (BOOLEAN) if they do not exist, then recompute scores from `source_path` and update rows in-place. Backup your DB first.

2. Retrain model:
   - Command:
     ./.venv_training/bin/python crates/dev/scripts/training_pipeline.py train --connstr "host=localhost dbname=modern_format_boost"

   - Notes: Training uses the numeric `directory_meme_score` in `FEATURE_COLUMNS`. The dataset used in the recent run was highly imbalanced (1798 `high` vs 1 `low`), so consider collecting or reweighting low-class examples.

3. Run tests:
   - Command:
     cargo test -p shared_utils

   - Notes: Integration tests rely on `shared_utils/test/videos` and `shared_utils/test/gifs` assets. The agent created minimal placeholder videos/gifs to satisfy CI.

Next steps / recommendations:

- Run the backfill against your production DB only after making a backup.
- Retrain after backfill and verify model behavior (ablation tests, confusion matrix).
- Consider collecting more low-class samples or using resampling/weights to mitigate imbalance.
- When comfortable, remove `directory_meme_hint` from ingestion and training (the code still writes a conservative boolean for compatibility).

Files modified/created by the agent:

- `crates/dev/scripts/backfill_directory_scores.py` (added migration to create columns)
- `crates/dev/scripts/training_pipeline.py` (no changes; used for training)
- `shared_utils/test/gifs/test_pattern.gif` (placeholder)
- `shared_utils/test/gifs/test_simple.gif` (existing/placeholder)
- `shared_utils/test/MEDIA_MANIFEST.md` (existing)
- `shared_utils/test/videos/*` (placeholder video files added)
- `docs/BACKFILL_RETRAIN.md` (this file)

If you want, I can now:

- Run backfill/train on a different connection string you provide.
- Re-run tests under a different target or create larger test media.
