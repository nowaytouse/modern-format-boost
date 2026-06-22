#!/usr/bin/env python3
"""
Ephemeral validation in /tmp only — never writes under the repo tree or user bundles.

Creates fixtures, runs release vid/img binaries, greps logs for contract signals, runs verify.
"""

from __future__ import annotations

import argparse
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parent
REPO_ROOT = SCRIPT_DIR.parents[2]
VID_BIN = REPO_ROOT / "target" / "release" / "vid"
IMG_BIN = REPO_ROOT / "target" / "release" / "img"


def run(
    cmd: list[str], *, log: Path | None = None, check: bool = False
) -> subprocess.CompletedProcess[str]:
    print("+", " ".join(cmd), flush=True)
    text = subprocess.run(
        cmd,
        capture_output=True,
        text=True,
        check=check,
    )
    out = (text.stdout or "") + (text.stderr or "")
    if log:
        log.write_text(out, encoding="utf-8")
    if text.returncode != 0 and not check:
        print(out[-4000:], file=sys.stderr)
    return text


def require_tools() -> None:
    for tool in ("ffmpeg", "ffprobe"):
        if shutil.which(tool) is None:
            raise SystemExit(f"Missing required tool: {tool}")
    if not VID_BIN.is_file():
        raise SystemExit(f"Build vid first: cargo build -p vid --release ({VID_BIN})")
    if not IMG_BIN.is_file():
        raise SystemExit(f"Build img first: cargo build -p img --release ({IMG_BIN})")


def make_fixtures(src: Path) -> None:
    src.mkdir(parents=True, exist_ok=True)
    run(
        [
            "ffmpeg",
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "testsrc=duration=8:size=640x360:rate=30",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            str(src / "video9.mp4"),
        ],
        check=True,
    )
    run(
        [
            "ffmpeg",
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "color=red:s=64x64:d=1",
            "-frames:v",
            "1",
            str(src / "static.webp"),
        ],
        check=True,
    )
    # ffmpeg webp often yields 0x0 in ffprobe — exercises header dimension fallback
    run(
        [
            "ffmpeg",
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "testsrc=duration=1:size=64x64:rate=10",
            str(src / "anim.webp"),
        ],
        check=False,
    )


def grep_checks(log_text: str) -> dict[str, bool]:
    low = log_text.lower()
    return {
        "3d_gate_passed": "3d quality gate: passed" in low,
        "no_ssim_enforce_reject": "enforce_ssim_presence" not in low
        and "ssim below target" not in low,
        "ultimate_mode_logged": "ultimate mode" in low or "3d quality gate" in low,
        "ignore_class_static": "ignore_class=vid_static_single_frame" in low
        or "vid ignores static media" in low,
        "ignore_class_img_anim": "ignore_class=img_animated_handoff" in low,
        "confidence_skip": "exploration confidence missing" in low,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--keep",
        action="store_true",
        help="Print sandbox path and do not delete (still under /tmp)",
    )
    args = parser.parse_args()
    require_tools()

    sandbox = Path(tempfile.mkdtemp(prefix="mfb_sandbox_"))
    src = sandbox / "src"
    opt = sandbox / "opt"
    logs = sandbox / "logs"
    opt.mkdir(parents=True)
    logs.mkdir(parents=True)

    print(f"Sandbox: {sandbox}")
    try:
        make_fixtures(src)

        # Ultimate explore (HEVC) — must not fail SSIM contract
        vlog = logs / "video9.txt"
        run(
            [
                str(VID_BIN),
                "run",
                "--codec",
                "hevc",
                "--force",
                "--ultimate",
                "--explore",
                "--compress",
                "--match-quality",
                "--plain",
                "--no-resume",
                "-o",
                str(opt),
                str(src / "video9.mp4"),
            ],
            log=vlog,
        )
        vchecks = grep_checks(vlog.read_text(encoding="utf-8", errors="ignore"))

        # Static webp — vid ignore with structured class
        slog = logs / "static_webp.txt"
        run(
            [
                str(VID_BIN),
                "run",
                "--plain",
                "--no-resume",
                "-o",
                str(opt),
                str(src / "static.webp"),
            ],
            log=slog,
        )
        schecks = grep_checks(slog.read_text(encoding="utf-8", errors="ignore"))

        # Animated webp — should not die on ffprobe 0x0 without fallback attempt
        alog = logs / "anim_webp.txt"
        run(
            [
                str(VID_BIN),
                "run",
                "--plain",
                "--no-resume",
                "-o",
                str(opt),
                str(src / "anim.webp"),
            ],
            log=alog,
        )
        anim_text = alog.read_text(encoding="utf-8", errors="ignore")
        anim_not_static_ignore = (
            "ignore_class=vid_static_unknown_frames" not in anim_text
            and "ignore_class=vid_static_single_frame" not in anim_text
        )

        # img on animated webp — structured handoff to vid
        ilog = logs / "anim_img.txt"
        run(
            [
                str(IMG_BIN),
                "run",
                "--plain",
                "--no-resume",
                "-o",
                str(opt),
                str(src / "anim.webp"),
            ],
            log=ilog,
        )
        ichecks = grep_checks(ilog.read_text(encoding="utf-8", errors="ignore"))

        # verify.py integrity (both mode) — only tmp trees
        verify_out = logs / "verify_report.txt"
        verify_cmd = [
            sys.executable,
            str(SCRIPT_DIR / "verify.py"),
            "--verify",
            str(src),
            str(opt),
            "--mode",
            "both",
        ]
        if any(logs.iterdir()):
            verify_cmd.extend(["--session-audit", str(logs)])
        proc = run(verify_cmd, log=verify_out)

        print("\n── Sandbox checks ──")
        print(f"  video9 3D gate passed:     {vchecks['3d_gate_passed']}")
        print(f"  video9 ultimate/3D signal: {vchecks['ultimate_mode_logged']}")
        print(f"  video9 no SSIM reject:     {vchecks['no_ssim_enforce_reject']}")
        print(
            f"  video9 confidence skip:    {vchecks['confidence_skip']} (want False after backfill)"
        )
        print(f"  static ignore_class/heur:  {schecks['ignore_class_static']}")
        print(
            f"  anim log has 0x0 probe:    {'0x0' in anim_text or 'image data not found' in anim_text}"
        )
        print(f"  anim not static-ignore:    {anim_not_static_ignore}")
        print(f"  img anim handoff class:    {ichecks['ignore_class_img_anim']}")
        print(f"  verify exit:               {proc.returncode}")

        ok = (
            vchecks["3d_gate_passed"]
            and vchecks["ultimate_mode_logged"]
            and vchecks["no_ssim_enforce_reject"]
            and not vchecks["confidence_skip"]
            and schecks["ignore_class_static"]
            and anim_not_static_ignore
            and ichecks["ignore_class_img_anim"]
        )
        if not ok:
            print("\nSANDBOX FAILED — see logs under", logs, file=sys.stderr)
            return 1
        print("\nSANDBOX OK")
        if args.keep:
            print(f"Kept: {sandbox}")
            return 0
        return 0
    finally:
        if not args.keep:
            shutil.rmtree(sandbox, ignore_errors=True)


if __name__ == "__main__":
    raise SystemExit(main())
