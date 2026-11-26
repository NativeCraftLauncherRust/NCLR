#!/usr/bin/env python3
"""Compare hzr-export output with the Java/prepared table of the same name."""
import json
import sys
from pathlib import Path

TABLES = ["block-tags.json", "damage-tags.json", "damage-types.json", "damage-catalog.json", "player-float-ids.json",
    "entity-types.json",
    "attributes.json",
    "biome-parameters/overworld.json",
    "biome-parameters/nether.json",
    "worldgen/biome_parameters/overworld.json",
    "worldgen/biome_parameters/nether.json",
    "landing-definitions.json",
    "registries.json",
    "render-definitions.json",
    "datafix-tables.json",
    "entity-models.json",
]

def main() -> int:
    if len(sys.argv) != 3:
        print("usage: compare.py <generated-dir> <ship-extract-dir>")
        return 2
    generated, reference = Path(sys.argv[1]), Path(sys.argv[2])
    failed = False
    for name in TABLES:
        left = json.loads((generated / name).read_text())
        right = json.loads((reference / name).read_text())
        ok = left == right
        print(f"{name:24} {'OK' if ok else 'DIFF'}  keys={len(left)}")
        failed |= not ok
    return 1 if failed else 0

if __name__ == "__main__":
    raise SystemExit(main())

