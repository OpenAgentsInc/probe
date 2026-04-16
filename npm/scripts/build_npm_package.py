#!/usr/bin/env python3
"""Stage and optionally pack the mac-first Probe npm package."""

from __future__ import annotations

import argparse
import json
import shutil
import subprocess
import tempfile
from pathlib import Path


SCRIPT_DIR = Path(__file__).resolve().parent
NPM_ROOT = SCRIPT_DIR.parent
REPO_ROOT = NPM_ROOT.parent
PROBE_NPM_NAME = "@openagentsinc/probe"

PROBE_PLATFORM_PACKAGES: dict[str, dict[str, str]] = {
    "probe-darwin-arm64": {
        "npm_name": "@openagentsinc/probe-darwin-arm64",
        "npm_tag": "darwin-arm64",
        "target_triple": "aarch64-apple-darwin",
        "os": "darwin",
        "cpu": "arm64",
    }
}

PACKAGE_EXPANSIONS: dict[str, list[str]] = {
    "probe": ["probe", *PROBE_PLATFORM_PACKAGES],
}

PACKAGE_NATIVE_COMPONENTS: dict[str, list[str]] = {
    "probe": ["probe"],
    "probe-darwin-arm64": ["probe"],
}

PACKAGE_TARGET_FILTERS: dict[str, str] = {
    package_name: package_config["target_triple"]
    for package_name, package_config in PROBE_PLATFORM_PACKAGES.items()
}

PACKAGE_CHOICES = tuple(PACKAGE_NATIVE_COMPONENTS)

COMPONENT_DEST_DIR: dict[str, str] = {
    "probe": "probe",
}


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Stage the Probe npm packages.")
    parser.add_argument(
        "--package",
        choices=PACKAGE_CHOICES,
        default="probe",
        help="Which npm package to stage (default: probe).",
    )
    parser.add_argument(
        "--version",
        required=True,
        help="Version number to write inside the staged package.",
    )
    parser.add_argument(
        "--staging-dir",
        type=Path,
        help=(
            "Directory to stage the package contents. Defaults to a new temporary directory "
            "if omitted. The directory must be empty when provided."
        ),
    )
    parser.add_argument(
        "--pack-output",
        type=Path,
        help="Path where the generated npm tarball should be written.",
    )
    parser.add_argument(
        "--vendor-src",
        type=Path,
        help="Directory containing native Probe payloads laid out as vendor/<target>/probe/probe.",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    staging_dir, created_temp = prepare_staging_dir(args.staging_dir)

    try:
        stage_sources(staging_dir, args.version, args.package)

        vendor_src = args.vendor_src.resolve() if args.vendor_src else None
        native_components = PACKAGE_NATIVE_COMPONENTS.get(args.package, [])
        target_filter = PACKAGE_TARGET_FILTERS.get(args.package)

        if vendor_src is not None and native_components:
            copy_native_binaries(
                vendor_src,
                staging_dir,
                native_components,
                target_filter={target_filter} if target_filter else None,
            )
        elif args.package in PROBE_PLATFORM_PACKAGES:
            raise RuntimeError(
                f"Package '{args.package}' requires --vendor-src pointing to a staged vendor tree."
            )

        if args.pack_output is not None:
            output_path = run_npm_pack(staging_dir, args.pack_output)
            print(f"npm pack output written to {output_path}")
        else:
            print(f"Staged package in {staging_dir}")
    finally:
        if created_temp:
            # Preserve the temp staging directory for inspection.
            pass

    return 0


def prepare_staging_dir(staging_dir: Path | None) -> tuple[Path, bool]:
    if staging_dir is not None:
        staging_dir = staging_dir.resolve()
        staging_dir.mkdir(parents=True, exist_ok=True)
        if any(staging_dir.iterdir()):
            raise RuntimeError(f"Staging directory {staging_dir} is not empty.")
        return staging_dir, False

    temp_dir = Path(tempfile.mkdtemp(prefix="probe-npm-stage-"))
    return temp_dir, True


def stage_sources(staging_dir: Path, version: str, package: str) -> None:
    if package == "probe":
        bin_dir = staging_dir / "bin"
        bin_dir.mkdir(parents=True, exist_ok=True)
        shutil.copy2(NPM_ROOT / "bin" / "probe.js", bin_dir / "probe.js")

        readme_src = REPO_ROOT / "README.md"
        if readme_src.exists():
            shutil.copy2(readme_src, staging_dir / "README.md")

        package_json = load_package_json(NPM_ROOT / "package.json")
        package_json["version"] = version
        package_json["files"] = ["bin"]
        package_json["optionalDependencies"] = {
            PROBE_PLATFORM_PACKAGES[platform_package]["npm_name"]: (
                f"npm:{PROBE_NPM_NAME}@"
                f"{compute_platform_package_version(version, PROBE_PLATFORM_PACKAGES[platform_package]['npm_tag'])}"
            )
            for platform_package in PACKAGE_EXPANSIONS["probe"]
            if platform_package != "probe"
        }
    elif package in PROBE_PLATFORM_PACKAGES:
        platform_package = PROBE_PLATFORM_PACKAGES[package]

        readme_src = REPO_ROOT / "README.md"
        if readme_src.exists():
            shutil.copy2(readme_src, staging_dir / "README.md")

        npm_package_json = load_package_json(NPM_ROOT / "package.json")
        package_json = {
            "name": PROBE_NPM_NAME,
            "version": compute_platform_package_version(version, platform_package["npm_tag"]),
            "license": npm_package_json.get("license", "Apache-2.0"),
            "os": [platform_package["os"]],
            "cpu": [platform_package["cpu"]],
            "files": ["vendor"],
            "repository": npm_package_json.get("repository"),
        }

        engines = npm_package_json.get("engines")
        if isinstance(engines, dict):
            package_json["engines"] = engines
    else:
        raise RuntimeError(f"Unknown package '{package}'.")

    with open(staging_dir / "package.json", "w", encoding="utf-8") as out:
        json.dump(package_json, out, indent=2)
        out.write("\n")


def load_package_json(path: Path) -> dict:
    with open(path, "r", encoding="utf-8") as fh:
        return json.load(fh)


def compute_platform_package_version(version: str, platform_tag: str) -> str:
    return f"{version}-{platform_tag}"


def copy_native_binaries(
    vendor_src: Path,
    staging_dir: Path,
    components: list[str],
    target_filter: set[str] | None = None,
) -> None:
    vendor_src = vendor_src.resolve()
    if not vendor_src.exists():
        raise RuntimeError(f"Vendor source directory not found: {vendor_src}")

    vendor_dest = staging_dir / "vendor"
    if vendor_dest.exists():
        shutil.rmtree(vendor_dest)
    vendor_dest.mkdir(parents=True, exist_ok=True)

    copied_targets: set[str] = set()
    components_set = {component for component in components if component in COMPONENT_DEST_DIR}

    for target_dir in vendor_src.iterdir():
        if not target_dir.is_dir():
            continue
        if target_filter is not None and target_dir.name not in target_filter:
            continue

        dest_target_dir = vendor_dest / target_dir.name
        dest_target_dir.mkdir(parents=True, exist_ok=True)
        copied_targets.add(target_dir.name)

        for component in components_set:
            source_component_dir = target_dir / COMPONENT_DEST_DIR[component]
            if not source_component_dir.exists():
                raise RuntimeError(
                    f"Missing native component '{component}' in vendor source: {source_component_dir}"
                )

            dest_component_dir = dest_target_dir / COMPONENT_DEST_DIR[component]
            if dest_component_dir.exists():
                shutil.rmtree(dest_component_dir)
            shutil.copytree(source_component_dir, dest_component_dir)

    if target_filter is not None:
        missing_targets = sorted(target_filter - copied_targets)
        if missing_targets:
            missing_list = ", ".join(missing_targets)
            raise RuntimeError(f"Missing target directories in vendor source: {missing_list}")


def run_npm_pack(staging_dir: Path, output_path: Path) -> Path:
    output_path = output_path.resolve()
    output_path.parent.mkdir(parents=True, exist_ok=True)

    with tempfile.TemporaryDirectory(prefix="probe-npm-pack-") as pack_dir_str:
        pack_dir = Path(pack_dir_str)
        stdout = subprocess.check_output(
            ["npm", "pack", "--json", "--pack-destination", str(pack_dir)],
            cwd=staging_dir,
            text=True,
        )
        pack_output = json.loads(stdout)
        if not pack_output:
            raise RuntimeError("npm pack did not produce an output tarball.")

        tarball_name = pack_output[0].get("filename") or pack_output[0].get("name")
        if not tarball_name:
            raise RuntimeError("Unable to determine npm pack output filename.")

        tarball_path = pack_dir / tarball_name
        if not tarball_path.exists():
            raise RuntimeError(f"Expected npm pack output not found: {tarball_path}")

        shutil.move(str(tarball_path), output_path)

    return output_path


if __name__ == "__main__":
    raise SystemExit(main())
