# WASM standard library members

`horizon-wasm-std/*.wasm` are the standard-library members shipped beside the
executable. A wasm mod imports its Rust core/data symbols by hash, and that hash
depends on the rustc that built the mod. One std cannot link every mod, so the
loader tries each file here for a mod group and keeps the first that links.

## Adding a member

1. Build `horizon-wasm-std.wasm` with the same toolchain as the mods it must
   serve (the SDK's `cargo horizon-mod wasm-std` command).
2. Drop it into this directory with a distinct file name, e.g.
   `horizon-wasm-std-1.100.0.wasm`.
3. Rebuild the IPA. `ios/build.sh` copies every `*.wasm` file into
   `NCLR.app/horizon-wasm-std/`.

Do not overwrite an existing member unless you know no installed mod needs its
core hash. If no member links, the mod is refused as a whole and the reason is
written to `Documents/nclr-wasm-refusals.txt` and shown on the Mods page.

## Current members

| File | Bytes | Serves |
|---|---:|---|
| `horizon-wasm-std.wasm` | 652,933 | farmersdelight (core hash `Cs1SJTbsAhpUU`) |
| `horizon-wasm-std-1.99.1.wasm` | 652,157 | alternate rustc core hash `CsiancVaMSlX` |
