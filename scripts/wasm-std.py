#!/usr/bin/env python3
"""Validate and stage the exact standard-library inventory carried by an IPA."""
import argparse
import hashlib
import json
import shutil
import sys
from pathlib import Path


def sha256(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def check_module(path):
    data = path.read_bytes()
    if data[:8] != b"\0asm\x01\0\0\0":
        raise ValueError(f"{path}: invalid WebAssembly module header")
    # Full validation and dynamic linking are performed by nclr-wasm-verify.
    if b"dylink.0" not in data:
        raise ValueError(f"{path}: missing dylink.0 shared-library metadata")


def inventory(directory, legacy=None):
    if not directory or not Path(directory).is_dir():
        raise ValueError(f"WASM_STD_DIR must name a nonempty directory: {directory!r}")
    directory = Path(directory)
    manifest = json.loads((directory / "manifest.json").read_text())
    if (not isinstance(manifest, dict) or manifest.get("schema_version") != 1
            or not isinstance(manifest.get("members"), list) or not manifest["members"]):
        raise ValueError(f"{directory}: invalid or empty standard-library manifest")
    names = set()
    for member in manifest["members"]:
        if not isinstance(member, dict):
            raise ValueError(f"{directory}: standard-library member must be an object")
        name = member["file"]
        if not isinstance(name, str) or Path(name).name != name or not name.endswith(".wasm"):
            raise ValueError(f"unsafe standard-library filename: {name!r}")
        if name in names:
            raise ValueError(f"duplicate standard-library member: {name}")
        names.add(name)
        path = directory / name
        check_module(path)
        if sha256(path) != member["sha256"] or path.stat().st_size != member["bytes"]:
            raise ValueError(f"{path}: standard-library size or SHA-256 mismatch")
        for field in ("rustc", "sdk", "build_policy", "provenance"):
            if field not in member:
                raise ValueError(f"{path}: missing {field} provenance field")
    actual = {path.name for path in directory.glob("*.wasm")}
    if actual != names:
        raise ValueError(f"{directory}: manifest inventory differs from files: {sorted(actual ^ names)}")
    if legacy:
        check_module(Path(legacy))
    return manifest


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory")
    parser.add_argument("--legacy")
    parser.add_argument("--stage", type=Path)
    parser.add_argument("--engine", type=Path)
    parser.add_argument("--root", type=Path)
    args = parser.parse_args()
    manifest = inventory(args.directory, args.legacy)
    if args.stage:
        destination = args.stage / "horizon-wasm-std"
        destination.mkdir(parents=True, exist_ok=True)
        for member in manifest["members"]:
            shutil.copyfile(Path(args.directory) / member["file"], destination / member["file"])
        shutil.copyfile(Path(args.directory) / "manifest.json", destination / "manifest.json")
        if args.legacy:
            shutil.copyfile(args.legacy, args.stage / "horizon-wasm-std.wasm")
        if args.engine:
            import subprocess

            root = args.root.resolve()
            revision = subprocess.check_output(["git", "-C", str(root), "rev-parse", "HEAD"], text=True).strip()
            metadata = {
                "schema_version": 1,
                "engine_sha256": sha256(args.engine),
                "source_commit": revision,
                "standard_library": manifest,
                "legacy_sha256": sha256(Path(args.legacy)) if args.legacy else None,
            }
            (args.stage / "nclr-build.json").write_text(json.dumps(metadata, indent=2) + "\n")
    print(f"verified {len(manifest['members'])} standard-library members in {args.directory}")


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, TypeError) as error:
        print(f"wasm standard library: {error}", file=sys.stderr)
        sys.exit(1)
