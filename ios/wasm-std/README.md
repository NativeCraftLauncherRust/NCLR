# WASM standard library members

The full launcher packages this directory as `NCLR.app/horizon-wasm-std/`.
`manifest.json` records the exact member names, sizes, SHA-256 values, compiler,
SDK and build policy. The loader sorts candidates by path, validates their bytes,
deduplicates identical members and tries each candidate against a complete Mod
dependency group. Compatibility is established by symbols, function types and
API subscriptions during loading.

## Current members

| File | Bytes | Provenance and compatibility |
|---|---:|---|
| `horizon-wasm-std-1.99.1.wasm` | 652,157 | Rebuilt byte for byte with Rust 1.99.1 (`58cf8f9c35b355199727627d07ae34a4e2846d67`) and the pinned Horizon SDK. TouchController passes linking, API and initialization checks. |
| `horizon-wasm-std.wasm` | 652,933 | Historical compatibility member for core hash `Cs1SJTbsAhpUU`. Exact compiler and SDK provenance remain unverified and are recorded as null in the manifest. |

## Adding a member

1. Use the intended Mod compiler, SDK and build policy to run
   `cargo horizon-mod wasm-std /path/to/new-member.wasm`.
2. Add the file under a distinct name. Record its size and SHA-256, full
   `rustc -vV` output, SDK revision, relevant generator digest and build policy
   in `manifest.json`. Preserve existing compatibility members.
3. Run `python3 scripts/wasm-std.py ios/wasm-std` and
   `bash scripts/verify-wasm-std.sh --check-launcher ios/wasm-std` from the repo
   root. Pass additional representative `.hmod` packages to the Rust verifier
   when introducing a new compatibility family.
4. Build with `scripts/build-launcher.sh` or `ios/build.sh`. The build validates
   source resources before deleting previous outputs. It checks the final IPA
   inventory and hashes, then loads TouchController with the production loader
   using the iOS Pulley64 backend. Only a passing archive receives the requested
   output filename and its accompanying identity manifest.

`WASM_STD_DIR` defaults to this directory. An explicitly empty, missing or invalid
directory fails the build. `WASM_STD=/path/to/legacy.wasm` additionally packages
the historical single-file location. Installed older bundles containing only
that single file remain readable by the loader.

The standalone legacy launcher built by `ios/build-launcher.sh` has no game
engine. The shared engine IPA validation applies to the full launcher entries.

## Diagnostics and regression checks

The game writes discovery paths, file errors, hashes and candidate selection to
`Documents/nclr-wasm-std-diagnostics.txt`. Load and initialization refusals are
written to `Documents/nclr-wasm-refusals.txt` for the Mods page.

After building the host verifier, run `python3 scripts/test-wasm-std.py`.
Use `--ipa /path/to/NCLR.ipa` to include final archive deletion and corruption
cases. These checks exercise production linking and initialization on the host.
Actual device startup, Mod controls and gameplay require device acceptance.
