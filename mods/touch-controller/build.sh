#!/bin/bash
# Builds touch_controller.hmod against the local Horizon checkout.
#
# The mod uses the `touch_input` hook this repository adds to the mod API, so it
# builds against `$HORIZON_DIR` (prepared by scripts/prepare-horizon.sh), not
# against a released SDK: the project's Cargo.toml names the SDK path a player's
# `cargo horizon-mod` would fetch, and this script points that at the checkout.
#
# Inputs:
#   HORIZON_DIR  prepared Horizon checkout (default: .local/horizon)
#   TOOL         the cargo-horizon-mod binary (default: the checkout's debug build)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
PROJECT="$ROOT/mods/touch-controller"
HORIZON_DIR="${HORIZON_DIR:-$ROOT/.local/horizon}"
STAGE="$HORIZON_DIR/target/mod-build/touch-controller"
TOOL="${TOOL:-$HORIZON_DIR/target/debug/cargo-horizon-mod}"

# The pinned wasm toolchain (1.99.1 with rust-src) lives in the repository, not
# in the player's home, so the build needs no network and no writable ~/.rustup.
export RUSTUP_HOME="${RUSTUP_HOME:-$ROOT/.local/rustup}"
export CARGO_HOME="${CARGO_HOME:-$ROOT/.local/cargo-home}"
export PATH="$HOME/.cargo/bin:$PATH"

test -d "$HORIZON_DIR/crates/horizon-mod-api" || {
    echo "no Horizon checkout at $HORIZON_DIR: run scripts/prepare-horizon.sh" >&2
    exit 1
}
if [ ! -x "$TOOL" ]; then
    (cd "$HORIZON_DIR" && cargo build -p cargo-horizon-mod)
fi

mkdir -p "$STAGE"
rsync -a --delete --exclude target --exclude .horizon --exclude '*.hmod' \
    --exclude Cargo.lock "$PROJECT/" "$STAGE/"
sed -i '' 's|path = ".horizon/sdk/api/crates/horizon-mod-api"|path = "../../../crates/horizon-mod-api"|' \
    "$STAGE/Cargo.toml"

(cd "$STAGE" && "$TOOL" horizon-mod build --wasm)
cp "$STAGE/target/horizon-mod/touch_controller.hmod" "$PROJECT/touch_controller.hmod"

# Keep the source manifest pinned to the bytes just built: the next build reads
# the same [wasm] block, and a diff says what changed.
python3 - "$STAGE/horizon-mod.toml" "$PROJECT/horizon-mod.toml" <<'PY'
import sys

staged, project = sys.argv[1], sys.argv[2]
wasm = open(staged).read().split("[wasm]", 1)[1]
head = open(project).read().split("[wasm]", 1)[0].rstrip()
open(project, "w").write(head + "\n\n[wasm]" + wasm)
PY

echo "built $PROJECT/touch_controller.hmod"
shasum -a 256 "$PROJECT/touch_controller.hmod"
