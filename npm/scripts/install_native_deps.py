#!/usr/bin/env python3
"""Install mac-first Probe native binaries into a vendor tree."""

from __future__ import annotations

import argparse
import gzip
import os
import shutil
import tarfile
from pathlib import Path


DEFAULT_TARGET = "aarch64-apple-darwin"
DEFAULT_BINARY_NAME = "probe"


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Install native Probe binaries into vendor/.")
    parser.add_argument(
        "--artifact-path",
        required=True,
        type=Path,
        help=(
            "Path to the compressed Probe artifact. Supports .gz, .tgz, .tar.gz, "
            "or a raw binary path."
        ),
    )
    parser.add_argument(
        "--target",
        default=DEFAULT_TARGET,
        help=f"Rust target triple to install (default: {DEFAULT_TARGET}).",
    )
    parser.add_argument(
        "--binary-name",
        default=DEFAULT_BINARY_NAME,
        help=f"Installed executable name under vendor/ (default: {DEFAULT_BINARY_NAME}).",
    )
    parser.add_argument(
        "root",
        nargs="?",
        type=Path,
        default=Path.cwd(),
        help="Directory under which vendor/ will be created (default: current directory).",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()

    artifact_path = args.artifact_path.resolve()
    if not artifact_path.exists():
        raise RuntimeError(f"Artifact not found: {artifact_path}")

    vendor_dir = args.root.resolve() / "vendor"
    dest_dir = vendor_dir / args.target / "probe"
    dest_dir.mkdir(parents=True, exist_ok=True)
    dest = dest_dir / args.binary_name

    install_probe_binary(artifact_path, dest)
    dest.chmod(0o755)

    print(f"Installed {artifact_path.name} -> {dest}")
    return 0


def install_probe_binary(artifact_path: Path, dest: Path) -> None:
    name = artifact_path.name

    if name.endswith(".tar.gz") or name.endswith(".tgz"):
        install_from_tarball(artifact_path, dest)
        return

    if name.endswith(".gz"):
        with gzip.open(artifact_path, "rb") as src, open(dest, "wb") as out:
            shutil.copyfileobj(src, out)
        return

    shutil.copy2(artifact_path, dest)


def install_from_tarball(artifact_path: Path, dest: Path) -> None:
    with tarfile.open(artifact_path, "r:gz") as archive:
        members = [member for member in archive.getmembers() if member.isfile()]
        if not members:
            raise RuntimeError(f"No file entries found in archive {artifact_path}.")

        member = members[0]
        extracted = archive.extractfile(member)
        if extracted is None:
            raise RuntimeError(f"Failed to read archive member {member.name} from {artifact_path}.")

        with extracted, open(dest, "wb") as out:
            shutil.copyfileobj(extracted, out)


if __name__ == "__main__":
    raise SystemExit(main())
