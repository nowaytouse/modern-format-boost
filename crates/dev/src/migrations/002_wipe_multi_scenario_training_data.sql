-- DESTRUCTIVE: wipes all multi-scenario training samples and inference logs.
--
-- Requires explicit confirmation (fail-closed by default):
--   psql "$MFB_PG_CONNSTR" -v ON_ERROR_STOP=1 \
--     -c "SET mfb.confirm_training_wipe = 'YES'" \
--     -f migrations/002_wipe_multi_scenario_training_data.sql
--
-- Keeps schema + metadata rows; resets sample_count and feature_stats.

DO $$
BEGIN
    IF current_setting('mfb.confirm_training_wipe', true) IS DISTINCT FROM 'YES' THEN
        RAISE EXCEPTION
            'Refusing wipe: run psql -c "SET mfb.confirm_training_wipe = ''YES''" before this script';
    END IF;
END
$$;

BEGIN;

TRUNCATE TABLE
    video_quality_inference_log,
    animated_image_quality_inference_log,
    image_quality_inference_log,
    loop_intent_inference_log,
    video_quality_samples,
    animated_image_quality_samples,
    image_quality_samples,
    loop_samples
RESTART IDENTITY CASCADE;

UPDATE multi_scenario_metadata SET
    sample_count = 0,
    feature_stats = '{}'::jsonb,
    last_updated = CURRENT_TIMESTAMP;

COMMIT;
