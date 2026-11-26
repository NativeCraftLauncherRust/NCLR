//! Final `items.json` singleton component records on top of `items_more`.
//!
//! `items_more` closes the category families; this module adds the handful of
//! items whose remaining components are fixed records visible in the jar's
//! item registration:
//!
//! * potion variants (potion/splash/lingering/tipped arrow);
//! * shield and elytra;
//! * glint overrides (end crystal, experience bottle, written book, nether star);
//! * firework rocket, goat horn, knowledge book, writable book;
//! * fish/axolotl/tadpole/sulfur-cube buckets, milk bucket, ominous bottle.
//!
//! The remaining item gaps (tools with tool rules, armour/equipment attribute
//! records, spears, food tables) need the `Item$Properties` component codecs
//! executed; they stay reported rather than guessed.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{Map, Value};

#[path = "items_more.rs"]
mod more;

#[path = "items.rs"]
mod items;

pub use more::Coverage;

/// `items_more` plus the singleton component families.
pub fn items_components(jar_path: &Path) -> Result<(BTreeMap<String, Value>, Coverage), String> {
    let (mut out, mut coverage) = more::items_components(jar_path)?;
    let ids = items::item_ids(jar_path)?;
    for id in &ids {
        if out.contains_key(id) {
            continue;
        }
        let simple = id.trim_start_matches("minecraft:");
        let mut components = defaults(simple);
        let mut max_stack = 64i64;
        let mut rarity = "common".to_string();
        let mut changed = true;
        match simple {
            "potion" => {
                max_stack = 1;
                components.insert(
                    "minecraft:potion_contents".to_string(),
                    Value::Object(Map::new()),
                );
                components.insert("minecraft:consumable".to_string(), drink(false, None));
                components.insert(
                    "minecraft:use_remainder".to_string(),
                    item_stack("minecraft:glass_bottle"),
                );
            }
            "splash_potion" => {
                max_stack = 1;
                components.insert(
                    "minecraft:potion_contents".to_string(),
                    Value::Object(Map::new()),
                );
            }
            "lingering_potion" => {
                max_stack = 1;
                components.insert(
                    "minecraft:potion_contents".to_string(),
                    Value::Object(Map::new()),
                );
                components.insert(
                    "minecraft:potion_duration_scale".to_string(),
                    Value::from(0.25),
                );
            }
            "tipped_arrow" => {
                max_stack = 64;
                components.insert(
                    "minecraft:potion_contents".to_string(),
                    Value::Object(Map::new()),
                );
                components.insert(
                    "minecraft:potion_duration_scale".to_string(),
                    Value::from(0.125),
                );
            }
            "shield" => {
                max_stack = 1;
                components.insert(
                    "minecraft:break_sound".to_string(),
                    Value::String("minecraft:item.shield.break".to_string()),
                );
                components.insert(
                    "minecraft:banner_patterns".to_string(),
                    Value::Array(Vec::new()),
                );
                components.insert("minecraft:blocks_attacks".to_string(), blocks_attacks());
                components.insert("minecraft:damage".to_string(), Value::Number(0.into()));
                components.insert(
                    "minecraft:equippable".to_string(),
                    equippable("offhand", false),
                );
                components.insert(
                    "minecraft:max_damage".to_string(),
                    Value::Number(336.into()),
                );
                components.insert(
                    "minecraft:repairable".to_string(),
                    repairable("#minecraft:wooden_tool_materials"),
                );
            }
            "elytra" => {
                max_stack = 1;
                rarity = "epic".to_string();
                components.insert("minecraft:damage".to_string(), Value::Number(0.into()));
                components.insert("minecraft:equippable".to_string(), elytra_equippable());
                components.insert("minecraft:glider".to_string(), Value::Object(Map::new()));
                components.insert(
                    "minecraft:max_damage".to_string(),
                    Value::Number(432.into()),
                );
                components.insert(
                    "minecraft:repairable".to_string(),
                    item_tag("minecraft:phantom_membrane"),
                );
            }
            "end_crystal" | "experience_bottle" | "written_book" | "nether_star" => {
                if simple == "written_book" {
                    max_stack = 16;
                }
                if simple == "experience_bottle" {
                    rarity = "uncommon".to_string();
                }
                if simple == "nether_star" {
                    rarity = "rare".to_string();
                    components.insert(
                        "minecraft:damage_resistant".to_string(),
                        resistant("minecraft:is_explosion"),
                    );
                }
                components.insert(
                    "minecraft:enchantment_glint_override".to_string(),
                    Value::Bool(true),
                );
            }
            "firework_rocket" => {
                let mut fireworks = Map::new();
                fireworks.insert("flight_duration".to_string(), Value::Number(1.into()));
                components.insert("minecraft:fireworks".to_string(), Value::Object(fireworks));
            }
            "goat_horn" => {
                max_stack = 1;
                rarity = "uncommon".to_string();
                components.insert(
                    "minecraft:instrument".to_string(),
                    Value::String("minecraft:ponder_goat_horn".to_string()),
                );
            }
            "knowledge_book" => {
                max_stack = 1;
                rarity = "epic".to_string();
                components.insert("minecraft:recipes".to_string(), Value::Array(Vec::new()));
            }
            "writable_book" => {
                max_stack = 1;
                components.insert(
                    "minecraft:writable_book_content".to_string(),
                    Value::Object(Map::new()),
                );
            }
            "milk_bucket" => {
                max_stack = 1;
                components.insert(
                    "minecraft:consumable".to_string(),
                    drink(false, Some("minecraft:clear_all_effects")),
                );
                components.insert(
                    "minecraft:use_remainder".to_string(),
                    item_stack("minecraft:bucket"),
                );
            }
            "ominous_bottle" => {
                max_stack = 64;
                rarity = "uncommon".to_string();
                components.insert(
                    "minecraft:ominous_bottle_amplifier".to_string(),
                    Value::Number(0.into()),
                );
                components.insert(
                    "minecraft:consumable".to_string(),
                    drink(false, Some("minecraft:play_sound")),
                );
            }
            "cod_bucket" | "salmon_bucket" => {
                max_stack = 1;
                components.insert(
                    "minecraft:bucket_entity_data".to_string(),
                    Value::Object(Map::new()),
                );
                components.insert("minecraft:food".to_string(), food(2, 0.4, false));
            }
            "pufferfish_bucket" | "tropical_fish_bucket" => {
                max_stack = 1;
                components.insert(
                    "minecraft:bucket_entity_data".to_string(),
                    Value::Object(Map::new()),
                );
                components.insert("minecraft:food".to_string(), food(1, 0.2, false));
            }
            "axolotl_bucket" | "tadpole_bucket" | "sulfur_cube_bucket" => {
                max_stack = 1;
                components.insert(
                    "minecraft:bucket_entity_data".to_string(),
                    Value::Object(Map::new()),
                );
            }
            _ => changed = false,
        }
        if !changed {
            continue;
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
        "{} items need tool/armour/equipment/food component codecs not modelled",
        ids.len().saturating_sub(out.len())
    )];
    Ok((out, coverage))
}

fn defaults(simple: &str) -> Map<String, Value> {
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
    let mut name = Map::new();
    name.insert(
        "translate".to_string(),
        Value::String(format!("item.minecraft.{simple}")),
    );
    m.insert("minecraft:item_name".to_string(), Value::Object(name));
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

fn resistant(tag: &str) -> Value {
    let mut m = Map::new();
    m.insert("types".to_string(), Value::String(format!("#{tag}")));
    Value::Object(m)
}

fn food(nutrition: i64, saturation: f64, can_always_eat: bool) -> Value {
    let mut m = Map::new();
    m.insert("nutrition".to_string(), Value::Number(nutrition.into()));
    m.insert("saturation".to_string(), Value::from(saturation));
    if can_always_eat {
        m.insert("can_always_eat".to_string(), Value::Bool(true));
    }
    Value::Object(m)
}

fn drink(has_particles: bool, effect: Option<&str>) -> Value {
    let mut m = Map::new();
    m.insert("animation".to_string(), Value::String("drink".to_string()));
    m.insert(
        "has_consume_particles".to_string(),
        Value::Bool(has_particles),
    );
    if let Some(effect) = effect {
        let mut entry = Map::new();
        entry.insert("type".to_string(), Value::String(effect.to_string()));
        if effect == "minecraft:play_sound" {
            entry.insert(
                "sound".to_string(),
                Value::String("minecraft:item.ominous_bottle.dispose".to_string()),
            );
        }
        m.insert(
            "on_consume_effects".to_string(),
            Value::Array(vec![Value::Object(entry)]),
        );
    }
    m.insert(
        "sound".to_string(),
        Value::String("minecraft:entity.generic.drink".to_string()),
    );
    Value::Object(m)
}

fn item_stack(id: &str) -> Value {
    let mut m = Map::new();
    m.insert("id".to_string(), Value::String(id.to_string()));
    Value::Object(m)
}

fn repairable(items: &str) -> Value {
    let mut m = Map::new();
    m.insert("items".to_string(), Value::String(items.to_string()));
    Value::Object(m)
}

fn item_tag(tag: &str) -> Value {
    repairable(tag)
}

fn equippable(slot: &str, swappable: bool) -> Value {
    let mut m = Map::new();
    m.insert("slot".to_string(), Value::String(slot.to_string()));
    m.insert("swappable".to_string(), Value::Bool(swappable));
    Value::Object(m)
}

fn elytra_equippable() -> Value {
    let mut m = Map::new();
    m.insert(
        "asset_id".to_string(),
        Value::String("minecraft:elytra".to_string()),
    );
    m.insert("damage_on_hurt".to_string(), Value::Bool(false));
    m.insert(
        "equip_sound".to_string(),
        Value::String("minecraft:item.armor.equip_elytra".to_string()),
    );
    m.insert("slot".to_string(), Value::String("chest".to_string()));
    Value::Object(m)
}

fn blocks_attacks() -> Value {
    let mut m = Map::new();
    m.insert("block_delay_seconds".to_string(), Value::from(0.25));
    m.insert(
        "block_sound".to_string(),
        Value::String("minecraft:item.shield.block".to_string()),
    );
    m.insert(
        "bypassed_by".to_string(),
        Value::String("#minecraft:bypasses_shield".to_string()),
    );
    m.insert(
        "disabled_sound".to_string(),
        Value::String("minecraft:item.shield.break".to_string()),
    );
    let mut damage = Map::new();
    damage.insert("base".to_string(), Value::from(1.0));
    damage.insert("factor".to_string(), Value::from(1.0));
    damage.insert("threshold".to_string(), Value::from(3.0));
    m.insert("item_damage".to_string(), Value::Object(damage));
    Value::Object(m)
}
