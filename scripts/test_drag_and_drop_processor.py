#!/usr/bin/env python3
import os
import sys
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch


TEST_STATE_ROOT = tempfile.mkdtemp(prefix="mfb_drag_drop_test_")
os.environ["MFB_HOME_ROOT"] = TEST_STATE_ROOT
sys.path.insert(0, str(Path(__file__).resolve().parent))

import drag_and_drop_processor as ddp


class TestCheckSystemResources(unittest.TestCase):
    def setUp(self):
        self.env_patch = patch.dict(os.environ, {}, clear=False)
        self.env_patch.start()
        os.environ.pop("MFB_SKIP_DISK_PRECHECK", None)
        ddp.MEDIA_TOTAL_SIZE = 0

    def tearDown(self):
        self.env_patch.stop()

    def test_low_memory_returns_to_home(self):
        fake_psutil = SimpleNamespace(
            disk_usage=lambda _: SimpleNamespace(free=10 * 1024**3),
            virtual_memory=lambda: SimpleNamespace(percent=96),
            cpu_percent=lambda interval=0.1: 12,
        )

        with patch.object(ddp, "psutil", fake_psutil, create=True):
            with patch.object(ddp.time, "sleep") as sleep_mock:
                with self.assertRaises(ddp.ReturnToHomeException):
                    ddp.check_system_resources("/tmp")

        sleep_mock.assert_called_once_with(5)
        self.assertNotIn("MFB_SKIP_DISK_PRECHECK", os.environ)

    def test_disk_shortage_returns_to_home(self):
        fake_psutil = SimpleNamespace(
            disk_usage=lambda _: SimpleNamespace(free=0),
            virtual_memory=lambda: SimpleNamespace(percent=10),
            cpu_percent=lambda interval=0.1: 12,
        )

        with patch.object(ddp, "psutil", fake_psutil, create=True):
            with patch.object(ddp.time, "sleep") as sleep_mock:
                with self.assertRaises(ddp.ReturnToHomeException):
                    ddp.check_system_resources("/tmp")

        sleep_mock.assert_called_once_with(5)
        self.assertNotIn("MFB_SKIP_DISK_PRECHECK", os.environ)

    def test_healthy_resources_enable_skip_flag(self):
        fake_psutil = SimpleNamespace(
            disk_usage=lambda _: SimpleNamespace(free=10 * 1024**3),
            virtual_memory=lambda: SimpleNamespace(percent=42),
            cpu_percent=lambda interval=0.1: 12,
        )

        with patch.object(ddp, "psutil", fake_psutil, create=True):
            with patch.object(ddp.time, "sleep") as sleep_mock:
                ddp.check_system_resources("/tmp")

        sleep_mock.assert_not_called()
        self.assertEqual(os.environ.get("MFB_SKIP_DISK_PRECHECK"), "1")


if __name__ == "__main__":
    unittest.main()
