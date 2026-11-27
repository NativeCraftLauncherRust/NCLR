//! Extra `items.json` component families on top of `items_data`.
//!
//! `items_data` emits the 1,312 items whose map is the universal defaults plus
//! `max_stack_size`/`rarity`, spawn eggs, dyes, sherds, music discs, bundles,
//! maps, cooking fuel and compostable. This module adds the families whose
//! remaining components are category constants visible in the jar/registry:
//!
//! * signs and hanging signs (`sign_text_front/back`, cooking fuel for the
//!   wooden variants);
//! * coloured banners (empty `banner_patterns` + cooking fuel);
//! * coloured cushions (`cushion/color` + cooking fuel);
//! * container blocks (empty `container`, wood cooking fuel for shelves/barrels/
//!   chests, stack size 1 for shulker boxes);
//! * trim materials, banner-pattern items, chicken eggs, blaze powder, potions,
//!   use cooldowns and the beehive/bee-nest block-state defaults.
//!
//! Items needing food/tool/armour/equipment/attribute codecs remain gaps.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{Map, Value};

#[path = "items_data.rs"]
mod base;

#[path = "block.rs"]
mod block;

#[path = "items.rs"]
mod items;

pub use base::Coverage;

/// Base items plus the category families this module knows exactly.
pub fn items_components(jar_path: &Path) -> Result<(BTreeMap<String, Value>, Coverage), String> {
    let (mut out, mut coverage) = base::items_components(jar_path)?;
    let ids = items::item_ids(jar_path)?;
    let registry = block::registry(jar_path)?;
    let mut by_id: BTreeMap<&str, &block::Block> = BTreeMap::new();
    for entry in &registry {
        by_id.insert(entry.id.as_str(), entry);
    }
    for id in &ids {
        if out.contains_key(id) {
            continue;
        }
        let simple = id.trim_start_matches("minecraft:");
        let mut components = defaults(simple, false);
        let mut block_item = false;
        let mut rarity = "common".to_string();
        let mut max_stack = 64i64;
        let mut blocks: Vec<&str> = Vec::new();

        if simple.ends_with("_sign") {
            block_item = true;
            max_stack = 16;
            components.insert(
                "minecraft:max_stack_size".to_string(),
                Value::Number(max_stack.into()),
            );
            components.insert(
                "minecraft:rarity".to_string(),
                Value::String(rarity.clone()),
            );
            components.insert(
                "minecraft:item_name".to_string(),
                translate(&format!("block.minecraft.{simple}")),
            );
            let sign_text = sign_text();
            components.insert("minecraft:sign_text_front".to_string(), sign_text.clone());
            components.insert("minecraft:sign_text_back".to_string(), sign_text);
            if !simple.starts_with("crimson_") && !simple.starts_with("warped_") {
                let burn = if simple.contains("hanging_sign") {
                    "cooking/time_hanging_signs"
                } else {
                    "cooking/time_wood_items_large"
                };
                components.insert("minecraft:cooking_fuel".to_string(), fuel(burn));
            }
            out.insert(id.clone(), Value::Object(components));
            continue;
        }

        if simple.ends_with("_banner") {
            block_item = true;
            max_stack = 16;
            components.insert(
                "minecraft:max_stack_size".to_string(),
                Value::Number(max_stack.into()),
            );
            components.insert(
                "minecraft:rarity".to_string(),
                Value::String(rarity.clone()),
            );
            components.insert(
                "minecraft:item_name".to_string(),
                translate(&format!("block.minecraft.{simple}")),
            );
            components.insert(
                "minecraft:banner_patterns".to_string(),
                Value::Array(Vec::new()),
            );
            components.insert(
                "minecraft:cooking_fuel".to_string(),
                fuel("cooking/time_wood_blocks"),
            );
            out.insert(id.clone(), Value::Object(components));
            continue;
        }

        if simple.ends_with("_cushion") {
            block_item = true;
            max_stack = 16;
            components.insert(
                "minecraft:max_stack_size".to_string(),
                Value::Number(max_stack.into()),
            );
            components.insert(
                "minecraft:rarity".to_string(),
                Value::String(rarity.clone()),
            );
            components.insert(
                "minecraft:cooking_fuel".to_string(),
                fuel("cooking/time_wool_slabs"),
            );
            let colour = simple.trim_end_matches("_cushion");
            components.insert(
                "minecraft:cushion/color".to_string(),
                Value::String(colour.to_string()),
            );
            out.insert(id.clone(), Value::Object(components));
            continue;
        }

        if let Some(entry) = by_id.get(id.as_str()) {
            blocks.extend(entry.chain.iter().map(String::as_str));
        }
        let has = |class: &str| blocks.iter().any(|name| *name == class);
        let container = has("ShulkerBoxBlock")
            || has("ShelfBlock")
            || has("BarrelBlock")
            || has("AbstractFurnaceBlock")
            || has("BrewingStandBlock")
            || has("ChestBlock")
            || has("HopperBlock")
            || has("DispenserBlock")
            || has("DropperBlock")
            || has("CampfireBlock")
            || has("ChiseledBookShelfBlock")
            || has("CrafterBlock")
            || has("DecoratedPotBlock");
        if container {
            block_item = true;
            max_stack = if has("ShulkerBoxBlock") { 1 } else { 64 };
            components.insert(
                "minecraft:max_stack_size".to_string(),
                Value::Number(max_stack.into()),
            );
            components.insert(
                "minecraft:rarity".to_string(),
                Value::String(rarity.clone()),
            );
            components.insert(
                "minecraft:item_name".to_string(),
                translate(&format!("block.minecraft.{simple}")),
            );
            components.insert("minecraft:container".to_string(), Value::Array(Vec::new()));
            let wood_shelf = has("ShelfBlock")
                && !simple.starts_with("crimson_")
                && !simple.starts_with("warped_");
            if wood_shelf
                || has("BarrelBlock")
                || has("ChestBlock")
                || has("ChiseledBookShelfBlock")
            {
                components.insert(
                    "minecraft:cooking_fuel".to_string(),
                    fuel("cooking/time_wood_blocks"),
                );
            }
            if has("DecoratedPotBlock") {
                let mut decorations = Map::new();
                for side in ["back", "front", "left", "right"] {
                    let mut part = Map::new();
                    part.insert(
                        "id".to_string(),
                        Value::String("minecraft:brick".to_string()),
                    );
                    decorations.insert(side.to_string(), Value::Object(part));
                }
                components.insert(
                    "minecraft:pot_decorations".to_string(),
                    Value::Object(decorations),
                );
            }
            out.insert(id.clone(), Value::Object(components));
            continue;
        }

        if let Some(material) = trim_material(simple) {
            components.insert(
                "minecraft:provides_trim_material".to_string(),
                Value::String(format!("minecraft:{material}")),
            );
            if simple == "netherite_ingot" {
                components.insert(
                    "minecraft:damage_resistant".to_string(),
                    resistant("minecraft:is_fire"),
                );
            }
        } else if simple.ends_with("_banner_pattern") {
            let pattern = simple.trim_end_matches("_banner_pattern");
            max_stack = 1;
            rarity = match pattern {
                "creeper" | "piglin" => "uncommon",
                "flow" | "guster" | "mojang" | "skull" => "rare",
                _ => "common",
            }
            .to_string();
            components.insert(
                "minecraft:provides_banner_patterns".to_string(),
                Value::String(format!("#minecraft:pattern_item/{pattern}")),
            );
        } else if let Some(variant) = chicken_variant(simple) {
            max_stack = 16;
            components.insert(
                "minecraft:chicken/variant".to_string(),
                Value::String(format!("minecraft:{variant}")),
            );
        } else if simple == "ancient_debris"
            || simple == "netherite_scrap"
            || simple == "netherite_block"
        {
            block_item = simple != "netherite_scrap";
            components.insert(
                "minecraft:damage_resistant".to_string(),
                resistant("minecraft:is_fire"),
            );
        } else if simple == "blaze_powder" {
            let mut brewing = Map::new();
            brewing.insert(
                "speed_multiplier".to_string(),
                Value::String("minecraft:brewing/speed_default".to_string()),
            );
            brewing.insert(
                "uses".to_string(),
                Value::String("minecraft:brewing/uses_default".to_string()),
            );
            components.insert("minecraft:brewing_fuel".to_string(), Value::Object(brewing));
        } else if let Some(seconds) = use_cooldown(simple) {
            max_stack = match simple {
                "ender_pearl" => 16,
                _ => 64,
            };
            let mut cooldown = Map::new();
            cooldown.insert("seconds".to_string(), Value::from(seconds));
            components.insert(
                "minecraft:use_cooldown".to_string(),
                Value::Object(cooldown),
            );
        } else if simple == "bee_nest" || simple == "beehive" {
            block_item = true;
            components.insert("minecraft:bees".to_string(), Value::Array(Vec::new()));
            let mut state = Map::new();
            state.insert("honey_level".to_string(), Value::String("0".to_string()));
            components.insert("minecraft:block_state".to_string(), Value::Object(state));
        } else if simple.ends_with("_copper_golem_statue") || simple == "copper_golem_statue" {
            block_item = true;
            let mut state = Map::new();
            state.insert(
                "copper_golem_pose".to_string(),
                Value::String("standing".to_string()),
            );
            components.insert("minecraft:block_state".to_string(), Value::Object(state));
        } else {
            continue;
        }

        if block_item {
            components.insert(
                "minecraft:item_name".to_string(),
                translate(&format!("block.minecraft.{simple}")),
            );
        } else {
            components.insert(
                "minecraft:item_name".to_string(),
                translate(&format!("item.minecraft.{simple}")),
            );
        }
        components.insert(
            "minecraft:max_stack_size".to_string(),
            Value::Number(max_stack.into()),
        );
        components.insert("minecraft:rarity".to_string(), Value::String(rarity));
        out.insert(id.clone(), Value::Object(components));
    }
    coverage.emitted = out.len();
    coverage.gaps = vec![format!(
        "{} items still need component codecs not modelled (food/tool/armour/equipment/attributes/spears/buckets/...)",
        ids.len().saturating_sub(out.len())
    )];
    Ok((out, coverage))
}

fn defaults(simple: &str, block_item: bool) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert(
        "minecraft:attack_animation".to_string(),
        Value::Object(Map::new()),
    );
    m.insert(
        "minecraft:attribute_modifiers".to_string(),
        Value::Array(Vec::new()),
    );
    m.insert(
        "minecraft:break_sound".to_string(),
        Value::String("minecraft:entity.item.break".to_string()),
    );
    m.insert(
        "minecraft:enchantments".to_string(),
        Value::Object(Map::new()),
    );
    m.insert(
        "minecraft:interact_animation".to_string(),
        Value::Object(Map::new()),
    );
    m.insert(
        "minecraft:item_model".to_string(),
        Value::String(format!("minecraft:{simple}")),
    );
    m.insert(
        "minecraft:item_name".to_string(),
        translate(&format!(
            "{}.minecraft.{simple}",
            if block_item { "block" } else { "item" }
        )),
    );
    m.insert("minecraft:lore".to_string(), Value::Array(Vec::new()));
    m.insert(
        "minecraft:max_stack_size".to_string(),
        Value::Number(64.into()),
    );
    m.insert(
        "minecraft:rarity".to_string(),
        Value::String("common".to_string()),
    );
    m.insert("minecraft:repair_cost".to_string(), Value::Number(0.into()));
    m.insert(
        "minecraft:tooltip_display".to_string(),
        Value::Object(Map::new()),
    );
    m.insert(
        "minecraft:use_effects".to_string(),
        Value::Object(Map::new()),
    );
    m
}

fn translate(key: &str) -> Value {
    let mut name = Map::new();
    name.insert("translate".to_string(), Value::String(key.to_string()));
    Value::Object(name)
}

fn resistant(tag: &str) -> Value {
    let mut m = Map::new();
    m.insert("types".to_string(), Value::String(format!("#{tag}")));
    Value::Object(m)
}

fn fuel(key: &str) -> Value {
    let mut m = Map::new();
    m.insert(
        "burn_time".to_string(),
        Value::String(format!("minecraft:{key}")),
    );
    m.insert(
        "speed_multiplier".to_string(),
        Value::String("minecraft:cooking/speed_default".to_string()),
    );
    Value::Object(m)
}

fn sign_text() -> Value {
    let mut m = Map::new();
    m.insert("color".to_string(), Value::String("black".to_string()));
    m.insert("has_glowing_text".to_string(), Value::Bool(false));
    m.insert(
        "messages".to_string(),
        Value::Array(vec![Value::String(String::new()); 4]),
    );
    Value::Object(m)
}

fn trim_material(simple: &str) -> Option<&'static str> {
    match simple {
        "amethyst_shard" => Some("amethyst"),
        "copper_ingot" => Some("copper"),
        "diamond" => Some("diamond"),
        "emerald" => Some("emerald"),
        "gold_ingot" => Some("gold"),
        "iron_ingot" => Some("iron"),
        "lapis_lazuli" => Some("lapis"),
        "netherite_ingot" => Some("netherite"),
        "quartz" => Some("quartz"),
        "redstone" => Some("redstone"),
        "resin_brick" => Some("resin"),
        _ => None,
    }
}

fn chicken_variant(simple: &str) -> Option<&'static str> {
    match simple {
        "egg" => Some("temperate"),
        "blue_egg" => Some("cold"),
        "brown_egg" => Some("warm"),
        _ => None,
    }
}

fn use_cooldown(simple: &str) -> Option<f64> {
    match simple {
        "ender_pearl" => Some(1.0),
        "wind_charge" => Some(0.5),
        _ => None,
    }
}
