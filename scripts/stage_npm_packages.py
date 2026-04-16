#!/usr/bin/env python3
"""Stage and optionally publish the mac-first Probe npm packages."""

from __future__ import annotations

import argparse
import os
import shutil
import subprocess
import tempfile
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
BUILD_SCRIPT = REPO_ROOT / "npm" / "scripts" / "build_npm_package.py"
INSTALL_NATIVE_DEPS = REPO_ROOT / "npm" / "scripts" / "install_native_deps.py"
PACKAGES = ("probe-darwin-arm64", "probe")
PLATFORM_PUBLISH_TAGS = {
    "probe-darwin-arm64": "darwin-arm64",
    "probe": "latest",
}


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--release-version",
        required=True,
        help="Version to stage, for example 0.1.0.",
    )
    parser.add_argument(
        "--artifact-path",
        required=True,
        type=Path,
        help="Path to the compressed macOS Probe artifact that should populate vendor/.",
    )
    parser.add_argument(
        "--output-dir",
        type=Path,
        default=None,
        help="Directory where npm tarballs should be written (default: dist/npm).",
    )
    parser.add_argument(
        "--publish",
        action="store_true",
        help="Publish the staged tarballs to npm after packaging them.",
    )
    parser.add_argument(
        "--otp",
        help="One-time password for npm accounts that require 2FA when publishing.",
    )
    parser.add_argument(
        "--keep-staging-dirs",
        action="store_true",
        help="Retain temporary vendor and staging directories instead of deleting them.",
    )
    return parser.parse_args()


def run_command(cmd: list[str]) -> None:
    print("+", " ".join(cmd))
    subprocess.run(cmd, cwd=REPO_ROOT, check=True)


def tarball_name_for_package(package: str, version: str) -> str:
    if package == "probe":
        return f"probe-npm-{version}.tgz"
    return f"probe-npm-darwin-arm64-{version}.tgz"


def publish_tag_for_package(package: str) -> str:
    return PLATFORM_PUBLISH_TAGS[package]


def main() -> int:
    args = parse_args()

    output_dir = args.output_dir or (REPO_ROOT / "dist" / "npm")
    output_dir.mkdir(parents=True, exist_ok=True)
    otp = args.otp or os.environ.get("NPM_CONFIG_OTP")

    runner_temp = Path(os.environ.get("RUNNER_TEMP", tempfile.gettempdir()))
    vendor_temp_root = Path(tempfile.mkdtemp(prefix="probe-npm-native-", dir=runner_temp))
    vendor_src = vendor_temp_root / "vendor"
    tarballs: dict[str, Path] = {}

    try:
        run_command(
            [
                str(INSTALL_NATIVE_DEPS),
                "--artifact-path",
                str(args.artifact_path.resolve()),
                str(vendor_temp_root),
            ]
        )

        for package in PACKAGES:
            staging_dir = Path(tempfile.mkdtemp(prefix=f"probe-npm-stage-{package}-", dir=runner_temp))
            tarball_path = output_dir / tarball_name_for_package(package, args.release_version)

            cmd = [
                str(BUILD_SCRIPT),
                "--package",
                package,
                "--version",
                args.release_version,
                "--staging-dir",
                str(staging_dir),
                "--pack-output",
                str(tarball_path),
                "--vendor-src",
                str(vendor_src),
            ]

            try:
                run_command(cmd)
            finally:
                if not args.keep_staging_dirs:
                    shutil.rmtree(staging_dir, ignore_errors=True)

            tarballs[package] = tarball_path
            print(f"Staged {package} at {tarball_path}")

        if args.publish:
            for package in PACKAGES:
                publish_cmd = [
                    "npm",
                    "publish",
                    str(tarballs[package]),
                    "--access",
                    "public",
                    "--tag",
                    publish_tag_for_package(package),
                ]
                if otp:
                    publish_cmd.extend(["--otp", otp])
                run_command(publish_cmd)

        print("Publish order:")
        for package in PACKAGES:
            print(f"  {package}: npm tag {publish_tag_for_package(package)}")
    finally:
        if not args.keep_staging_dirs:
            shutil.rmtree(vendor_temp_root, ignore_errors=True)

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
