-- Modern Format Boost — PostgreSQL Analysis Cache Schema
-- v4.0: Strict payload cutover (table layout unchanged from v3)
-- Core analysis records for images
CREATE TABLE IF NOT EXISTS analysis_records (
  content_hash BYTEA PRIMARY KEY,
  file_size BIGINT NOT NULL,
  analysis_data BYTEA NOT NULL,
  created_at BIGINT NOT NULL,
  algorithm_version INT DEFAULT 1,
  content_fingerprint_hash BYTEA,
  data_checksum BIGINT
);

-- Quality analysis records for images
CREATE TABLE IF NOT EXISTS quality_records (
  content_hash BYTEA PRIMARY KEY,
  file_size BIGINT NOT NULL,
  analysis_data BYTEA NOT NULL,
  created_at BIGINT NOT NULL,
  algorithm_version INT DEFAULT 1,
  content_fingerprint_hash BYTEA,
  data_checksum BIGINT
);

-- Analysis records for videos
CREATE TABLE IF NOT EXISTS video_records (
  content_hash BYTEA PRIMARY KEY,
  file_size BIGINT NOT NULL,
  analysis_data BYTEA NOT NULL,
  created_at BIGINT NOT NULL,
  algorithm_version INT DEFAULT 1,
  content_fingerprint_hash BYTEA,
  data_checksum BIGINT
);

-- Path-based index for fast lookup (path, mtime, size)
CREATE TABLE IF NOT EXISTS path_index (
  file_path TEXT PRIMARY KEY,
  content_hash BYTEA NOT NULL,
  mtime BIGINT NOT NULL,
  file_size BIGINT NOT NULL,
  atime BIGINT DEFAULT 0,
  ctime BIGINT DEFAULT 0,
  btime BIGINT DEFAULT 0
);

-- Path-tree scan snapshots (batch image/video directory walks; M213)
CREATE TABLE IF NOT EXISTS path_tree_snapshots (
  cache_key TEXT PRIMARY KEY,
  media_kind TEXT NOT NULL,
  root_path TEXT NOT NULL,
  schema_version INT NOT NULL,
  payload JSONB NOT NULL,
  updated_at BIGINT NOT NULL
);

-- Cache metadata (schema versioning)
CREATE TABLE IF NOT EXISTS cache_metadata (key TEXT PRIMARY KEY, value INT NOT NULL);

-- Indexes for performance and maintenance
CREATE INDEX IF NOT EXISTS idx_analysis_created ON analysis_records (created_at);

CREATE INDEX IF NOT EXISTS idx_quality_created ON quality_records (created_at);

CREATE INDEX IF NOT EXISTS idx_video_created ON video_records (created_at);

CREATE INDEX IF NOT EXISTS idx_path_hash ON path_index (content_hash);

CREATE INDEX IF NOT EXISTS idx_path_tree_root ON path_tree_snapshots (root_path);

CREATE INDEX IF NOT EXISTS idx_path_tree_media_kind ON path_tree_snapshots (media_kind);
