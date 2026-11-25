#!/bin/bash
# Clone/pin upstream Horizon and overlay the iOS port crate, its four data
# crates, and the patch that adds them.
#
# Inputs:
#   HORIZON_DIR  upstream checkout (default: sibling ../horizon)
set -euxo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
HORIZON_DIR="${HORIZON_DIR:-$ROOT/../horizon}"
PIN="69ab572dbd1bf2f2924a424f791014e09d9d0b50"

ARCHIVE="$ROOT/vendor/horizon-69ab572.tar.gz"
if [ ! -d "$HORIZON_DIR/crates/horizon-core" ]; then
    rm -rf "$HORIZON_DIR"
    mkdir -p "$HORIZON_DIR"
    tar -xzf "$ARCHIVE" -C "$HORIZON_DIR"
fi

# The port crate plus the four crates it depends on.
rm -rf "$HORIZON_DIR/crates/horizon-ios"
cp -R "$ROOT/port/horizon-ios" "$HORIZON_DIR/crates/horizon-ios"

for crate in hzr-pack hzr-fetch hzr-device hzr-download; do
    rm -rf "$HORIZON_DIR/crates/$crate"
    cp -R "$ROOT/port/$crate" "$HORIZON_DIR/crates/$crate"
done

# The patched winit carries the UIScene adoption and the late-start path that
# lets the launcher call UIApplicationMain first and start the engine on tap.
rm -rf "$HORIZON_DIR/third_party/winit"
cp -R "$ROOT/port/winit" "$HORIZON_DIR/third_party/winit"

python3 - "$HORIZON_DIR" <<'PY'
import sys
from pathlib import Path

root = Path(sys.argv[1])
members = ["horizon-ios", "hzr-pack", "hzr-fetch", "hzr-device", "hzr-download"]

# Each crate ships with a standalone `[workspace]` so it can be tested on its
# own; inside Horizon it has to join the parent workspace instead.
for crate in members:
    manifest = root / "crates" / crate / "Cargo.toml"
    lines = manifest.read_text().splitlines()
    kept = [line for line in lines if line.strip() != "[workspace]"]
    manifest.write_text("\n".join(kept).rstrip() + "\n")

workspace = root / "Cargo.toml"
text = workspace.read_text()
for crate in members:
    marker = f'"crates/{crate}"'
    if marker not in text:
        text = text.replace("members = [", "members = [\n    " + marker + ",", 1)
if "[patch.crates-io]" not in text:
    text = text.rstrip() + '\n\n[patch.crates-io]\nwinit = { path = "third_party/winit" }\n'
elif 'winit = { path = "third_party/winit" }' not in text:
    text = text.replace(
        "[patch.crates-io]",
        '[patch.crates-io]\nwinit = { path = "third_party/winit" }',
        1,
    )
workspace.write_text(text)
PY

if (cd "$HORIZON_DIR" && git apply --check "$ROOT/patches/ios-all.patch"); then
    (cd "$HORIZON_DIR" && git apply "$ROOT/patches/ios-all.patch")
elif (cd "$HORIZON_DIR" && git apply --reverse --check "$ROOT/patches/ios-all.patch"); then
    echo "iOS patch already applied"
else
    echo "iOS patch does not apply to $PIN" >&2
    exit 1
fi

echo "prepared $HORIZON_DIR at $PIN"
