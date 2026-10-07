import ast
import io
import math
import os
import sys
import tempfile
import unittest
from contextlib import redirect_stderr
from pathlib import Path
from unittest.mock import MagicMock, patch

SCRIPT_DIR = Path(__file__).resolve().parents[1]
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))


import fabrication_policy
import loop_intent_clustering
import mfb_entry_guard
import post_training_closure
import run_training
import start_training_four

try:
    import mfb_tool_refresh  # type: ignore[import-not-found]
except ModuleNotFoundError:
    mfb_tool_refresh = None

TRAINING_QUALITY_CRITICAL = (
    SCRIPT_DIR / "run_training.py",
    SCRIPT_DIR / "training_pipeline.py",
)

# Functions where broad except is an honest probe/telemetry boundary (logged + no fake success).
_BROAD_EXCEPT_ALLOWLIST: dict[str, frozenset[str]] = {
    "run_training.py": frozenset(
        {
            "main",
            "reset_training_db",
            "resolve_api_urls",
            "try_probe_loop_intent_for_collect",
            "probe_static_image",
            "collect_static_local_unified",
            "ingest_quality_group",
            "ingest_loop_via_api",
            "ingest_replica_batch",
            "run_four_lane_launcher",
            "run_training_isolated",
        }
    ),
    "training_pipeline.py": frozenset(
        {
            "connect_pg",
            "cmd_repair_multi_scenario_schema",
            "repair_multi_scenario_schema",
        }
    ),
}


def _broad_except_handler_functions(path: Path) -> list[str]:
    tree = ast.parse(path.read_text(encoding="utf-8"), filename=str(path))
    found: list[str] = []
    for node in ast.walk(tree):
        if not isinstance(node, ast.ExceptHandler):
            continue
        if node.type is None:
            found.append(_enclosing_function_name(tree, node))
            continue
        if isinstance(node.type, ast.Name) and node.type.id == "Exception":
            found.append(_enclosing_function_name(tree, node))
        if isinstance(node.type, ast.Tuple):
            names = [elt.id for elt in node.type.elts if isinstance(elt, ast.Name)]
            if "Exception" in names:
                found.append(_enclosing_function_name(tree, node))
    return found


def _enclosing_function_name(tree: ast.AST, node: ast.AST) -> str:
    for parent in ast.walk(tree):
        if not isinstance(parent, (ast.FunctionDef, ast.AsyncFunctionDef)):
            continue
        for child in ast.walk(parent):
            if child is node:
                return parent.name
    return "<module>"


class TestFabricationGuards(unittest.TestCase):
    def test_iter_media_files_fails_closed_on_unreadable_root(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / "corpus"
            root.mkdir()
            with (
                patch.object(
                    run_training.os,
                    "scandir",
                    side_effect=OSError("permission denied"),
                ),
                self.assertRaises(run_training.ScanPlanningError),
            ):
                list(run_training.iter_media_files(root))

    def test_iter_media_files_fails_closed_on_unreadable_entry(self):
        class BrokenEntry:
            path = "/tmp/broken-entry.jpg"

            def is_dir(self, *, follow_symlinks: bool) -> bool:
                raise OSError("entry disappeared")

            def is_file(self, *, follow_symlinks: bool) -> bool:
                return False

        class FakeScandir:
            def __enter__(self):
                return iter([BrokenEntry()])

            def __exit__(self, exc_type, exc, tb):
                return False

        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / "corpus"
            root.mkdir()
            with patch.object(run_training.os, "scandir", return_value=FakeScandir()):  # noqa: SIM117
                with self.assertRaises(run_training.ScanPlanningError):
                    list(run_training.iter_media_files(root))

    def test_file_size_ge_rule_fails_closed_on_stat_error(self):
        with (
            patch.object(
                run_training.Path,
                "stat",
                side_effect=OSError("stat denied"),
            ),
            self.assertRaises(run_training.ScanPlanningError),
        ):
            run_training.rule_file_size_kb_ge(Path("/tmp/source.jpg"), 1)

    def test_file_size_le_rule_fails_closed_on_stat_error(self):
        with (
            patch.object(
                run_training.Path,
                "stat",
                side_effect=OSError("stat denied"),
            ),
            self.assertRaises(run_training.ScanPlanningError),
        ):
            run_training.rule_file_size_kb_le(Path("/tmp/source.jpg"), 1)

    def test_fail_closed_training_default_enabled(self):
        with patch.dict(os.environ, {}, clear=False):
            os.environ.pop("MFB_TRAINING_FAIL_CLOSED", None)
            self.assertTrue(fabrication_policy.fail_closed_training_enabled())

    def test_fail_closed_training_can_be_disabled_for_debug(self):
        with patch.dict(os.environ, {"MFB_TRAINING_FAIL_CLOSED": "0"}):
            self.assertFalse(fabrication_policy.fail_closed_training_enabled())

    def test_run_training_except_policy_reraises_by_default(self):
        with patch.dict(os.environ, {}, clear=False):
            os.environ.pop("MFB_TRAINING_FAIL_CLOSED", None)
            with self.assertRaises(RuntimeError):
                fabrication_policy.run_training_except_policy(
                    "unit_test",
                    ValueError("boom"),
                )

    def test_critical_training_scripts_have_no_unlisted_broad_except(self):
        for path in TRAINING_QUALITY_CRITICAL:
            rel = path.name
            allow = _BROAD_EXCEPT_ALLOWLIST.get(rel, frozenset())
            offenders = [
                fn
                for fn in _broad_except_handler_functions(path)
                if fn not in allow and fn != "<module>"
            ]
            self.assertEqual(
                offenders,
                [],
                f"{rel}: broad except in {offenders} not allowlisted "
                f"(update test allowlist only for honest probe boundaries)",
            )

    def test_sample_complexity_score_invalid_probe_is_unknown_not_zero(self):
        sample = run_training.Sample(
            path_or_url="dummy.gif",
            base_label="animated_loop",
            is_remote=False,
            source=run_training.SampleSources(),
            tier_audit={},
        )
        with patch.object(
            run_training,
            "probe_loop_intent",
            return_value={"ok": True, "complexity": "bad"},
        ):
            self.assertIsNone(run_training.sample_complexity_score(sample))

    def test_mean_complexity_all_unknown_returns_nan(self):
        sample = run_training.Sample(
            path_or_url="dummy.gif",
            base_label="animated_loop",
            is_remote=False,
            source=run_training.SampleSources(),
            tier_audit={},
        )
        with patch.object(run_training, "probe_loop_intent", return_value={"ok": True}):
            value = run_training._mean_complexity([sample])
        self.assertTrue(math.isnan(value))

    def test_loop_intent_bucket_probe_failure_fails_closed_by_default(self):
        sample = run_training.Sample(
            path_or_url="dummy.gif",
            base_label="animated_loop",
            is_remote=False,
            source=run_training.SampleSources(),
            tier_audit={},
        )
        with (
            patch.dict(os.environ, {}, clear=False),
            patch.object(
                run_training,
                "probe_loop_intent",
                return_value={"ok": False, "error": "probe failed"},
            ),
        ):
            os.environ.pop("MFB_TRAINING_FAIL_CLOSED", None)
            with self.assertRaises(RuntimeError):
                run_training.sample_loop_intent_bucket(sample)

    def test_loop_intent_bucket_probe_failure_debug_can_use_uncertain(self):
        sample = run_training.Sample(
            path_or_url="dummy.gif",
            base_label="animated_loop",
            is_remote=False,
            source=run_training.SampleSources(),
            tier_audit={},
        )
        with (
            patch.dict(os.environ, {"MFB_TRAINING_FAIL_CLOSED": "0"}),
            patch.object(
                run_training,
                "probe_loop_intent",
                return_value={"ok": False, "error": "probe failed"},
            ),
        ):
            self.assertEqual(
                run_training.sample_loop_intent_bucket(sample), "uncertain"
            )

    def test_static_local_collector_empty_dirs_fails_closed_by_default(self):
        with (
            patch.dict(os.environ, {}, clear=False),
            patch.object(
                run_training,
                "require_rust_tier_probe",
                side_effect=AssertionError("tier probe must not mask empty local_dirs"),
            ),
        ):
            os.environ.pop("MFB_TRAINING_FAIL_CLOSED", None)
            with self.assertRaises(RuntimeError):
                run_training.collect_static_local_unified(
                    run_training.EMPTY_QUALITY_GROUP,
                    run_training.EMPTY_QUALITY_GROUP,
                    label_filter=None,
                )

    def test_static_local_collector_empty_dirs_debug_can_return_empty(self):
        with (
            patch.dict(os.environ, {"MFB_TRAINING_FAIL_CLOSED": "0"}),
            patch.object(run_training, "require_rust_tier_probe", return_value=None),
        ):
            self.assertEqual(
                run_training.collect_static_local_unified(
                    run_training.EMPTY_QUALITY_GROUP,
                    run_training.EMPTY_QUALITY_GROUP,
                    label_filter=None,
                ),
                [],
            )

    def test_loop_local_collector_empty_dirs_fails_closed_by_default(self):
        with patch.dict(os.environ, {}, clear=False):
            os.environ.pop("MFB_TRAINING_FAIL_CLOSED", None)
            with self.assertRaises(RuntimeError):
                run_training.collect_loop_local_from_media_dirs(
                    run_training.EMPTY_QUALITY_GROUP,
                    run_training.EMPTY_QUALITY_GROUP,
                )

    def test_load_rules_missing_file_fails_closed(self):
        with tempfile.TemporaryDirectory() as tmp:
            missing = Path(tmp) / "training_rules.json"
            with patch.object(run_training, "RULES_FILE", missing):  # noqa: SIM117
                with self.assertRaises(FileNotFoundError):
                    run_training.load_rules()

    def test_invalid_training_env_fails_closed_by_default(self):
        with patch.dict(os.environ, {"MFB_BAD_INT": "0"}, clear=False):
            os.environ.pop("MFB_TRAINING_FAIL_CLOSED", None)
            with self.assertRaises(ValueError):
                run_training.parse_positive_int_env("MFB_BAD_INT", 25)

    def test_invalid_training_env_debug_can_use_default(self):
        with patch.dict(
            os.environ,
            {"MFB_BAD_FLOAT": "bad", "MFB_TRAINING_FAIL_CLOSED": "0"},
            clear=False,
        ):
            self.assertEqual(
                run_training.parse_positive_float_env("MFB_BAD_FLOAT", 15.0),
                15.0,
            )

    def test_training_pipeline_quality_average_is_not_coalesced_to_zero(self):
        source = (SCRIPT_DIR / "training_pipeline.py").read_text(encoding="utf-8")
        self.assertNotIn("COALESCE(AVG({}), 0.0)", source)
        self.assertNotIn(
            "COALESCE((metadata->>'directory_loop_intent_score')::double precision, 0.5)",
            source,
        )

    def test_loop_clustering_does_not_fabricate_neutral_priors(self):
        source = (SCRIPT_DIR / "loop_intent_clustering.py").read_text(encoding="utf-8")
        self.assertNotIn("if priors else 0.5", source)
        self.assertNotIn("per_row_updates.append((row.blake3, -1, 0.5))", source)

    def test_loop_clustering_rejects_unexpected_training_labels(self):
        self.assertTrue(loop_intent_clustering.label_is_valid_training_class(0))
        self.assertTrue(loop_intent_clustering.label_is_valid_training_class(1))
        self.assertFalse(loop_intent_clustering.label_is_valid_training_class(2))

    def test_reset_training_db_requires_psycopg2(self):
        original_import = __import__

        def side_effect(name, globals=None, locals=None, fromlist=(), level=0):
            if name == "psycopg2":
                raise ImportError("blocked")
            return original_import(name, globals, locals, fromlist, level)

        with patch("builtins.__import__", side_effect=side_effect):
            stderr = io.StringIO()
            with redirect_stderr(stderr), self.assertRaises(SystemExit) as raised:
                run_training.reset_training_db(
                    "postgresql://localhost/modern_format_boost"
                )

        self.assertEqual(raised.exception.code, 1)
        self.assertIn("refusing to start training", stderr.getvalue())

    def test_start_training_four_requires_reset_db_for_real_run(self):
        with self.assertRaises(SystemExit) as raised:
            start_training_four.ensure_reset_db_before_training(
                reset_db=False,
                dry_run=False,
            )

        self.assertIn("--reset-db is required", str(raised.exception))
        start_training_four.ensure_reset_db_before_training(
            reset_db=True, dry_run=False
        )
        start_training_four.ensure_reset_db_before_training(
            reset_db=False, dry_run=True
        )

    def test_db_training_closure_docs_reject_open_launcher_gate(self):
        with tempfile.TemporaryDirectory() as tmp:
            hardening = Path(tmp)
            (hardening / "AUDIT_SINGLE_SOURCE_OF_TRUTH_WITH_VERIFY.md").write_text(
                "DB_TRAIN_BOUNDED_AUDIT=17/17\n",
                encoding="utf-8",
            )
            (hardening / "SINGLE_SOURCE_OF_TRUTH_WITH_CLOSURE.md").write_text(
                "DB_TRAIN_FOUR_LANE_RESET_GATE=4/4\n"
                "DB_TRAIN_LAUNCHER_CLOSURE_DOC_GATE=0/4\n"
                "DB_TRAIN_TRAINING_LAUNCH_ALLOWED=no\n",
                encoding="utf-8",
            )

            with self.assertRaises(SystemExit) as raised:
                start_training_four.ensure_db_training_closure_before_training(
                    hardening
                )

        self.assertIn("DB/train closure gate is not closed", str(raised.exception))

    def test_db_training_closure_docs_accept_closed_launch_markers(self):
        with tempfile.TemporaryDirectory() as tmp:
            hardening = Path(tmp)
            (hardening / "AUDIT_SINGLE_SOURCE_OF_TRUTH_WITH_VERIFY.md").write_text(
                "DB_TRAIN_BOUNDED_AUDIT=17/17\n",
                encoding="utf-8",
            )
            (hardening / "SINGLE_SOURCE_OF_TRUTH_WITH_CLOSURE.md").write_text(
                "DB_TRAIN_FOUR_LANE_RESET_GATE=4/4\n"
                "DB_TRAIN_LAUNCHER_CLOSURE_DOC_GATE=4/4\n"
                "DB_TRAIN_TRAINING_LAUNCH_ALLOWED=yes\n",
                encoding="utf-8",
            )

            start_training_four.ensure_db_training_closure_before_training(hardening)

    def test_db_training_closure_docs_accept_current_ssot_file(self):
        with tempfile.TemporaryDirectory() as tmp:
            hardening = Path(tmp)
            (hardening / "SSOT.md").write_text(
                "DB_TRAIN_BOUNDED_AUDIT=17/17\n"
                "DB_TRAIN_FOUR_LANE_RESET_GATE=4/4\n"
                "DB_TRAIN_LAUNCHER_CLOSURE_DOC_GATE=4/4\n"
                "DB_TRAIN_TRAINING_LAUNCH_ALLOWED=yes\n",
                encoding="utf-8",
            )

            start_training_four.ensure_db_training_closure_before_training(hardening)

    def test_four_lane_dry_run_does_not_spawn_training_process(self):
        with tempfile.TemporaryDirectory() as tmp:  # noqa: SIM117
            with patch.object(run_training.subprocess, "run") as run_mock:
                name, code, log_path = run_training.start_four_lane(
                    lane="static_high",
                    argv_tail=[
                        "--training-mode",
                        "static",
                        "--label",
                        "high",
                        "--no-loop",
                        "--max-high",
                        "1450",
                    ],
                    log_root=Path(tmp),
                    stamp="20260609_000000",
                    connstr="postgresql://localhost/modern_format_boost",
                    dry_run=True,
                )

        self.assertEqual(name, "static_high")
        self.assertEqual(code, 0)
        self.assertEqual(log_path.name, "run_training_20260609_000000.log")
        run_mock.assert_not_called()

    def test_start_training_four_lane_caps_are_expected(self):
        lane_args = {lane: tail for lane, tail in start_training_four.LANE_SPECS}

        self.assertEqual(
            lane_args["static_high"][lane_args["static_high"].index("--max-high") + 1],
            "1450",
        )
        self.assertEqual(
            lane_args["static_low"][lane_args["static_low"].index("--max-low") + 1],
            "1450",
        )
        self.assertEqual(
            lane_args["loop_high"][lane_args["loop_high"].index("--max-loop") + 1],
            "450",
        )
        self.assertEqual(
            lane_args["loop_low"][lane_args["loop_low"].index("--max-non-loop") + 1],
            "450",
        )
        self.assertEqual(
            lane_args["loop_low"][
                lane_args["loop_low"].index("--loop-intent-label") + 1
            ],
            "video",
        )
        self.assertEqual(run_training.explicit_loop_balance_bucket("video"), "non_loop")
        self.assertEqual(
            start_training_four._lane_slug_from_tail(lane_args["loop_low"]),
            "loop_low",
        )

    def test_four_lane_static_lanes_finalize_lightgbm_model(self):
        lane_args = {lane: tail for lane, tail in start_training_four.LANE_SPECS}

        for lane in ("static_high", "static_low"):
            self.assertIn("--no-fill-runtime-assets", lane_args[lane])
            self.assertNotIn("--finalize-image-quality-model", lane_args[lane])

        for lane in ("loop_high", "loop_low"):
            self.assertNotIn("--finalize-image-quality-model", lane_args[lane])

    def test_training_error_mode_env_accepts_log_and_continue(self):
        with patch.dict(
            os.environ,
            {"MFB_TRAINING_ERROR_MODE": "log-and-continue"},
            clear=False,
        ):
            self.assertEqual(
                run_training.training_error_mode(),
                run_training.TRAINING_ERROR_MODE_LOG_AND_CONTINUE,
            )
            self.assertFalse(run_training.training_ingest_fail_fast())

    def test_four_lane_spawn_uses_log_and_continue_error_mode(self):
        class FakeProc:
            pid = 424244

            def poll(self):
                return None

        captured_env = {}

        def fake_popen(*_args, **kwargs):
            captured_env.update(kwargs["env"])
            return FakeProc()

        with (
            tempfile.TemporaryDirectory() as tmp,
            patch.object(
                run_training, "training_lane_pid_is_active", return_value=False
            ),
            patch.object(run_training, "record_stale_lane_death"),
            patch.object(
                run_training,
                "ensure_foundation_dylib",
                return_value="/tmp/libfoundation.dylib",
            ),
            patch.object(run_training.subprocess, "Popen", side_effect=fake_popen),
            patch.object(run_training.time, "sleep", return_value=None),
        ):
            start_training_four.start_lane(
                lane="static_high",
                argv_tail=["--training-mode", "static", "--label", "high"],
                log_root=Path(tmp),
                stamp="teststamp",
                connstr="postgresql://localhost/modern_format_boost",
                dry_run=False,
            )

        self.assertEqual(
            captured_env["MFB_TRAINING_ERROR_MODE"],
            run_training.TRAINING_ERROR_MODE_LOG_AND_CONTINUE,
        )

    def test_ingest_quality_cli_log_and_continue_processes_later_files(self):
        failed = MagicMock(returncode=1, stderr="feature missing", stdout="")
        passed = MagicMock(returncode=0, stderr="", stdout="")
        with (
            patch.dict(
                os.environ,
                {"MFB_TRAINING_ERROR_MODE": "log-and-continue"},
                clear=False,
            ),
            patch.object(
                run_training,
                "run_rust_ingest",
                side_effect=[failed, passed],
            ) as ingest_mock,
        ):
            result = run_training.ingest_quality_via_cli(
                [Path("/tmp/a.heic"), Path("/tmp/b.jpg")],
                "high",
                "image_quality",
                "postgresql://localhost/modern_format_boost",
            )

        self.assertEqual(result, (1, 1, 0))
        self.assertEqual(ingest_mock.call_count, 2)

    def test_ingest_quality_cli_fail_fast_stops_at_first_failure(self):
        failed = MagicMock(returncode=1, stderr="feature missing", stdout="")
        with (
            patch.dict(
                os.environ,
                {"MFB_TRAINING_ERROR_MODE": "fail-fast"},
                clear=False,
            ),
            patch.object(
                run_training,
                "run_rust_ingest",
                return_value=failed,
            ) as ingest_mock,
            self.assertRaises(SystemExit) as raised,
        ):
            run_training.ingest_quality_via_cli(
                [Path("/tmp/a.heic"), Path("/tmp/b.jpg")],
                "high",
                "image_quality",
                "postgresql://localhost/modern_format_boost",
            )

        self.assertEqual(raised.exception.code, 1)
        ingest_mock.assert_called_once()

    def test_log_and_continue_allows_nonzero_failures_with_successes(self):
        with patch.dict(
            os.environ,
            {"MFB_TRAINING_ERROR_MODE": "log-and-continue"},
            clear=False,
        ):
            self.assertEqual(
                run_training.training_ingest_failure_exit_code(
                    total_success=449,
                    total_fail_other=1,
                    total_fail_label_conflict=0,
                ),
                0,
            )

    def test_fail_fast_reports_nonzero_failures_as_failure(self):
        with patch.dict(
            os.environ,
            {"MFB_TRAINING_ERROR_MODE": "fail-fast"},
            clear=False,
        ):
            self.assertEqual(
                run_training.training_ingest_failure_exit_code(
                    total_success=449,
                    total_fail_other=1,
                    total_fail_label_conflict=0,
                ),
                1,
            )

    def test_post_training_closure_can_finalize_image_quality_model(self):
        with (
            tempfile.TemporaryDirectory() as tmp_repo,
            tempfile.TemporaryDirectory() as tmp_home,
        ):
            repo_root = Path(tmp_repo)
            with patch.dict(os.environ, {"HOME": tmp_home}, clear=False):  # noqa: SIM117
                with patch.object(
                    post_training_closure.subprocess,
                    "run",
                    return_value=MagicMock(returncode=0),
                ):
                    code = post_training_closure.finalize_image_quality_model(
                        "postgresql://localhost/modern_format_boost",
                        repo_root,
                        "20260607_183000",
                    )

        self.assertEqual(code, 0)

    def test_run_training_owns_four_lane_launcher_contract(self):
        self.assertEqual(start_training_four.LANE_SPECS, run_training.FOUR_LANE_SPECS)
        self.assertIs(
            start_training_four.ensure_reset_db_before_training,
            run_training.ensure_reset_db_before_training,
        )
        self.assertIs(start_training_four.start_lane, run_training.start_four_lane)

    def test_start_training_four_coerces_workspace_log_root(self):
        with tempfile.TemporaryDirectory() as tmp_home:  # noqa: SIM117
            with patch.dict(os.environ, {"HOME": tmp_home}, clear=False):
                os.environ.pop("MFB_HOME_ROOT", None)
                resolved = start_training_four.resolve_launch_log_root(
                    start_training_four.ROOT / "logs"
                )

        self.assertEqual(
            resolved,
            Path(tmp_home) / ".modern_format_boost" / "logs",
        )

    def test_start_training_four_rejects_workspace_home_root_logs(self):
        with (
            tempfile.TemporaryDirectory() as tmp_home,
            patch.dict(
                os.environ,
                {"HOME": tmp_home, "MFB_HOME_ROOT": str(start_training_four.ROOT)},
                clear=False,
            ),
        ):
            resolved = start_training_four.resolve_launch_log_root(None)

        self.assertEqual(
            resolved,
            Path(tmp_home) / ".modern_format_boost" / "logs",
        )

    def test_shell_wrapper_guard_ignores_codex_snapshot_wrapper(self):
        sample = (
            "lean-ctx -c exec /bin/zsh -c "
            "'python3 crates/dev/scripts/run_training.py --dry-run'"
        )
        with (
            patch.object(
                mfb_entry_guard,
                "_process_args",
                side_effect=[sample, ""],
            ),
            patch.object(mfb_entry_guard, "_parent_pid", side_effect=[100, 0]),
        ):
            self.assertIsNone(mfb_entry_guard.shell_wrapper_in_ancestry())

    def test_run_rust_ingest_rebuilds_missing_train_knn_before_spawn(self):
        completed = MagicMock(returncode=0)
        with (
            patch.object(run_training, "artifact_is_stale", return_value=True),
            patch.object(run_training, "rebuild_rust_artifacts") as rebuild_mock,
            patch.object(Path, "exists", return_value=True),
            patch.object(
                mfb_entry_guard.subprocess,
                "run",
                return_value=completed,
            ) as spawn_mock,
        ):
            result = run_training.run_rust_ingest(
                [str(run_training.TRAIN_BIN_KNN), "sample.gif"],
                conn_str="postgresql://localhost/modern_format_boost",
            )

        self.assertIs(result, completed)
        rebuild_mock.assert_called_once_with(["train_knn"])
        spawn_mock.assert_called_once()

    def test_post_training_closure_verify_log_uses_user_log_root(self):
        with (
            tempfile.TemporaryDirectory() as tmp_repo,
            tempfile.TemporaryDirectory() as tmp_home,
        ):
            repo_root = Path(tmp_repo)
            with (
                patch.dict(os.environ, {"HOME": tmp_home}, clear=False),
                patch.object(
                    post_training_closure.subprocess,
                    "run",
                    return_value=MagicMock(returncode=0),
                ),
            ):
                os.environ.pop("MFB_HOME_ROOT", None)
                os.environ.pop("MFB_LOG_DIR", None)
                code = post_training_closure.run_verify_stack(
                    "postgresql://localhost/modern_format_boost",
                    repo_root,
                    "20260607_183000",
                )

            self.assertEqual(code, 0)
            self.assertFalse((repo_root / "logs").exists())
            self.assertTrue(
                (
                    Path(tmp_home)
                    / ".modern_format_boost"
                    / "logs"
                    / "runtime_v1_verify_stack_20260607_183000.log"
                ).is_file()
            )

    def test_start_training_four_stops_spawn_when_pid_write_fails(self):
        class FakeProc:
            pid = 424242

            def poll(self):
                return None

        with (  # noqa: SIM117
            tempfile.TemporaryDirectory() as tmp,
            patch.object(
                run_training,
                "ensure_foundation_dylib",
                return_value="/tmp/libfoundation.dylib",
            ),
            patch.object(run_training.subprocess, "Popen", return_value=FakeProc()),
            patch.object(run_training.time, "sleep", return_value=None),
            patch.object(Path, "write_text", side_effect=PermissionError("pid denied")),
            patch.object(run_training.os, "kill") as kill_mock,
        ):
            with self.assertRaises(RuntimeError) as raised:
                start_training_four.start_lane(
                    lane="static_high",
                    argv_tail=["--training-mode", "static", "--label", "high"],
                    log_root=Path(tmp),
                    stamp="teststamp",
                    connstr="postgresql://localhost/modern_format_boost",
                    dry_run=False,
                )

        kill_mock.assert_called_once_with(FakeProc.pid, 15)
        self.assertIn("pid file write failed", str(raised.exception))

    def test_stop_four_lane_escalates_when_pid_survives_sigterm(self):
        with tempfile.TemporaryDirectory() as tmp:
            lane_dir = Path(tmp)
            (lane_dir / "run_training.pid").write_text("4242\n", encoding="utf-8")
            with (
                patch.object(run_training.os, "kill") as kill_mock,
                patch.object(
                    run_training.time, "monotonic", side_effect=[0.0, 0.0, 10.0]
                ),
                patch.object(run_training.time, "sleep"),
            ):
                run_training.stop_four_lane(lane_dir)

        self.assertEqual(
            [call.args for call in kill_mock.call_args_list],
            [(4242, 15), (4242, 0), (4242, 9)],
        )
        self.assertFalse((lane_dir / "run_training.pid").exists())

    def test_start_four_lane_records_stale_pid_crash_snapshot_before_relaunch(self):
        class FakeProc:
            pid = 424243

            def poll(self):
                return None

        with tempfile.TemporaryDirectory() as tmp:
            lane_dir = Path(tmp) / "static_high"
            lane_dir.mkdir()
            (lane_dir / "run_training.pid").write_text("4242\n", encoding="utf-8")
            (lane_dir / "training_session_audit.jsonl").write_text(
                '{"event":"heartbeat","phase":"collect"}\n',
                encoding="utf-8",
            )
            (lane_dir / "run_training_oldstamp.log").write_text(
                "old run\n",
                encoding="utf-8",
            )
            with (
                patch.object(
                    run_training, "training_lane_pid_is_active", return_value=False
                ),
                patch.object(
                    run_training,
                    "ensure_foundation_dylib",
                    return_value="/tmp/libfoundation.dylib",
                ),
                patch.object(run_training.subprocess, "Popen", return_value=FakeProc()),
                patch.object(run_training.time, "sleep", return_value=None),
            ):
                start_training_four.start_lane(
                    lane="static_high",
                    argv_tail=["--training-mode", "static", "--label", "high"],
                    log_root=Path(tmp),
                    stamp="teststamp",
                    connstr="postgresql://localhost/modern_format_boost",
                    dry_run=False,
                )

            snapshot = (
                lane_dir / "TrainingBundle_oldstamp" / "training_session_exit.json"
            )
            self.assertTrue(snapshot.is_file())
            snapshot_text = snapshot.read_text(encoding="utf-8")
            self.assertIn("stale-pid-dead-process", snapshot_text)
            self.assertIn('"session_stamp": "oldstamp"', snapshot_text)
            self.assertFalse((lane_dir / "training_session_exit.json").exists())

    def test_finalize_training_session_logs_removes_child_owned_pid_file(self):
        with tempfile.TemporaryDirectory() as tmp:
            log_dir = Path(tmp)
            pid_file = log_dir / "run_training.pid"
            pid_file.write_text(f"{os.getpid()}\n", encoding="utf-8")
            with (
                patch.object(run_training, "TRAINING_LOG_DIR", log_dir),
                patch.object(
                    run_training,
                    "ensure_training_session_stamp",
                    return_value="20260613_010203",
                ),
                patch.object(run_training, "close_tier_audit_stream"),
                patch.object(
                    run_training, "archive_training_session_bundle", return_value=None
                ),
            ):
                run_training._finalize_training_session_logs("test-scope")

            self.assertFalse(pid_file.exists())

    @unittest.skipIf(
        mfb_tool_refresh is None,
        "legacy Python tool refresh was replaced by the Rust toolchain manager",
    )
    def test_tool_refresh_logs_import_failure(self):
        mfb_tool_refresh._REFRESH_DONE = False
        with (
            patch.object(mfb_tool_refresh, "_ci_environment", return_value=False),
            patch.object(mfb_tool_refresh, "_skip_refresh_env", return_value=False),
            patch.object(
                mfb_tool_refresh.shutil,
                "which",
                side_effect=lambda name: name == "cargo",
            ),
            patch("builtins.__import__") as import_mock,
        ):
            original_import = __import__

            def side_effect(name, globals=None, locals=None, fromlist=(), level=0):
                if name == "mfb_rust_toolchain":
                    raise RuntimeError("import blocked")
                return original_import(name, globals, locals, fromlist, level)

            import_mock.side_effect = side_effect
            stderr = io.StringIO()
            with redirect_stderr(stderr):
                mfb_tool_refresh.refresh_tools_for_processing(quiet=True, force=True)
        self.assertIn("cargo install preflight failed", stderr.getvalue())

    @unittest.skipIf(
        mfb_tool_refresh is None,
        "legacy Python tool refresh was replaced by the Rust toolchain manager",
    )
    def test_tool_refresh_rustup_toolchain_install_omits_invalid_yes_flag(self):
        mfb_tool_refresh._REFRESH_DONE = False
        calls = []

        def capture_run_step(label, cmd, **_kwargs):
            calls.append((label, cmd))
            return True

        with (
            patch.object(mfb_tool_refresh, "_ci_environment", return_value=False),
            patch.object(mfb_tool_refresh, "_skip_refresh_env", return_value=False),
            patch.object(
                mfb_tool_refresh,
                "_pinned_rust_channel",
                return_value="nightly-2026-07-16",
            ),
            patch.object(
                mfb_tool_refresh, "_rust_toolchain_components", return_value=[]
            ),
            patch.object(
                mfb_tool_refresh.shutil,
                "which",
                side_effect=lambda name: name == "rustup",
            ),
            patch.object(mfb_tool_refresh, "_run_step", side_effect=capture_run_step),
        ):
            mfb_tool_refresh.refresh_tools_for_processing(quiet=True, force=True)

        rustup_cmds = [
            cmd for label, cmd in calls if label.startswith("rustup toolchain")
        ]
        self.assertEqual(
            rustup_cmds, [["rustup", "toolchain", "install", "nightly-2026-07-16"]]
        )


if __name__ == "__main__":
    unittest.main()
