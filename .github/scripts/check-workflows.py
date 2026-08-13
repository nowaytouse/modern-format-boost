#!/usr/bin/env python3
# GitHub Actions Workflow Health Check Script
# This script helps identify common workflow issues before they cause failures

import re
import shutil
import subprocess
import sys
from pathlib import Path

# Colors for output
RED = "\033[0;31m"
GREEN = "\033[0;32m"
YELLOW = "\033[1;33m"
BLUE = "\033[0;34m"
NC = "\033[0m"  # No Color


def print_status(color, message):
    print(f"{color}{message}{NC}")


def collect_workflow_files():
    workflows_dir = Path(".github/workflows")
    if not workflows_dir.exists():
        return []
    return list(workflows_dir.glob("*.yml")) + list(workflows_dir.glob("*.yaml"))


def check_workflow_syntax(workflow_files):
    print_status(BLUE, "🔍 Checking workflow syntax...")
    syntax_errors = 0
    yamllint_path = shutil.which("yamllint")
    ruby_path = shutil.which("ruby")

    for file in workflow_files:
        if yamllint_path:
            res = subprocess.run(  # noqa: PLW1510
                [yamllint_path, str(file)], capture_output=True, text=True
            )
            if res.returncode != 0:
                print_status(RED, f"❌ YAML syntax error in {file}")
                print(res.stdout, file=sys.stderr)
                syntax_errors += 1
            else:
                print_status(GREEN, f"✅ {file.name} syntax OK")
        elif ruby_path:
            cmd = [
                "ruby",
                "-e",
                "require 'yaml'; YAML.load_file(ARGV.fetch(0))",
                str(file),
            ]
            res = subprocess.run(cmd, capture_output=True, text=True)  # noqa: PLW1510
            if res.returncode != 0:
                print_status(RED, f"❌ YAML syntax error in {file}")
                print(res.stderr, file=sys.stderr)
                syntax_errors += 1
            else:
                print_status(GREEN, f"✅ {file.name} syntax OK (ruby YAML fallback)")
        else:
            print_status(
                RED, "❌ yamllint not installed and ruby YAML fallback unavailable"
            )
            syntax_errors += 1
            break

    return syntax_errors


def check_action_versions(workflow_files):
    print_status(BLUE, "🔍 Checking action versions...")
    outdated_actions = 0
    for file in workflow_files:
        try:
            with open(file) as f:
                content = f.read()
            if re.search(r"actions/checkout@(v1|v2|v3)\b", content):
                print_status(
                    YELLOW,
                    f"⚠️  Old checkout action version in {file.name} (should be v4)",
                )
                outdated_actions += 1
            if re.search(r"actions/cache@(v1|v2|v3)\b", content):
                print_status(
                    YELLOW, f"⚠️  Old cache action version in {file.name} (should be v4)"
                )
                outdated_actions += 1
            if re.search(r"actions/upload-artifact@(v1|v2|v3)\b", content):
                print_status(
                    YELLOW,
                    f"⚠️  Old upload-artifact action version in {file.name} (should be v4)",
                )
                outdated_actions += 1
            if re.search(r"actions/download-artifact@(v1|v2|v3)\b", content):
                print_status(
                    YELLOW,
                    f"⚠️  Old download-artifact action version in {file.name} (should be v4)",
                )
                outdated_actions += 1
            if "actions-rs/toolchain" in content:
                print_status(
                    YELLOW,
                    f"⚠️  Deprecated actions-rs/toolchain in {file.name} (use dtolnay/rust-toolchain@v1)",
                )
                outdated_actions += 1
            if "FORCE_JAVASCRIPT_ACTIONS_TO_NODE24" not in content:
                print_status(
                    YELLOW,
                    f"⚠️  Missing FORCE_JAVASCRIPT_ACTIONS_TO_NODE24 in {file.name}",
                )
                outdated_actions += 1
        except Exception as e:  # noqa: BLE001
            print_status(RED, f"Error reading {file.name}: {e}")
    if outdated_actions == 0:
        print_status(GREEN, "✅ All action versions look up to date")
    return outdated_actions


def check_permissions(workflow_files):
    print_status(BLUE, "🔍 Checking workflow permissions...")
    permission_issues = 0
    for file in workflow_files:
        try:
            with open(file) as f:
                content = f.read()
            if "permissions: read-all" in content:
                print_status(
                    YELLOW,
                    f"⚠️  Using 'read-all' permissions in {file.name} (consider specific permissions)",
                )
                permission_issues += 1
            if (
                "contents: write" in content or "packages: write" in content
            ) and "permissions:" not in content:
                print_status(
                    YELLOW,
                    f"⚠️  Workflow needs write permissions but permissions block missing in {file.name}",
                )
                permission_issues += 1
        except Exception as e:  # noqa: BLE001
            print_status(RED, f"Error reading {file.name}: {e}")
    if permission_issues == 0:
        print_status(GREEN, "✅ Permissions look good")
    return permission_issues


def check_anti_patterns(workflow_files):
    print_status(BLUE, "🔍 Checking for common anti-patterns...")
    anti_patterns = 0
    for file in workflow_files:
        try:
            with open(file) as f:
                content = f.read()
            if "timeout-minutes:" not in content:
                print_status(YELLOW, f"⚠️  Missing timeout-minutes in {file.name}")
                anti_patterns += 1
            if "schedule:" in content and "concurrency:" not in content:
                print_status(
                    YELLOW,
                    f"⚠️  Scheduled workflow {file.name} missing concurrency control",
                )
                anti_patterns += 1
            matches = re.findall(
                r"(?:password|secret|token|key)\s*:\s*['\"][^'\"]+['\"]",
                content,
                re.IGNORECASE,
            )
            real_secrets = [m for m in matches if "${{" not in m]
            if real_secrets:
                print_status(RED, f"❌ Potential hardcoded secret in {file.name}")
                anti_patterns += len(real_secrets)
            if (
                "curl" in content or "wget" in content or "download" in content
            ) and not any(
                x in content
                for x in ["set -e", "for i in", "--retry", "continue-on-error: false"]
            ):
                print_status(
                    YELLOW,
                    f"⚠️  Network operations in {file.name} may need fail-fast retry handling",
                )
                anti_patterns += 1
        except Exception as e:  # noqa: BLE001
            print_status(RED, f"Error reading {file.name}: {e}")
    if anti_patterns == 0:
        print_status(GREEN, "✅ No common anti-patterns found")
    return anti_patterns


def check_rust_specific(workflow_files):
    print_status(BLUE, "🔍 Checking Rust-specific issues...")
    rust_issues = 0
    rust_toolchain = Path("rust-toolchain.toml")
    if rust_toolchain.exists():
        try:
            with open(rust_toolchain) as f:
                content = f.read()
            if "channel" not in content:
                print_status(
                    YELLOW, "⚠️  rust-toolchain.toml missing channel specification"
                )
                rust_issues += 1
            else:
                print_status(GREEN, "✅ rust-toolchain.toml looks good")
        except Exception as e:  # noqa: BLE001
            print_status(RED, f"Error reading rust-toolchain.toml: {e}")
            rust_issues += 1
    else:
        print_status(
            YELLOW,
            "⚠️  No rust-toolchain.toml found (recommended for consistent builds)",
        )
        rust_issues += 1

    toolchain_inconsistency = 0
    for file in workflow_files:
        try:
            with open(file) as f:
                content = f.read()
            if "dtolnay/rust-toolchain" in content:  # noqa: SIM102
                if (
                    "channel" in content
                    and "nightly" in content
                    and "nightly" not in file.name
                ):
                    print_status(
                        YELLOW,
                        f"⚠️  Using nightly Rust in {file.name} (consider stable for reliability)",
                    )
                    toolchain_inconsistency += 1
        except Exception as e:  # noqa: BLE001
            print_status(RED, f"Error reading {file.name}: {e}")

    if toolchain_inconsistency == 0:
        print_status(GREEN, "✅ Rust toolchain usage looks consistent")

    return rust_issues + toolchain_inconsistency


def main():
    print_status(BLUE, "🚀 Starting GitHub Actions workflow health check...")
    print()

    workflow_files = collect_workflow_files()
    if not workflow_files:
        print_status(RED, "❌ No workflow files found")
        sys.exit(1)

    total_issues = 0
    total_issues += check_workflow_syntax(workflow_files)
    print()
    total_issues += check_action_versions(workflow_files)
    print()
    total_issues += check_permissions(workflow_files)
    print()
    total_issues += check_anti_patterns(workflow_files)
    print()
    total_issues += check_rust_specific(workflow_files)
    print()

    if total_issues == 0:
        print_status(GREEN, "🎉 All checks passed! Your workflows look healthy.")
    else:
        print_status(YELLOW, f"⚠️  Found {total_issues} potential issues to review.")
        print_status(
            BLUE, "💡 Consider addressing these issues to improve workflow reliability."
        )

    sys.exit(total_issues)


if __name__ == "__main__":
    main()
