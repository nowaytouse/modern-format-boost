-- Modern Format Boost — PostgreSQL Analysis Cache Schema
-- v3.0: Enhanced cache with content fingerprint + integrity verification

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

-- Cache metadata (schema versioning)
CREATE TABLE IF NOT EXISTS cache_metadata (
    key TEXT PRIMARY KEY,
    value INT NOT NULL
);

-- Indexes for performance and maintenance
CREATE INDEX IF NOT EXISTS idx_analysis_created ON analysis_records(created_at);
CREATE INDEX IF NOT EXISTS idx_quality_created ON quality_records(created_at);
CREATE INDEX IF NOT EXISTS idx_video_created ON video_records(created_at);
CREATE INDEX IF NOT EXISTS idx_path_hash ON path_index(content_hash);
