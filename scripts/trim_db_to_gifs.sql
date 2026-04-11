-- Modern Format Boost - Database Cleanup Script
-- Purpose: Remove all samples that are not native GIFs or are static images.
-- This ensures the KNN database provides a clean reference for loop intent.

DELETE FROM samples WHERE is_native_gif = FALSE;

-- Optional: Re-calculate feature statistics after cleanup
-- (This will be triggered automatically on the next run by seed_positive_dataset_if_needed,
-- or we can manually trigger it if we had a dedicated tool).
