#!/usr/bin/env python3
"""Regression checks for discovery, candidate fallback and release rejection."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import zipfile

ROOT = Path(__file__).resolve().parent.parent


def run(command, success=True, contains=(), env=None):
    result = subprocess.run([str(value) for value in command], cwd=ROOT, env=env,
                            text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    if (result.returncode == 0) != success or any(value not in result.stdout for value in contains):
        raise AssertionError(f"{command}\nexit={result.returncode}\n{result.stdout}")
    return result.stdout


def uleb(value):
    result = bytearray()
    while value >= 128:
        result.append((value & 127) | 128)
        value >>= 7
    return bytes(result + bytes([value]))


def name(value):
    value = value.encode()
    return uleb(len(value)) + value


def section(kind, payload):
    return bytes([kind]) + uleb(len(payload)) + payload


def layout_fixture(path):
    # A valid shared member whose API fingerprint deliberately differs.
    module = b'\0asm\x01\0\0\0'
    module += section(0, name('dylink.0') + b'\x01\x04\0\0\0\0')
    module += section(1, b'\x02\x60\0\x01\x7e\x60\0\0')
    module += section(3, b'\x03\0\x01\x01')
    exports = [name(value) + b'\0' + bytes([index]) for index, value in
               enumerate(('horizon_layout', 'horizon_in', 'horizon_hook'))]
    module += section(7, b'\x03' + b''.join(exports))
    bodies = (b'\0\x42\0\x0b', b'\0\x0b', b'\0\x0b')
    module += section(10, b'\x03' + b''.join(uleb(len(body)) + body for body in bodies))
    path.mkdir()
    (path / 'mod.wasm').write_bytes(module)
    (path / 'horizon-mod.toml').write_text(
        "id='layout_probe'\nname='Layout probe'\nversion='1.0.0'\napi='0.2'\n"
        "capabilities=['registries']\n[wasm]\n"
        f"sha512='{hashlib.sha512(module).hexdigest()}'\ntoolchain='test'\nbuilt_with='test'\n")


def check_loader(verifier, temporary):
    directory = temporary / 'members'
    directory.mkdir()
    good = (ROOT / 'ios/wasm-std/horizon-wasm-std-1.99.1.wasm').read_bytes()
    old = (ROOT / 'ios/wasm-std/horizon-wasm-std.wasm').read_bytes()
    package = ROOT / 'mods/touch-controller/touch_controller.hmod'
    if not package.exists():
        print('SKIP TouchController checks: package is not built')
        return
    legacy = temporary / 'legacy.wasm'
    base = [verifier, directory, '--load-only', '--legacy', legacy]
    (directory / 'a.wasm').write_bytes(old)
    (directory / 'b.wasm').write_bytes(good)
    run(base + [package], contains=('a.wasm', 'which no earlier member has', 'b.wasm', 'initialization passed'))
    print('PASS incompatible first candidate falls through to compatible member')
    (directory / 'a.wasm').write_bytes(b'corrupt')
    run(base + [package], contains=('invalid wasm', 'a.wasm', 'initialization passed'))
    print('PASS corrupt first candidate is diagnosed and skipped')
    legacy.write_bytes(good)
    run(base + [package], contains=('duplicate', 'initialization passed'))
    print('PASS directory and legacy candidates are deduplicated by SHA-256')
    (directory / 'a.wasm').write_bytes(old)
    (directory / 'b.wasm').unlink()
    run(base + [package], contains=('which no earlier member has', 'legacy.wasm', 'initialization passed'))
    print('PASS incompatible directory candidates fall through to the legacy member')
    shutil.rmtree(directory)
    run(base + [package], contains=('read directory', 'initialization passed'))
    print('PASS legacy single-file installation loads without a directory or manifest')
    legacy.unlink()
    run(base + [package], success=False, contains=('no valid standard-library', str(directory), str(legacy)))
    print('PASS missing resources identify both searched locations')
    directory.mkdir()
    (directory / 'a.wasm').write_bytes(old)
    (directory / 'b.wasm').write_bytes(old + section(0, name('probe')))
    run(base + [package], success=False, contains=('a.wasm', 'b.wasm', 'package load refused'))
    print('PASS all incompatible candidates retain individual failure evidence')
    (directory / 'a.wasm').write_bytes(good)
    (directory / 'b.wasm').unlink()
    fixture = temporary / 'layout_probe'
    layout_fixture(fixture)
    run(base + [fixture], success=False, contains=('Layout', 'built for another version'))
    print('PASS API layout mismatch remains a distinct refusal')
    manifest = {'members': [{'file': 'a.wasm', 'sha256': '0' * 64}]}
    (directory / 'manifest.json').write_text(json.dumps(manifest))
    run(base + [package], success=False, contains=('SHA-256 mismatch', 'no valid standard-library'))
    print('PASS installed manifest hash mismatch rejects the changed member')


def check_preflight(temporary):
    directory = temporary / 'source'
    output = temporary / 'previous-build'
    dist = temporary / 'previous-dist'
    for path in (output, dist):
        path.mkdir()
        (path / 'sentinel').write_text('preserve')
    env = {**os.environ, 'BUILD_DIR': str(output), 'DIST_DIR': str(dist), 'WASM_STD_DIR': str(directory)}
    env.pop('WASM_STD', None)
    for label in ('missing', 'empty', 'corrupt', 'hash'):
        if label == 'empty':
            directory.mkdir()
        if label in ('corrupt', 'hash'):
            shutil.copytree(ROOT / 'ios/wasm-std', directory, dirs_exist_ok=True)
            member = directory / 'horizon-wasm-std-1.99.1.wasm'
            data = member.read_bytes()
            member.write_bytes(b'corrupt' if label == 'corrupt' else data + section(0, name('changed')))
        run(['bash', 'ios/build.sh'], success=False, env=env, contains=('wasm standard library:',))
        assert all((path / 'sentinel').read_text() == 'preserve' for path in (output, dist))
        print(f'PASS {label} source fails before previous build or IPA outputs are deleted')
    env['WASM_STD_DIR'] = ''
    run(['bash', 'ios/build.sh'], success=False, env=env, contains=('WASM_STD_DIR',))
    assert (output / 'sentinel').exists() and (dist / 'sentinel').exists()
    print('PASS explicitly empty WASM_STD_DIR fails before deleting outputs')


def check_preparation():
    # Keep the fixture inside this Git checkout to catch parent-repo discovery.
    with tempfile.TemporaryDirectory(prefix='.wasm-prepare-', dir=ROOT) as temporary:
        horizon = Path(temporary) / 'horizon'
        env = {**os.environ, 'HORIZON_DIR': str(horizon)}
        run(['bash', 'scripts/prepare-horizon.sh'], env=env)
        paths = ['src/lib.rs', 'src/runtime.rs', 'Cargo.toml',
                 'src/standard_library.rs', 'src/bin/nclr-wasm-verify.rs']
        crate = horizon / 'crates/horizon-mod-wasm'
        before = {path: (crate / path).read_bytes() for path in paths}
        run(['bash', 'scripts/prepare-horizon.sh'], env=env)
        assert before == {path: (crate / path).read_bytes() for path in paths}
        env['GIT_CEILING_DIRECTORIES'] = str(horizon.parent)
        for patch in ('pixel-touch-controls', 'mod-api-touch-input', 'ios-all'):
            command = ['git', '-C', horizon, 'apply', '--reverse']
            patch_path = ROOT / f'patches/{patch}.patch'
            run(command + ['--check', patch_path], env=env)
            run(command + [patch_path], env=env)
        print('PASS fresh vendor tree applies all patches and repeated preparation preserves loader bytes')


def check_ipa(ipa, temporary):
    with zipfile.ZipFile(ipa) as original:
        entries = [(entry, original.read(entry)) for entry in original.infolist()]
    manifest_name = next(entry.filename for entry, _ in entries if entry.filename.endswith('/horizon-wasm-std/manifest.json'))
    std_name = next(entry.filename for entry, _ in entries if entry.filename.endswith('/horizon-wasm-std/horizon-wasm-std-1.99.1.wasm'))
    for label in ('missing-member', 'missing-manifest', 'changed-member', 'rewritten-inventory'):
        candidate = temporary / f'{label}.ipa'
        with zipfile.ZipFile(candidate, 'w', zipfile.ZIP_DEFLATED) as output:
            for entry, data in entries:
                if entry.filename == std_name and label in ('missing-member', 'rewritten-inventory'):
                    continue
                if entry.filename == manifest_name:
                    if label == 'missing-manifest':
                        continue
                    if label == 'rewritten-inventory':
                        manifest = json.loads(data)
                        manifest['members'] = [m for m in manifest['members'] if m['file'] != Path(std_name).name]
                        data = json.dumps(manifest).encode()
                if entry.filename == std_name and label == 'changed-member':
                    data += section(0, name('changed'))
                output.writestr(entry, data)
        run(['python3', 'scripts/verify-wasm-ipa.py', candidate], success=False, contains=('IPA WASM check failed:',))
        assert not candidate.with_suffix('.manifest.json').exists()
        print(f'PASS final IPA rejects {label}')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--verifier', type=Path, default=ROOT / '.local/horizon/target/fast/nclr-wasm-verify')
    parser.add_argument('--ipa', type=Path)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='nclr-wasm-regression-') as temporary:
        temporary = Path(temporary)
        check_loader(args.verifier, temporary)
        check_preflight(temporary)
        check_preparation()
        if args.ipa:
            check_ipa(args.ipa, temporary)


if __name__ == '__main__':
    main()
