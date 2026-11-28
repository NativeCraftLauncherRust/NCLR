#!/bin/bash
# Use the production linker and API checks on the iOS interpreter backend.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
HORIZON_DIR="${HORIZON_DIR:-$ROOT/.local/horizon}"
TARGET_DIR="${CARGO_TARGET_DIR:-$HORIZON_DIR/target}"
export PATH="$HOME/.cargo/bin:$PATH"
CHECK_LAUNCHER=false
if [ "${1:-}" = --check-launcher ]; then
    CHECK_LAUNCHER=true
    shift
fi
cargo +1.100.0 build --manifest-path "$HORIZON_DIR/Cargo.toml" \
    --target-dir "$TARGET_DIR" --profile fast -p horizon-mod-wasm --features release-verifier --bin nclr-wasm-verify
if "$CHECK_LAUNCHER"; then
    PACKAGE="$ROOT/mods/touch-controller/touch_controller.hmod"
    if [ -f "$PACKAGE" ]; then
        set -- "$@" "$PACKAGE"
    elif [ -d "$ROOT/mods/touch-controller" ]; then
        cargo +1.100.0 build --manifest-path "$HORIZON_DIR/Cargo.toml" \
            --target-dir "$TARGET_DIR" --profile fast -p cargo-horizon-mod
        HORIZON_DIR="$HORIZON_DIR" TOOL="$TARGET_DIR/fast/cargo-horizon-mod" \
            bash "$ROOT/mods/touch-controller/build.sh"
        set -- "$@" "$PACKAGE"
    fi
fi
"$TARGET_DIR/fast/nclr-wasm-verify" "$@"
