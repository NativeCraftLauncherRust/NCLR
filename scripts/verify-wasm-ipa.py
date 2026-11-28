#!/usr/bin/env python3
"""Check final IPA resources and run the production WASM loader before publication."""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import zipfile

ROOT = Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location("wasm_std", ROOT / "scripts/wasm-std.py")
wasm_std = importlib.util.module_from_spec(spec)
spec.loader.exec_module(wasm_std)


def verify(ipa, expected):
    expected_manifest = wasm_std.inventory(expected)
    with tempfile.TemporaryDirectory(prefix="nclr-wasm-ipa-") as temporary:
        bundle = Path(temporary)
        with zipfile.ZipFile(ipa) as archive:
            names = archive.namelist()
            if len(names) != len(set(names)):
                raise ValueError("IPA has duplicate zip entries")
            roots = {match[1] for name in names if (match := re.match(r"^(Payload/[^/]+\.app)/Info\.plist$", name))}
            if len(roots) != 1:
                raise ValueError("IPA must contain exactly one app bundle")
            app = roots.pop()
            for name in names:
                relative = name.removeprefix(app + "/")
                if name == f"{app}/{relative}" and (
                    relative.startswith("horizon-wasm-std/")
                    or relative in ("horizon-wasm-std.wasm", "nclr-build.json")
                ) and not name.endswith("/"):
                    parts = Path(relative).parts
                    if ".." in parts or len(parts) > 2:
                        raise ValueError(f"unsafe standard-library resource: {name}")
                    destination = bundle / relative
                    destination.parent.mkdir(parents=True, exist_ok=True)
                    destination.write_bytes(archive.read(name))
        directory = bundle / "horizon-wasm-std"
        legacy = bundle / "horizon-wasm-std.wasm"
        actual = wasm_std.inventory(directory, legacy if legacy.exists() else None)
        if actual != expected_manifest:
            raise ValueError("IPA standard-library manifest differs from the required source inventory")
        build = json.loads((bundle / "nclr-build.json").read_text())
        if build.get("schema_version") != 1 or build.get("standard_library") != actual:
            raise ValueError("engine build identity differs from the packaged standard-library inventory")
        if not re.fullmatch(r"[0-9a-f]{64}", build.get("engine_sha256", "")):
            raise ValueError("missing engine SHA-256 build identity")
        legacy_hash = wasm_std.sha256(legacy) if legacy.exists() else None
        if build.get("legacy_sha256") != legacy_hash:
            raise ValueError("legacy standard-library resource differs from build identity")
        command = ["bash", str(ROOT / "scripts/verify-wasm-std.sh"), "--check-launcher", str(directory)]
        if legacy.exists():
            command.extend(["--legacy", str(legacy)])
        checked = subprocess.run(command, check=True, text=True, stdout=subprocess.PIPE)
        print(checked.stdout, end="")
        package = ROOT / "mods/touch-controller/touch_controller.hmod"
        runtime = {
            "backend": "pulley64",
            "checks": ["loader", "candidate inventory", "shared-library link"],
            "output": checked.stdout,
        }
        if package.exists():
            if "package linking, API and initialization passed for 1 packages" not in checked.stdout:
                raise ValueError("runtime verifier did not confirm representative package initialization")
            runtime["checks"].extend(["production linker", "API", "initialization"])
            runtime["package"] = "touch_controller.hmod"
            runtime["package_sha256"] = wasm_std.sha256(package)
        manifest = {
            "ipa": ipa.name,
            "sha256": hashlib.sha256(ipa.read_bytes()).hexdigest(),
            "engine": build,
            "wasm_runtime_check": runtime,
        }
        ipa.with_suffix(".manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
        print(f"final IPA WASM check passed: {ipa}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("ipa", type=Path)
    parser.add_argument("--expected-directory", default=os.environ.get("WASM_STD_DIR", str(ROOT / "ios/wasm-std")))
    args = parser.parse_args()
    verify(args.ipa, args.expected_directory)


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, TypeError, zipfile.BadZipFile, subprocess.CalledProcessError) as error:
        print(f"IPA WASM check failed: {error}", file=sys.stderr)
        sys.exit(1)
