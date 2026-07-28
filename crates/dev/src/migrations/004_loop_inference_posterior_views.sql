-- Loop inference_log analytics: Layer 7 policy exits and authoritative posteriors.
-- Idempotent: safe to re-run after schema init.
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
