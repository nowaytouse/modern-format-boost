-- Multi-Scenario Embedding Architecture Migration
-- New-schema-only deployment:
-- - Loop Intent (`loop_samples`)
-- - Image Quality (`image_quality_samples`)
-- - Animated Image Quality (`animated_image_quality_samples`)
-- - Video Quality (new table)
-- ============================================================================
-- PHASE 1: Ensure pgvector extension
-- ============================================================================
CREATE EXTENSION IF NOT EXISTS vector;

DO $$
BEGIN
    IF EXISTS (
        SELECT 1
        FROM information_schema.tables
        WHERE table_schema = 'public'
          AND table_name = 'gif_quality_samples'
    ) THEN
        RAISE EXCEPTION
            'Legacy schema object detected: gif_quality_samples. Remove or rename legacy animated-image schema objects before applying the strict animated_image_quality schema.';
    END IF;

    IF EXISTS (
        SELECT 1
        FROM information_schema.tables
        WHERE table_schema = 'public'
          AND table_name = 'gif_quality_inference_log'
    ) THEN
        RAISE EXCEPTION
            'Legacy schema object detected: gif_quality_inference_log. Remove or rename legacy animated-image schema objects before applying the strict animated_image_quality schema.';
    END IF;

    IF EXISTS (
        SELECT 1 FROM pg_class
        WHERE relkind = 'S' AND relname = 'gif_quality_samples_id_seq'
    ) THEN
        RAISE EXCEPTION
            'Legacy schema object detected: gif_quality_samples_id_seq. Remove or rename legacy animated-image schema objects before applying the strict animated_image_quality schema.';
    END IF;

    IF EXISTS (
        SELECT 1 FROM pg_class
        WHERE relkind = 'S' AND relname = 'gif_quality_inference_log_id_seq'
    ) THEN
        RAISE EXCEPTION
            'Legacy schema object detected: gif_quality_inference_log_id_seq. Remove or rename legacy animated-image schema objects before applying the strict animated_image_quality schema.';
    END IF;

    IF EXISTS (
        SELECT 1 FROM pg_class
        WHERE relkind = 'i' AND relname = 'idx_gif_quality_blake3'
    ) THEN
        RAISE EXCEPTION
            'Legacy schema object detected: idx_gif_quality_blake3. Remove or rename legacy animated-image schema objects before applying the strict animated_image_quality schema.';
    END IF;

    IF EXISTS (
        SELECT 1 FROM pg_class
        WHERE relkind = 'i' AND relname = 'idx_gif_quality_hnsw'
    ) THEN
        RAISE EXCEPTION
            'Legacy schema object detected: idx_gif_quality_hnsw. Remove or rename legacy animated-image schema objects before applying the strict animated_image_quality schema.';
    END IF;

    IF EXISTS (
        SELECT 1
        FROM pg_constraint
        WHERE conname = 'gif_quality_samples_quality_score_check'
    ) THEN
        RAISE EXCEPTION
            'Legacy schema object detected: gif_quality_samples_quality_score_check. Remove or rename legacy animated-image schema objects before applying the strict animated_image_quality schema.';
    END IF;

    IF EXISTS (
        SELECT 1 FROM pg_trigger
        WHERE tgname = 'trg_sync_gif_quality_samples_metadata'
    ) THEN
        RAISE EXCEPTION
            'Legacy schema object detected: trg_sync_gif_quality_samples_metadata. Remove or rename legacy animated-image schema objects before applying the strict animated_image_quality schema.';
    END IF;

    IF EXISTS (
        SELECT 1 FROM pg_trigger
        WHERE tgname = 'trg_sync_gif_quality_samples_metadata_truncate'
    ) THEN
        RAISE EXCEPTION
            'Legacy schema object detected: trg_sync_gif_quality_samples_metadata_truncate. Remove or rename legacy animated-image schema objects before applying the strict animated_image_quality schema.';
    END IF;

    IF EXISTS (
        SELECT 1
        FROM information_schema.tables
        WHERE table_schema = 'public'
          AND table_name = 'multi_scenario_metadata'
    ) THEN
        IF EXISTS (
            SELECT 1
            FROM multi_scenario_metadata
            WHERE scenario = 'gif_quality'
        ) THEN
            RAISE EXCEPTION
                'Legacy metadata row detected: scenario=gif_quality. Remove legacy animated-image metadata before applying the strict animated_image_quality schema.';
        END IF;
    END IF;
END;
$$;

-- ============================================================================
-- PHASE 2: Create new scenario-specific tables
-- ============================================================================
-- 2.1 Loop Intent Samples
CREATE TABLE IF NOT EXISTS loop_samples (
  id BIGSERIAL PRIMARY KEY,
  blake3 BYTEA UNIQUE NOT NULL,
  source_path TEXT,
  file_name TEXT,
  -- Physical features
  width INTEGER NOT NULL,
  height INTEGER NOT NULL,
  duration_secs DOUBLE PRECISION NOT NULL,
  frame_count BIGINT NOT NULL,
  fps DOUBLE PRECISION,
  file_size_bytes BIGINT NOT NULL,
  -- Loop-specific metrics
  motion_periodicity DOUBLE PRECISION,
  temporal_jitter DOUBLE PRECISION,
  motion_gini DOUBLE PRECISION,
  loop_closure_score DOUBLE PRECISION,
  cadence_score DOUBLE PRECISION,
  -- Embedding (261D optimized for loop intent: 36 learned dims + 225 physics dims)
  embedding VECTOR (261),
  -- Metadata
  label SMALLINT DEFAULT 0, -- 0=non-loop, 1=loop, 2=video-loop
  labeled_by TEXT,
  created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
  metadata JSONB DEFAULT '{}'::jsonb
);

CREATE INDEX IF NOT EXISTS idx_loop_samples_blake3 ON loop_samples (blake3);

CREATE INDEX IF NOT EXISTS idx_loop_samples_hnsw ON loop_samples USING hnsw (embedding vector_l2_ops);

-- 2.2 Image Quality Samples
CREATE TABLE IF NOT EXISTS image_quality_samples (
  id BIGSERIAL PRIMARY KEY,
  blake3 BYTEA UNIQUE NOT NULL,
  source_path TEXT,
  -- Physical features
  width INTEGER NOT NULL,
  height INTEGER NOT NULL,
  file_size_bytes BIGINT NOT NULL,
  format TEXT NOT NULL,
  total_pixels BIGINT,
  -- Quality metrics
  entropy DOUBLE PRECISION NOT NULL,
  compression_ratio DOUBLE PRECISION NOT NULL,
  spatial_bpp DOUBLE PRECISION NOT NULL,
  is_lossless BOOLEAN NOT NULL,
  -- Embedding (256D for image quality)
  embedding VECTOR (256),
  -- Training label
  quality_label TEXT, -- 'png-high', 'png-low', 'modern-high', 'modern-low'
  quality_score REAL NOT NULL CHECK (
    quality_score = quality_score
    AND quality_score >= 0.0
    AND quality_score <= 1.0
  ),
  labeled_by TEXT DEFAULT 'manual_training',
  created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
  metadata JSONB DEFAULT '{}'::jsonb
);

CREATE INDEX IF NOT EXISTS idx_image_quality_blake3 ON image_quality_samples (blake3);

CREATE INDEX IF NOT EXISTS idx_image_quality_hnsw ON image_quality_samples USING hnsw (embedding vector_l2_ops);

-- 2.3 Animated Image Quality Samples
CREATE TABLE IF NOT EXISTS animated_image_quality_samples (
  id BIGSERIAL PRIMARY KEY,
  blake3 BYTEA UNIQUE NOT NULL,
  source_path TEXT,
  -- Physical features
  width INTEGER NOT NULL,
  height INTEGER NOT NULL,
  frame_count BIGINT NOT NULL,
  duration_secs DOUBLE PRECISION NOT NULL,
  fps DOUBLE PRECISION,
  -- Animated-image-specific metrics
  palette_size INTEGER,
  palette_depth DOUBLE PRECISION,
  animation_smoothness DOUBLE PRECISION,
  frame_delay_variation DOUBLE PRECISION,
  -- Embedding (256D, 225 reference-frame physics dims + 31 animated-image dims)
  embedding VECTOR (256),
  -- Training label
  quality_score REAL NOT NULL CHECK (
    quality_score = quality_score
    AND quality_score >= 0.0
    AND quality_score <= 1.0
  ),
  is_meme BOOLEAN DEFAULT FALSE,
  labeled_by TEXT DEFAULT 'manual_training',
  created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
  metadata JSONB DEFAULT '{}'::jsonb
);

CREATE INDEX IF NOT EXISTS idx_animated_image_quality_blake3 ON animated_image_quality_samples (blake3);

CREATE INDEX IF NOT EXISTS idx_animated_image_quality_hnsw ON animated_image_quality_samples USING hnsw (embedding vector_l2_ops);

-- 2.4 Video Quality Samples
CREATE TABLE IF NOT EXISTS video_quality_samples (
  id BIGSERIAL PRIMARY KEY,
  blake3 BYTEA UNIQUE NOT NULL,
  source_path TEXT,
  -- Physical features
  width INTEGER NOT NULL,
  height INTEGER NOT NULL,
  duration_secs DOUBLE PRECISION NOT NULL,
  frame_count BIGINT NOT NULL,
  fps DOUBLE PRECISION,
  file_size_bytes BIGINT NOT NULL,
  codec TEXT NOT NULL,
  bitrate_mbps REAL,
  -- Video quality metrics (real container/runtime signals)
  bit_depth SMALLINT,
  has_audio BOOLEAN NOT NULL DEFAULT FALSE,
  is_variable_frame_rate BOOLEAN NOT NULL DEFAULT FALSE,
  is_hdr BOOLEAN NOT NULL DEFAULT FALSE,
  motion_intensity REAL,
  temporal_stability REAL,
  -- Embedding (256D for video quality)
  embedding VECTOR (256),
  -- Training label
  quality_score REAL NOT NULL CHECK (
    quality_score = quality_score
    AND quality_score >= 0.0
    AND quality_score <= 1.0
  ),
  labeled_by TEXT DEFAULT 'manual_training',
  created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
  metadata JSONB DEFAULT '{}'::jsonb
);

CREATE INDEX IF NOT EXISTS idx_video_quality_blake3 ON video_quality_samples (blake3);

CREATE INDEX IF NOT EXISTS idx_video_quality_hnsw ON video_quality_samples USING hnsw (embedding vector_l2_ops);

-- Idempotent column upgrades for older deployments that already had the table.
ALTER TABLE video_quality_samples
ADD COLUMN IF NOT EXISTS frame_count BIGINT NOT NULL DEFAULT 0;

ALTER TABLE video_quality_samples
ADD COLUMN IF NOT EXISTS file_size_bytes BIGINT NOT NULL DEFAULT 0;

ALTER TABLE video_quality_samples
ADD COLUMN IF NOT EXISTS codec TEXT NOT NULL DEFAULT 'unknown';

ALTER TABLE video_quality_samples
ADD COLUMN IF NOT EXISTS bit_depth SMALLINT;

ALTER TABLE video_quality_samples
ADD COLUMN IF NOT EXISTS has_audio BOOLEAN NOT NULL DEFAULT FALSE;

ALTER TABLE video_quality_samples
ADD COLUMN IF NOT EXISTS is_variable_frame_rate BOOLEAN NOT NULL DEFAULT FALSE;

ALTER TABLE video_quality_samples
ADD COLUMN IF NOT EXISTS is_hdr BOOLEAN NOT NULL DEFAULT FALSE;

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint
        WHERE conname = 'loop_samples_media_metadata_check'
          AND conrelid = 'loop_samples'::regclass
    ) THEN
        ALTER TABLE loop_samples
        ADD CONSTRAINT loop_samples_media_metadata_check
        CHECK (
            width > 0
            AND height > 0
            AND duration_secs = duration_secs
            AND duration_secs > 0.0
            AND duration_secs < 'Infinity'::double precision
            AND frame_count > 0
            AND file_size_bytes > 0
            AND (fps IS NULL OR (
                fps = fps
                AND fps > 0.0
                AND fps < 'Infinity'::double precision
            ))
        ) NOT VALID;
    END IF;

    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint
        WHERE conname = 'image_quality_samples_media_metadata_check'
          AND conrelid = 'image_quality_samples'::regclass
    ) THEN
        ALTER TABLE image_quality_samples
        ADD CONSTRAINT image_quality_samples_media_metadata_check
        CHECK (
            width > 0
            AND height > 0
            AND file_size_bytes > 0
            AND LENGTH(BTRIM(format)) > 0
            AND (total_pixels IS NULL OR total_pixels > 0)
            AND entropy = entropy
            AND entropy >= 0.0
            AND entropy < 'Infinity'::double precision
            AND compression_ratio = compression_ratio
            AND compression_ratio > 0.0
            AND compression_ratio < 'Infinity'::double precision
            AND spatial_bpp = spatial_bpp
            AND spatial_bpp > 0.0
            AND spatial_bpp < 'Infinity'::double precision
        ) NOT VALID;
    END IF;

    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint
        WHERE conname = 'animated_image_quality_samples_media_metadata_check'
          AND conrelid = 'animated_image_quality_samples'::regclass
    ) THEN
        ALTER TABLE animated_image_quality_samples
        ADD CONSTRAINT animated_image_quality_samples_media_metadata_check
        CHECK (
            width > 0
            AND height > 0
            AND frame_count > 1
            AND duration_secs = duration_secs
            AND duration_secs > 0.0
            AND duration_secs < 'Infinity'::double precision
            AND (fps IS NULL OR (
                fps = fps
                AND fps > 0.0
                AND fps < 'Infinity'::double precision
            ))
        ) NOT VALID;
    END IF;

    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint
        WHERE conname = 'video_quality_samples_media_metadata_check'
          AND conrelid = 'video_quality_samples'::regclass
    ) THEN
        ALTER TABLE video_quality_samples
        ADD CONSTRAINT video_quality_samples_media_metadata_check
        CHECK (
            width > 0
            AND height > 0
            AND duration_secs = duration_secs
            AND duration_secs > 0.0
            AND duration_secs < 'Infinity'::double precision
            AND frame_count > 0
            AND file_size_bytes > 0
            AND LENGTH(BTRIM(codec)) > 0
            AND LOWER(BTRIM(codec)) <> 'unknown'
            AND (fps IS NULL OR (
                fps = fps
                AND fps > 0.0
                AND fps < 'Infinity'::double precision
            ))
        ) NOT VALID;
    END IF;
END;
$$;

ALTER TABLE loop_samples VALIDATE CONSTRAINT loop_samples_media_metadata_check;

ALTER TABLE image_quality_samples VALIDATE CONSTRAINT image_quality_samples_media_metadata_check;

ALTER TABLE animated_image_quality_samples VALIDATE CONSTRAINT animated_image_quality_samples_media_metadata_check;

ALTER TABLE video_quality_samples VALIDATE CONSTRAINT video_quality_samples_media_metadata_check;

-- ============================================================================
-- PHASE 3: Inference logging tables (one per scenario)
-- ============================================================================
CREATE TABLE IF NOT EXISTS loop_intent_inference_log (
  id BIGSERIAL PRIMARY KEY,
  blake3 BYTEA,
  source_path TEXT,
  knn_score DOUBLE PRECISION,
  knn_confidence DOUBLE PRECISION,
  knn_neighbor_count INTEGER,
  final_verdict TEXT NOT NULL DEFAULT 'unknown',
  created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS image_quality_inference_log (
  id BIGSERIAL PRIMARY KEY,
  blake3 BYTEA,
  source_path TEXT,
  knn_score DOUBLE PRECISION,
  knn_confidence DOUBLE PRECISION,
  knn_neighbor_count INTEGER,
  bpp_fallback_score DOUBLE PRECISION,
  final_verdict TEXT NOT NULL DEFAULT 'low',
  created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS animated_image_quality_inference_log (
  id BIGSERIAL PRIMARY KEY,
  blake3 BYTEA,
  source_path TEXT,
  knn_score DOUBLE PRECISION,
  knn_confidence DOUBLE PRECISION,
  knn_neighbor_count INTEGER,
  final_verdict TEXT NOT NULL DEFAULT 'unknown',
  created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS video_quality_inference_log (
  id BIGSERIAL PRIMARY KEY,
  blake3 BYTEA,
  source_path TEXT,
  knn_score DOUBLE PRECISION,
  knn_confidence DOUBLE PRECISION,
  knn_neighbor_count INTEGER,
  final_verdict TEXT NOT NULL DEFAULT 'unknown',
  created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

-- ============================================================================
-- PHASE 4: Metadata tables for tracking
-- ============================================================================
CREATE TABLE IF NOT EXISTS multi_scenario_metadata (
  scenario TEXT PRIMARY KEY, -- 'loop_intent', 'image_quality', 'animated_image_quality', 'video_quality'
  table_name TEXT NOT NULL,
  embedding_dimension INTEGER NOT NULL,
  sample_count BIGINT DEFAULT 0,
  last_updated TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
  feature_stats JSONB DEFAULT '{}'::jsonb,
  collection_stats JSONB DEFAULT '{}'::jsonb
);

-- Initialize metadata
INSERT INTO
  multi_scenario_metadata (scenario, table_name, embedding_dimension)
VALUES
  ('loop_intent', 'loop_samples', 261),
  ('image_quality', 'image_quality_samples', 256),
  (
    'animated_image_quality',
    'animated_image_quality_samples',
    256
  ),
  ('video_quality', 'video_quality_samples', 256)
ON CONFLICT (scenario) DO NOTHING;

CREATE OR REPLACE FUNCTION normalize_image_quality_score () RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.quality_score IS NULL THEN
        NEW.quality_score := CASE
            WHEN NEW.quality_label IN ('png-high', 'modern-high') THEN 1.0
            WHEN NEW.quality_label IN ('png-low', 'modern-low') THEN 0.0
            ELSE NEW.quality_score
        END;
    END IF;
    RETURN NEW;
END;
$$;

DROP TRIGGER IF EXISTS trg_normalize_image_quality_score ON image_quality_samples;

CREATE TRIGGER trg_normalize_image_quality_score
BEFORE INSERT OR UPDATE ON image_quality_samples FOR EACH ROW
EXECUTE FUNCTION normalize_image_quality_score ();

CREATE OR REPLACE FUNCTION sync_multi_scenario_metadata_sample_count () RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'INSERT' THEN
        UPDATE multi_scenario_metadata
        SET sample_count = sample_count + 1,
            last_updated = CURRENT_TIMESTAMP
        WHERE table_name = TG_TABLE_NAME;
        RETURN NEW;
    ELSIF TG_OP = 'DELETE' THEN
        UPDATE multi_scenario_metadata
        SET sample_count = GREATEST(sample_count - 1, 0),
            last_updated = CURRENT_TIMESTAMP
        WHERE table_name = TG_TABLE_NAME;
        RETURN OLD;
    END IF;

    UPDATE multi_scenario_metadata
    SET last_updated = CURRENT_TIMESTAMP
    WHERE table_name = TG_TABLE_NAME;
    RETURN NEW;
END;
$$;

CREATE OR REPLACE FUNCTION sync_multi_scenario_metadata_on_truncate () RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    UPDATE multi_scenario_metadata
    SET sample_count = 0,
        last_updated = CURRENT_TIMESTAMP
    WHERE table_name = TG_TABLE_NAME;
    RETURN NULL;
END;
$$;

DROP TRIGGER IF EXISTS trg_sync_loop_samples_metadata ON loop_samples;

CREATE TRIGGER trg_sync_loop_samples_metadata
AFTER INSERT OR UPDATE OR DELETE ON loop_samples FOR EACH ROW
EXECUTE FUNCTION sync_multi_scenario_metadata_sample_count ();

DROP TRIGGER IF EXISTS trg_sync_image_quality_samples_metadata ON image_quality_samples;

CREATE TRIGGER trg_sync_image_quality_samples_metadata
AFTER INSERT OR UPDATE OR DELETE ON image_quality_samples FOR EACH ROW
EXECUTE FUNCTION sync_multi_scenario_metadata_sample_count ();

DROP TRIGGER IF EXISTS trg_sync_animated_image_quality_samples_metadata ON animated_image_quality_samples;

CREATE TRIGGER trg_sync_animated_image_quality_samples_metadata
AFTER INSERT OR UPDATE OR DELETE ON animated_image_quality_samples FOR EACH ROW
EXECUTE FUNCTION sync_multi_scenario_metadata_sample_count ();

DROP TRIGGER IF EXISTS trg_sync_video_quality_samples_metadata ON video_quality_samples;

CREATE TRIGGER trg_sync_video_quality_samples_metadata
AFTER INSERT OR UPDATE OR DELETE ON video_quality_samples FOR EACH ROW
EXECUTE FUNCTION sync_multi_scenario_metadata_sample_count ();

DROP TRIGGER IF EXISTS trg_sync_loop_samples_metadata_truncate ON loop_samples;

CREATE TRIGGER trg_sync_loop_samples_metadata_truncate
AFTER TRUNCATE ON loop_samples FOR EACH STATEMENT
EXECUTE FUNCTION sync_multi_scenario_metadata_on_truncate ();

DROP TRIGGER IF EXISTS trg_sync_image_quality_samples_metadata_truncate ON image_quality_samples;

CREATE TRIGGER trg_sync_image_quality_samples_metadata_truncate
AFTER TRUNCATE ON image_quality_samples FOR EACH STATEMENT
EXECUTE FUNCTION sync_multi_scenario_metadata_on_truncate ();

DROP TRIGGER IF EXISTS trg_sync_animated_image_quality_samples_metadata_truncate ON animated_image_quality_samples;

CREATE TRIGGER trg_sync_animated_image_quality_samples_metadata_truncate
AFTER TRUNCATE ON animated_image_quality_samples FOR EACH STATEMENT
EXECUTE FUNCTION sync_multi_scenario_metadata_on_truncate ();

DROP TRIGGER IF EXISTS trg_sync_video_quality_samples_metadata_truncate ON video_quality_samples;

CREATE TRIGGER trg_sync_video_quality_samples_metadata_truncate
AFTER TRUNCATE ON video_quality_samples FOR EACH STATEMENT
EXECUTE FUNCTION sync_multi_scenario_metadata_on_truncate ();

-- ============================================================================
-- Migration Complete
-- ============================================================================
SELECT
  'Multi-Scenario Embedding Schema Created Successfully!' as status;
