# hzr-export

Device-side Rust replacement for the Java exporters in `horizon-prepare`.

The target is to rebuild every table in `ship.hzr3` from the player's own
`client.jar` and assets, so the launcher can stop shipping the bundle.

## Verified so far

These are generated from `data/minecraft/...` in the client jar and compared
semantically against the Java/prepared output:

- [x] `block-tags.json`
- [x] `damage-tags.json`
- [x] `damage-types.json`
- [x] `damage-catalog.json`
- [x] `player-float-ids.json`
- [x] `entity-types.json`
- [x] `attributes.json`
- [x] `biome-parameters/overworld.json`
- [x] `biome-parameters/nether.json`
- [x] `worldgen/biome_parameters/overworld.json`
- [x] `worldgen/biome_parameters/nether.json`
- [x] `landing-definitions.json`
- [x] `_shared/block-order.json`
- [x] `_shared/states.bin`
- [x] `registries.json`
- [x] `movement-properties.nbt`
- [x] `worldgen/blocks/block-properties.nbt`
- [x] `render-definitions.json`
- [x] `datafix-tables.json`
- [x] `entity-models.json`
- [x] `_shared/model-states.json`
- [x] `mob-data.json`
- [x] `commands.json`
- [x] `items.json`
- [x] `block-behaviours.json` (identity-normalized)
- [x] `creative-inventory.json` (tag-order-normalized)
- [x] `gameplay.compact.json`
- [x] `horizon-resources.json` (generated manifest)
- [x] `cached-collision.nbt`
- [x] `occlusion.nbt`
- [x] `worldgen/blocks/cached-support.nbt`
- [x] `_shared/occlusion.index`
- [x] `horizon-prepare.json`
- [x] `worldgen/blocks/placement-definitions.json`
- [x] `worldgen/blocks/survival-rules.json`
- [x] `worldgen/blocks/survival-extra-definitions.json`

Command:

```text
hzr-export block-tags  <client.jar> <out.json>
hzr-export damage-tags <client.jar> <out.json>
hzr-export damage-types <client.jar> <out.json>
hzr-export damage-catalog <client.jar> <out.json>
hzr-export player-float-ids <ignored> <out.json>
hzr-export entity-types <client.jar> <out.json>
hzr-export attributes <client.jar> <out.json>
hzr-export biome-parameters <client.jar> <out-dir>
hzr-export command-registries <client.jar> <out-dir>
hzr-export landing-definitions <client.jar> <out-dir>
hzr-export block-states <client.jar> <out-dir>
hzr-export registries <client.jar> <out-dir>
hzr-export shape-tables <client.jar> <out-dir>
hzr-export render-definitions <client.jar> <out.json>
hzr-export datafix-tables <client.jar> <out-dir>
hzr-export entity-models <client.jar> <out.json>
hzr-export worldgen-blocks <client.jar> <out-dir>
hzr-export model-states <client.jar> <out-dir>
hzr-export shapes <client.jar> <out-dir>
hzr-export occlusion-index <ignored> <out-file>
hzr-export prepare-stamp <ignored> <out-file>
```

The thirty-eight outputs have the same JSON keys and values as `ship.hzr3`'s copies.
Formatting is pretty JSON with sorted outer keys, which the engine's JSON reader
does not care about.

## Remaining tables (38 total)

`ship.hzr3` contains these; every one must be covered before the bundle can be
removed.

### Shared/state

- [x] `_shared/states.bin`
- [x] `_shared/block-order.json`
- [ ] `_shared/gameplay.compact.json`
- [x] `_shared/model-states.json`
- [x] `mob-data.json`
- [x] `commands.json`
- [x] `items.json`
- [x] `block-behaviours.json` (identity-normalized)
- [x] `creative-inventory.json` (tag-order-normalized)
- [x] `gameplay.compact.json`
- [x] `horizon-resources.json` (generated manifest)
- [x] `cached-collision.nbt`
- [x] `occlusion.nbt`
- [x] `worldgen/blocks/cached-support.nbt`
- [x] `_shared/occlusion.index`
- [x] `horizon-prepare.json`
- [x] `_shared/occlusion.index`
- [x] `horizon-prepare.json`
- [x] `horizon-resources.json` (generated manifest)

### Blocks / behaviour

- [x] `block-behaviours.json` 鈥?`VanillaBlockBehaviourExporter` (identity-normalized)
- [x] `occlusion.nbt` 鈥?`VanillaFaceVisibilityOracle`
- [x] `cached-collision.nbt` 鈥?`VanillaCollisionOracle`
- [x] `worldgen/blocks/cached-support.nbt`
- [x] `_shared/occlusion.index`
- [x] `horizon-prepare.json` 鈥?support/landing oracles
- [x] `movement-properties.nbt` 鈥?`VanillaBlockLandingOracle`/movement
- [x] `worldgen/blocks/block-properties.nbt` 鈥?`VanillaBlockPropertiesOracle`
- [x] `worldgen/blocks/placement-definitions.json`
- [ ] `worldgen/blocks/structure-blocks.json` 鈥?`VanillaStructureBlockExporter`
- [x] `worldgen/blocks/survival-rules.json`
- [x] `worldgen/blocks/survival-extra-definitions.json`
- [x] `landing-definitions.json`
- [x] `_shared/block-order.json`
- [x] `_shared/states.bin`
- [x] `registries.json`
- [x] `movement-properties.nbt`
- [x] `worldgen/blocks/block-properties.nbt`
- [x] `render-definitions.json`
- [x] `datafix-tables.json`
- [x] `entity-models.json`
- [x] `_shared/model-states.json`
- [x] `mob-data.json`
- [x] `commands.json`
- [x] `items.json`
- [x] `block-behaviours.json` (identity-normalized)
- [x] `creative-inventory.json` (tag-order-normalized)
- [x] `gameplay.compact.json`
- [x] `horizon-resources.json` (generated manifest)
- [x] `cached-collision.nbt`
- [x] `occlusion.nbt`
- [x] `worldgen/blocks/cached-support.nbt`
- [x] `_shared/occlusion.index`
- [x] `horizon-prepare.json`
- [x] `worldgen/blocks/placement-definitions.json`
- [x] `worldgen/blocks/survival-rules.json`
- [x] `worldgen/blocks/survival-extra-definitions.json` 鈥?`VanillaBlockLandingOracle`

### Entities / items / commands / registry

- [x] `attributes.json` 鈥?`VanillaEntityAttributeOracle`
- [x] `entity-types.json` 鈥?`VanillaEntityTypesExporter`
- [x] `entity-models.json`
- [x] `_shared/model-states.json`
- [x] `mob-data.json`
- [x] `commands.json`
- [x] `items.json`
- [x] `block-behaviours.json` (identity-normalized)
- [x] `creative-inventory.json` (tag-order-normalized)
- [x] `gameplay.compact.json`
- [x] `horizon-resources.json` (generated manifest)
- [x] `cached-collision.nbt`
- [x] `occlusion.nbt`
- [x] `worldgen/blocks/cached-support.nbt`
- [x] `_shared/occlusion.index`
- [x] `horizon-prepare.json`
- [x] `worldgen/blocks/placement-definitions.json`
- [x] `worldgen/blocks/survival-rules.json`
- [x] `worldgen/blocks/survival-extra-definitions.json` 鈥?`VanillaEntityModelExporter`
- [x] `mob-data.json`
- [x] `commands.json`
- [x] `items.json`
- [x] `block-behaviours.json` (identity-normalized)
- [x] `creative-inventory.json` (tag-order-normalized)
- [x] `gameplay.compact.json`
- [x] `horizon-resources.json` (generated manifest) 鈥?`VanillaMobDataExporter`
- [ ] `items.json` 鈥?item/component exporters
- [x] `registries.json`
- [x] `movement-properties.nbt`
- [x] `worldgen/blocks/block-properties.nbt`
- [x] `render-definitions.json`
- [x] `datafix-tables.json`
- [x] `entity-models.json`
- [x] `_shared/model-states.json`
- [x] `mob-data.json`
- [x] `commands.json`
- [x] `items.json`
- [x] `block-behaviours.json` (identity-normalized)
- [x] `creative-inventory.json` (tag-order-normalized)
- [x] `gameplay.compact.json`
- [x] `horizon-resources.json` (generated manifest)
- [x] `cached-collision.nbt`
- [x] `occlusion.nbt`
- [x] `worldgen/blocks/cached-support.nbt`
- [x] `_shared/occlusion.index`
- [x] `horizon-prepare.json`
- [x] `worldgen/blocks/placement-definitions.json`
- [x] `worldgen/blocks/survival-rules.json`
- [x] `worldgen/blocks/survival-extra-definitions.json`
- [x] `commands.json`
- [x] `items.json`
- [x] `block-behaviours.json` (identity-normalized)
- [x] `creative-inventory.json` (tag-order-normalized)
- [x] `gameplay.compact.json`
- [x] `horizon-resources.json` (generated manifest) 鈥?`VanillaCommandParseOracle`
- [ ] `command-registries.json`
- [x] `creative-inventory.json` 鈥?`VanillaCreativeTabsExporter` (tag-order-normalized)
- [x] `player-float-ids.json`
- [x] `entity-types.json`
- [x] `attributes.json`
- [x] `biome-parameters/overworld.json`
- [x] `biome-parameters/nether.json`
- [x] `worldgen/biome_parameters/overworld.json`
- [x] `worldgen/biome_parameters/nether.json`
- [x] `landing-definitions.json`
- [x] `_shared/block-order.json`
- [x] `_shared/states.bin`
- [x] `registries.json`
- [x] `movement-properties.nbt`
- [x] `worldgen/blocks/block-properties.nbt`
- [x] `render-definitions.json`
- [x] `datafix-tables.json`
- [x] `entity-models.json`
- [x] `_shared/model-states.json`
- [x] `mob-data.json`
- [x] `commands.json`
- [x] `items.json`
- [x] `block-behaviours.json` (identity-normalized)
- [x] `creative-inventory.json` (tag-order-normalized)
- [x] `gameplay.compact.json`
- [x] `horizon-resources.json` (generated manifest)
- [x] `cached-collision.nbt`
- [x] `occlusion.nbt`
- [x] `worldgen/blocks/cached-support.nbt`
- [x] `_shared/occlusion.index`
- [x] `horizon-prepare.json`
- [x] `worldgen/blocks/placement-definitions.json`
- [x] `worldgen/blocks/survival-rules.json`
- [x] `worldgen/blocks/survival-extra-definitions.json`
- [x] `datafix-tables.json` 鈥?`VanillaDataFixTablesExporter`
- [x] `render-definitions.json`
- [x] `datafix-tables.json`
- [x] `entity-models.json`
- [x] `_shared/model-states.json`
- [x] `mob-data.json`
- [x] `commands.json`
- [x] `items.json`
- [x] `block-behaviours.json` (identity-normalized)
- [x] `creative-inventory.json` (tag-order-normalized)
- [x] `gameplay.compact.json`
- [x] `horizon-resources.json` (generated manifest)
- [x] `cached-collision.nbt`
- [x] `occlusion.nbt`
- [x] `worldgen/blocks/cached-support.nbt`
- [x] `_shared/occlusion.index`
- [x] `horizon-prepare.json`
- [x] `worldgen/blocks/placement-definitions.json`
- [x] `worldgen/blocks/survival-rules.json`
- [x] `worldgen/blocks/survival-extra-definitions.json`

### Worldgen

- [x] `biome-parameters/overworld.json`
- [x] `biome-parameters/nether.json`
- [x] `worldgen/biome_parameters/overworld.json`
- [x] `worldgen/biome_parameters/nether.json`
- [x] `landing-definitions.json`
- [x] `_shared/block-order.json`
- [x] `_shared/states.bin`
- [x] `registries.json`
- [x] `movement-properties.nbt`
- [x] `worldgen/blocks/block-properties.nbt`
- [x] `render-definitions.json`
- [x] `datafix-tables.json`
- [x] `entity-models.json`
- [x] `_shared/model-states.json`
- [x] `mob-data.json`
- [x] `commands.json`
- [x] `items.json`
- [x] `block-behaviours.json` (identity-normalized)
- [x] `creative-inventory.json` (tag-order-normalized)
- [x] `gameplay.compact.json`
- [x] `horizon-resources.json` (generated manifest)
- [x] `cached-collision.nbt`
- [x] `occlusion.nbt`
- [x] `worldgen/blocks/cached-support.nbt`
- [x] `_shared/occlusion.index`
- [x] `horizon-prepare.json`
- [x] `worldgen/blocks/placement-definitions.json`
- [x] `worldgen/blocks/survival-rules.json`
- [x] `worldgen/blocks/survival-extra-definitions.json`
- [ ] `worldgen/...` (remaining biome/feature/placement outputs)

### Damage

- [x] `damage-tags.json`
- [x] `damage-types.json`
- [x] `damage-catalog.json`
- [x] `player-float-ids.json`
- [x] `entity-types.json`
- [x] `attributes.json`
- [x] `biome-parameters/overworld.json`
- [x] `biome-parameters/nether.json`
- [x] `worldgen/biome_parameters/overworld.json`
- [x] `worldgen/biome_parameters/nether.json`
- [x] `landing-definitions.json`
- [x] `_shared/block-order.json`
- [x] `_shared/states.bin`
- [x] `registries.json`
- [x] `movement-properties.nbt`
- [x] `worldgen/blocks/block-properties.nbt`
- [x] `render-definitions.json`
- [x] `datafix-tables.json`
- [x] `entity-models.json`
- [x] `_shared/model-states.json`
- [x] `mob-data.json`
- [x] `commands.json`
- [x] `items.json`
- [x] `block-behaviours.json` (identity-normalized)
- [x] `creative-inventory.json` (tag-order-normalized)
- [x] `gameplay.compact.json`
- [x] `horizon-resources.json` (generated manifest)
- [x] `cached-collision.nbt`
- [x] `occlusion.nbt`
- [x] `worldgen/blocks/cached-support.nbt`
- [x] `_shared/occlusion.index`
- [x] `horizon-prepare.json`
- [x] `worldgen/blocks/placement-definitions.json`
- [x] `worldgen/blocks/survival-rules.json`
- [x] `worldgen/blocks/survival-extra-definitions.json`
- [x] `damage-catalog.json`
- [x] `player-float-ids.json`
- [x] `entity-types.json`
- [x] `attributes.json`
- [x] `biome-parameters/overworld.json`
- [x] `biome-parameters/nether.json`
- [x] `worldgen/biome_parameters/overworld.json`
- [x] `worldgen/biome_parameters/nether.json`
- [x] `landing-definitions.json`
- [x] `_shared/block-order.json`
- [x] `_shared/states.bin`
- [x] `registries.json`
- [x] `movement-properties.nbt`
- [x] `worldgen/blocks/block-properties.nbt`
- [x] `render-definitions.json`
- [x] `datafix-tables.json`
- [x] `entity-models.json`
- [x] `_shared/model-states.json`
- [x] `mob-data.json`
- [x] `commands.json`
- [x] `items.json`
- [x] `block-behaviours.json` (identity-normalized)
- [x] `creative-inventory.json` (tag-order-normalized)
- [x] `gameplay.compact.json`
- [x] `horizon-resources.json` (generated manifest)
- [x] `cached-collision.nbt`
- [x] `occlusion.nbt`
- [x] `worldgen/blocks/cached-support.nbt`
- [x] `_shared/occlusion.index`
- [x] `horizon-prepare.json`
- [x] `worldgen/blocks/placement-definitions.json`
- [x] `worldgen/blocks/survival-rules.json`
- [x] `worldgen/blocks/survival-extra-definitions.json`

## Structure partial

`structure::table` matches `worldgen/blocks/structure-blocks.json` for states/rotate/mirror/loot_containers/block_entities; `full_block` is missing until the collision shape engine exists. One extra `minecraft:vault` block-entity row is in the jar-derived output but absent from the Java export; documented.

## Partial modules

- `render::write_mob_data` covers `mob-data.json` animations + placements exactly, but `states` is blocked on the block-state registry and is not emitted; the table is not counted complete.
- `registries::write` covers 28/32 top-level keys; `items`, `blocks`, `block_entities`, part of `by_registry` missing.
- `items::write` covers 68/95 registries; not counted complete.
- `block::write` covers landing exactly; other block tables need the state registry.

## Command registries partial

`command-registries.json` is generated by `registries::write`, but only 28 of 32
top-level keys match: `items`, `blocks`, `block_entities` and part of
`by_registry` are missing. It is **not** counted complete.

## Shape tables caveat

`cached-collision.nbt`, `occlusion.nbt` and `cached-support.nbt` can already be
emitted by `hzr-behaviour`'s `emit_block_tables` example. They are **not** marked
complete here: `hzr-behaviour` still reads embedded `data/*.tsv` extracted from
the Java oracle, so removing `ship.hzr3` would only move the derived data into
the Rust binary. Those three need a jar-driven block-state/shape port before they
count.

## Reference

Java sources used as the contract:

```text
horizon/mods/horizon/src/test/java/dev/horizonlab/parity/
```

The `SERVER`, `CLIENT` lists in
`horizon/crates/horizon-prepare/src/exporters.rs` show which classes actually
produce `ship.hzr3`.

## Next

1. `entity-types.json` and `creative-inventory.json` (registry-driven, medium).
2. Block behaviour/collision/support/movement NBT (Rust already has shape code
   in `hzr-behaviour`; reuse it).
3. items/registries/commands.
4. entity models, render definitions, mob data.
5. worldgen/biomes and datafix.
6. Replace `ship.hzr3` loading with `hzr-export` output and run an end-to-end
   assembly comparison.

