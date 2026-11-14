-- Analytics views for audit-only inference_log rows (runtime verdict in JSON snapshot).
-- Idempotent: safe to re-run after schema init.
-- Loop view columns extended in 004_loop_inference_posterior_views.sql (run after this file).
CREATE OR REPLACE VIEW loop_inference_log_effective AS
SELECT
  id,
  blake3,
  source_path,
  COALESCE(
    signal_snapshot ->> 'runtime_final_verdict',
    final_verdict
  ) AS effective_final_verdict,
  COALESCE(
    signal_snapshot ->> 'runtime_decision_reason',
    decision_reason
  ) AS effective_decision_reason,
  COALESCE(
    NULLIF(
      signal_snapshot ->> 'runtime_final_probability',
      ''
    )::double precision,
    final_probability
  ) AS effective_final_probability,
  final_verdict AS stored_final_verdict,
  (final_verdict = 'TelemetryOnly') AS verdict_column_is_placeholder,
  (
    resolution_path = 'layer7_fallback'
    OR (signal_snapshot ->> 'layer7_upstream') IS NOT NULL
  ) AS is_layer7_policy_exit,
  (
    tree_probability IS NOT NULL
    AND NOT (
      resolution_path = 'layer7_fallback'
      OR (signal_snapshot ->> 'layer7_upstream') IS NOT NULL
    )
  ) AS tree_probability_is_authoritative,
  resolution_path,
  layer_exit,
  tree_probability,
  final_probability,
  created_at
FROM
  inference_log;

CREATE OR REPLACE VIEW image_quality_inference_log_effective AS
SELECT
  id,
  source_path,
  COALESCE(
    inference_snapshot ->> 'runtime_final_verdict',
    final_verdict
  ) AS effective_final_verdict,
  COALESCE(
    inference_snapshot ->> 'runtime_resolution_branch',
    resolution_branch
  ) AS effective_resolution_branch,
  final_verdict AS stored_final_verdict,
  (final_verdict = 'TelemetryOnly') AS verdict_column_is_placeholder,
  resolution_branch,
  predictor_family,
  created_at
FROM
  image_quality_inference_log;

CREATE OR REPLACE VIEW animated_image_quality_inference_log_effective AS
SELECT
  id,
  source_path,
  COALESCE(
    inference_snapshot ->> 'runtime_final_verdict',
    final_verdict
  ) AS effective_final_verdict,
  COALESCE(
    inference_snapshot ->> 'runtime_resolution_branch',
    resolution_branch
  ) AS effective_resolution_branch,
  final_verdict AS stored_final_verdict,
  (final_verdict = 'TelemetryOnly') AS verdict_column_is_placeholder,
  resolution_branch,
  predictor_family,
  created_at
FROM
  animated_image_quality_inference_log;

CREATE OR REPLACE VIEW video_quality_inference_log_effective AS
SELECT
  id,
  source_path,
  COALESCE(
    inference_snapshot ->> 'runtime_final_verdict',
    final_verdict
  ) AS effective_final_verdict,
  COALESCE(
    inference_snapshot ->> 'runtime_resolution_branch',
    resolution_branch
  ) AS effective_resolution_branch,
  final_verdict AS stored_final_verdict,
  (final_verdict = 'TelemetryOnly') AS verdict_column_is_placeholder,
  resolution_branch,
  predictor_family,
  created_at
FROM
  video_quality_inference_log;
