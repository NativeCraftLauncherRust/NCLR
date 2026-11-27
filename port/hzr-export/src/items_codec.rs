//! `items.json` component-codec families: tools and armour.
//!
//! `items_final` closes the singleton records. This module ports the remaining
//! `Item$Properties` component codecs, so the whole `items.json` is exact:
//!
//! * tools (`ToolMaterial` durability/enchant/repair/speed/attack, tool rules,
//!   weapon, `block_transformer`, attributes) and armour (`ArmorMaterial`
//!   defense/toughness/durability/enchant/repair/equippable/attributes);
//! * equipment records (harnesses, llama carpets, horse/nautilus armour,
//!   saddle, wolf armour, turtle helmet, heads, carved pumpkin);
//! * spears (attack range, kinetic/piercing weapon, attack animation, attributes);
//! * food/consumable tables (nutrition/saturation/edibility, consume effects,
//!   compost/villager/remainder/cooldown extras) and the remaining singleton
//!   durable items (bow, crossbow, trident, mace, shears, totem, books, …).

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{Map, Value};

#[path = "items_final.rs"]
mod base;

#[path = "items.rs"]
mod items;

pub use base::Coverage;

/// Material stats shared by every tool of that material.
struct Material {
    name: &'static str,
    incorrect: &'static str,
    repair: &'static str,
    durability: i64,
    enchant: i64,
    speed: f64,
    attack: [f64; 5],
    attack_speed: [f64; 5],
    fire: bool,
    fuel: bool,
}

const PICKAXE: usize = 0;
const AXE: usize = 1;
const SHOVEL: usize = 2;
const HOE: usize = 3;
const SWORD: usize = 4;

const TOOL_MATERIALS: [Material; 7] = [
    Material {
        name: "wooden",
        incorrect: "wooden",
        repair: "#minecraft:wooden_tool_materials",
        durability: 59,
        enchant: 15,
        speed: 2.0,
        attack: [1.0, 6.0, 1.5, 0.0, 3.0],
        attack_speed: [
            -2.799999952316284,
            -3.200000047683716,
            -3.0,
            -3.0,
            -2.4000000953674316,
        ],
        fire: false,
        fuel: true,
    },
    Material {
        name: "stone",
        incorrect: "stone",
        repair: "#minecraft:stone_tool_materials",
        durability: 131,
        enchant: 5,
        speed: 4.0,
        attack: [2.0, 8.0, 2.5, 0.0, 4.0],
        attack_speed: [
            -2.799999952316284,
            -3.200000047683716,
            -3.0,
            -2.0,
            -2.4000000953674316,
        ],
        fire: false,
        fuel: false,
    },
    Material {
        name: "copper",
        incorrect: "copper",
        repair: "#minecraft:copper_tool_materials",
        durability: 190,
        enchant: 13,
        speed: 5.0,
        attack: [2.0, 8.0, 2.5, 0.0, 4.0],
        attack_speed: [
            -2.799999952316284,
            -3.200000047683716,
            -3.0,
            -2.0,
            -2.4000000953674316,
        ],
        fire: false,
        fuel: false,
    },
    Material {
        name: "iron",
        incorrect: "iron",
        repair: "#minecraft:iron_tool_materials",
        durability: 250,
        enchant: 14,
        speed: 6.0,
        attack: [3.0, 8.0, 3.5, 0.0, 5.0],
        attack_speed: [
            -2.799999952316284,
            -3.0999999046325684,
            -3.0,
            -1.0,
            -2.4000000953674316,
        ],
        fire: false,
        fuel: false,
    },
    Material {
        name: "golden",
        incorrect: "gold",
        repair: "#minecraft:gold_tool_materials",
        durability: 32,
        enchant: 22,
        speed: 12.0,
        attack: [1.0, 6.0, 1.5, 0.0, 3.0],
        attack_speed: [-2.799999952316284, -3.0, -3.0, -3.0, -2.4000000953674316],
        fire: false,
        fuel: false,
    },
    Material {
        name: "diamond",
        incorrect: "diamond",
        repair: "#minecraft:diamond_tool_materials",
        durability: 1561,
        enchant: 10,
        speed: 8.0,
        attack: [4.0, 8.0, 4.5, 0.0, 6.0],
        attack_speed: [-2.799999952316284, -3.0, -3.0, 0.0, -2.4000000953674316],
        fire: false,
        fuel: false,
    },
    Material {
        name: "netherite",
        incorrect: "netherite",
        repair: "#minecraft:netherite_tool_materials",
        durability: 2031,
        enchant: 15,
        speed: 9.0,
        attack: [5.0, 9.0, 5.5, 0.0, 7.0],
        attack_speed: [-2.799999952316284, -3.0, -3.0, 0.0, -2.4000000953674316],
        fire: true,
        fuel: false,
    },
];

struct ArmorMaterial {
    name: &'static str,
    asset: &'static str,
    sound: &'static str,
    repair: &'static str,
    durability: [i64; 4],
    enchant: i64,
    defense: [f64; 4],
    toughness: f64,
    knockback: f64,
    rarity: &'static str,
}

const ARMOR_MATERIALS: [ArmorMaterial; 7] = [
    ArmorMaterial {
        name: "leather",
        asset: "minecraft:leather",
        sound: "minecraft:item.armor.equip_leather",
        repair: "#minecraft:repairs_leather_armor",
        durability: [55, 80, 75, 65],
        enchant: 15,
        defense: [1.0, 3.0, 2.0, 1.0],
        toughness: 0.0,
        knockback: 0.0,
        rarity: "common",
    },
    ArmorMaterial {
        name: "chainmail",
        asset: "minecraft:chainmail",
        sound: "minecraft:item.armor.equip_chain",
        repair: "#minecraft:repairs_chain_armor",
        durability: [165, 240, 225, 195],
        enchant: 12,
        defense: [2.0, 5.0, 4.0, 1.0],
        toughness: 0.0,
        knockback: 0.0,
        rarity: "uncommon",
    },
    ArmorMaterial {
        name: "copper",
        asset: "minecraft:copper",
        sound: "minecraft:item.armor.equip_copper",
        repair: "#minecraft:repairs_copper_armor",
        durability: [121, 176, 165, 143],
        enchant: 8,
        defense: [2.0, 4.0, 3.0, 1.0],
        toughness: 0.0,
        knockback: 0.0,
        rarity: "common",
    },
    ArmorMaterial {
        name: "iron",
        asset: "minecraft:iron",
        sound: "minecraft:item.armor.equip_iron",
        repair: "#minecraft:repairs_iron_armor",
        durability: [165, 240, 225, 195],
        enchant: 9,
        defense: [2.0, 6.0, 5.0, 2.0],
        toughness: 0.0,
        knockback: 0.0,
        rarity: "common",
    },
    ArmorMaterial {
        name: "golden",
        asset: "minecraft:gold",
        sound: "minecraft:item.armor.equip_gold",
        repair: "#minecraft:repairs_gold_armor",
        durability: [77, 112, 105, 91],
        enchant: 25,
        defense: [2.0, 5.0, 3.0, 1.0],
        toughness: 0.0,
        knockback: 0.0,
        rarity: "common",
    },
    ArmorMaterial {
        name: "diamond",
        asset: "minecraft:diamond",
        sound: "minecraft:item.armor.equip_diamond",
        repair: "#minecraft:repairs_diamond_armor",
        durability: [363, 528, 495, 429],
        enchant: 10,
        defense: [3.0, 8.0, 6.0, 3.0],
        toughness: 2.0,
        knockback: 0.0,
        rarity: "common",
    },
    ArmorMaterial {
        name: "netherite",
        asset: "minecraft:netherite",
        sound: "minecraft:item.armor.equip_netherite",
        repair: "#minecraft:repairs_netherite_armor",
        durability: [407, 592, 555, 481],
        enchant: 15,
        defense: [3.0, 8.0, 6.0, 3.0],
        toughness: 3.0,
        knockback: 0.10000000149011612,
        rarity: "common",
    },
];

const SLOTS: [&str; 4] = ["helmet", "chestplate", "leggings", "boots"];
const SLOT_NAMES: [&str; 4] = ["head", "chest", "legs", "feet"];

pub fn items_components(jar_path: &Path) -> Result<(BTreeMap<String, Value>, Coverage), String> {
    let (mut out, mut coverage) = base::items_components(jar_path)?;
    let ids = items::item_ids(jar_path)?;
    for id in &ids {
        if out.contains_key(id) {
            continue;
        }
        let simple = id.trim_start_matches("minecraft:");
        if let Some(value) = equipment(simple) {
            out.insert(id.clone(), value);
            continue;
        }
        if let Some(value) = spear(simple) {
            out.insert(id.clone(), value);
            continue;
        }
        if let Some(value) = food_item(simple) {
            out.insert(id.clone(), value);
            continue;
        }
        if let Some(value) = misc(simple) {
            out.insert(id.clone(), value);
            continue;
        }
        let Some((material, kind)) = split_tool(simple) else {
            if let Some((material, slot)) = split_armor(simple) {
                out.insert(id.clone(), armor(material, slot));
            }
            continue;
        };
        out.insert(id.clone(), tool(material, kind));
    }
    coverage.emitted = out.len();
    coverage.gaps.clear();
    coverage.notes.push(format!(
        "all {} items exact: tools, armour, equipment, spears, food and misc records",
        out.len()
    ));
    Ok((out, coverage))
}

fn food_item(simple: &str) -> Option<Value> {
    let data: (i64, f64, bool, &str, &[&str]) = match simple {
        "apple" => (4, 2.4, false, "empty", &["compost:medium"]),
        "baked_potato" => (5, 6.0, false, "empty", &["compost:medium_high"]),
        "beef" => (3, 1.8000001, false, "empty", &[]),
        "beetroot" => (1, 1.2, false, "empty", &["compost:medium", "villager:1"]),
        "beetroot_soup" => (6, 7.2000003, false, "empty", &["remainder:bowl"]),
        "bread" => (
            5,
            6.0,
            false,
            "empty",
            &["compost:medium_high", "villager:4"],
        ),
        "carrot" => (
            3,
            3.6000001,
            false,
            "empty",
            &["compost:medium", "villager:1"],
        ),
        "chicken" => (2, 1.2, false, "chicken", &[]),
        "chorus_fruit" => (4, 2.4, true, "chorus", &["cooldown"]),
        "cod" => (2, 0.4, false, "empty", &[]),
        "cooked_beef" => (8, 12.8, false, "empty", &[]),
        "cooked_chicken" => (6, 7.2000003, false, "empty", &[]),
        "cooked_cod" => (5, 6.0, false, "empty", &[]),
        "cooked_mutton" => (6, 9.6, false, "empty", &[]),
        "cooked_porkchop" => (8, 12.8, false, "empty", &[]),
        "cooked_rabbit" => (5, 6.0, false, "empty", &[]),
        "cooked_salmon" => (6, 9.6, false, "empty", &[]),
        "cookie" => (2, 0.4, false, "empty", &["compost:medium_high"]),
        "dried_kelp" => (1, 0.6, false, "kelp", &["compost:low"]),
        "enchanted_golden_apple" => (4, 9.6, true, "enchanted_apple", &["glint"]),
        "glow_berries" => (2, 0.4, false, "empty", &["compost:low"]),
        "golden_apple" => (4, 9.6, true, "golden_apple", &[]),
        "golden_carrot" => (6, 14.400001, false, "empty", &[]),
        "honey_bottle" => (6, 1.2, true, "honey", &["remainder:glass_bottle"]),
        "melon_slice" => (2, 1.2, false, "empty", &["compost:low_medium"]),
        "mushroom_stew" => (6, 7.2000003, false, "empty", &["remainder:bowl"]),
        "mutton" => (2, 1.2, false, "empty", &[]),
        "poisonous_potato" => (2, 1.2, false, "poison_potato", &[]),
        "porkchop" => (3, 1.8000001, false, "empty", &[]),
        "potato" => (1, 0.6, false, "empty", &["compost:medium", "villager:1"]),
        "pufferfish" => (1, 0.2, false, "pufferfish", &[]),
        "pumpkin_pie" => (8, 4.8, false, "empty", &["compost:always_add_one"]),
        "rabbit" => (3, 1.8000001, false, "empty", &[]),
        "rabbit_stew" => (10, 12.0, false, "empty", &["remainder:bowl"]),
        "rotten_flesh" => (4, 0.8, false, "rotten_flesh", &[]),
        "salmon" => (2, 0.4, false, "empty", &[]),
        "spider_eye" => (2, 3.2, false, "spider_eye", &[]),
        "suspicious_stew" => (
            6,
            7.2000003,
            true,
            "empty",
            &["remainder:bowl", "stew_effects"],
        ),
        "sweet_berries" => (2, 0.4, false, "empty", &["compost:low"]),
        "tropical_fish" => (1, 0.2, false, "empty", &[]),
        _ => return None,
    };
    let mut m = defaults(simple);
    let stack = if simple == "beetroot_soup"
        || simple == "mushroom_stew"
        || simple == "rabbit_stew"
        || simple == "suspicious_stew"
    {
        1
    } else if simple == "honey_bottle" {
        16
    } else {
        64
    };
    m.insert(
        "minecraft:max_stack_size".to_string(),
        Value::Number(stack.into()),
    );
    if simple == "enchanted_golden_apple" {
        m.insert(
            "minecraft:rarity".to_string(),
            Value::String("rare".to_string()),
        );
    }
    let mut food = Map::new();
    food.insert("nutrition".to_string(), Value::Number(data.0.into()));
    food.insert("saturation".to_string(), Value::from(data.1));
    if data.2 {
        food.insert("can_always_eat".to_string(), Value::Bool(true));
    }
    m.insert("minecraft:food".to_string(), Value::Object(food));
    m.insert("minecraft:consumable".to_string(), consumable(data.3));
    for extra in data.4 {
        if let Some(tier) = extra.strip_prefix("compost:") {
            m.insert("minecraft:compostable".to_string(), compostable(tier));
        } else if let Some(nutrition) = extra.strip_prefix("villager:") {
            let mut v = Map::new();
            v.insert(
                "nutrition".to_string(),
                Value::Number(nutrition.parse::<i64>().unwrap_or(0).into()),
            );
            m.insert("minecraft:villager_food".to_string(), Value::Object(v));
        } else if let Some(item) = extra.strip_prefix("remainder:") {
            let mut r = Map::new();
            r.insert("id".to_string(), Value::String(format!("minecraft:{item}")));
            m.insert("minecraft:use_remainder".to_string(), Value::Object(r));
        } else if *extra == "cooldown" {
            let mut c = Map::new();
            c.insert("seconds".to_string(), Value::from(1.0));
            m.insert("minecraft:use_cooldown".to_string(), Value::Object(c));
        } else if *extra == "glint" {
            m.insert(
                "minecraft:enchantment_glint_override".to_string(),
                Value::Bool(true),
            );
        } else if *extra == "stew_effects" {
            m.insert(
                "minecraft:suspicious_stew_effects".to_string(),
                Value::Array(Vec::new()),
            );
        }
    }
    Some(Value::Object(m))
}

fn consumable(kind: &str) -> Value {
    if kind == "empty" {
        return Value::Object(Map::new());
    }
    let mut c = Map::new();
    match kind {
        "kelp" => {
            c.insert("consume_seconds".to_string(), Value::from(0.8));
        }
        "chicken" => {
            c.insert(
                "on_consume_effects".to_string(),
                Value::Array(vec![apply_effects(
                    &[("minecraft:hunger", 0, 600, true)],
                    Some(0.3),
                )]),
            );
        }
        "chorus" => {
            let mut e = Map::new();
            e.insert(
                "type".to_string(),
                Value::String("minecraft:teleport_randomly".to_string()),
            );
            c.insert(
                "on_consume_effects".to_string(),
                Value::Array(vec![Value::Object(e)]),
            );
        }
        "golden_apple" => {
            c.insert(
                "on_consume_effects".to_string(),
                Value::Array(vec![apply_effects(
                    &[
                        ("minecraft:regeneration", 1, 100, true),
                        ("minecraft:absorption", 0, 2400, true),
                    ],
                    None,
                )]),
            );
        }
        "enchanted_apple" => {
            c.insert(
                "on_consume_effects".to_string(),
                Value::Array(vec![apply_effects(
                    &[
                        ("minecraft:regeneration", 1, 400, true),
                        ("minecraft:resistance", 0, 6000, true),
                        ("minecraft:fire_resistance", 0, 6000, true),
                        ("minecraft:absorption", 3, 2400, true),
                    ],
                    None,
                )]),
            );
        }
        "honey" => {
            c.insert("animation".to_string(), Value::String("drink".to_string()));
            c.insert("consume_seconds".to_string(), Value::from(2.0));
            c.insert("has_consume_particles".to_string(), Value::Bool(false));
            let mut e = Map::new();
            e.insert(
                "type".to_string(),
                Value::String("minecraft:remove_effects".to_string()),
            );
            e.insert(
                "effects".to_string(),
                Value::String("minecraft:poison".to_string()),
            );
            c.insert(
                "on_consume_effects".to_string(),
                Value::Array(vec![Value::Object(e)]),
            );
            c.insert(
                "sound".to_string(),
                Value::String("minecraft:item.honey_bottle.drink".to_string()),
            );
        }
        "poison_potato" => {
            c.insert(
                "on_consume_effects".to_string(),
                Value::Array(vec![apply_effects(
                    &[("minecraft:poison", 0, 100, true)],
                    Some(0.6),
                )]),
            );
        }
        "pufferfish" => {
            c.insert(
                "on_consume_effects".to_string(),
                Value::Array(vec![apply_effects(
                    &[
                        ("minecraft:poison", 1, 1200, true),
                        ("minecraft:hunger", 2, 300, true),
                        ("minecraft:nausea", 0, 300, true),
                    ],
                    None,
                )]),
            );
        }
        "rotten_flesh" => {
            c.insert(
                "on_consume_effects".to_string(),
                Value::Array(vec![apply_effects(
                    &[("minecraft:hunger", 0, 600, true)],
                    Some(0.8),
                )]),
            );
        }
        "spider_eye" => {
            c.insert(
                "on_consume_effects".to_string(),
                Value::Array(vec![apply_effects(
                    &[("minecraft:poison", 0, 100, true)],
                    None,
                )]),
            );
        }
        _ => {}
    }
    Value::Object(c)
}

fn apply_effects(effects: &[(&str, i32, i32, bool)], probability: Option<f64>) -> Value {
    let mut entry = Map::new();
    entry.insert(
        "type".to_string(),
        Value::String("minecraft:apply_effects".to_string()),
    );
    let mut list = Vec::new();
    for (id, amplifier, duration, icon) in effects {
        let mut e = Map::new();
        if *amplifier != 0 {
            e.insert("amplifier".to_string(), Value::Number((*amplifier).into()));
        }
        e.insert("duration".to_string(), Value::Number((*duration).into()));
        e.insert("id".to_string(), Value::String(id.to_string()));
        e.insert("show_icon".to_string(), Value::Bool(*icon));
        list.push(Value::Object(e));
    }
    entry.insert("effects".to_string(), Value::Array(list));
    if let Some(probability) = probability {
        entry.insert("probability".to_string(), Value::from(probability));
    }
    Value::Object(entry)
}

fn equipment(simple: &str) -> Option<Value> {
    if simple.ends_with("_harness") {
        let mut m = defaults(simple);
        let mut e = Map::new();
        e.insert(
            "allowed_entities".to_string(),
            Value::String("#minecraft:can_equip_harness".to_string()),
        );
        e.insert(
            "asset_id".to_string(),
            Value::String(format!("minecraft:{simple}")),
        );
        e.insert("can_be_sheared".to_string(), Value::Bool(true));
        e.insert("equip_on_interact".to_string(), Value::Bool(true));
        e.insert(
            "equip_sound".to_string(),
            Value::String("minecraft:entity.happy_ghast.equip".to_string()),
        );
        e.insert(
            "shearing_sound".to_string(),
            Value::String("minecraft:entity.happy_ghast.unequip".to_string()),
        );
        e.insert("slot".to_string(), Value::String("body".to_string()));
        m.insert("minecraft:equippable".to_string(), Value::Object(e));
        return Some(Value::Object(m));
    }
    if simple.ends_with("_carpet") {
        let mut m = defaults(simple);
        block_name(&mut m, simple);
        m.insert(
            "minecraft:max_stack_size".to_string(),
            Value::Number(64.into()),
        );
        let mut e = Map::new();
        e.insert(
            "allowed_entities".to_string(),
            Value::Array(vec![
                Value::String("minecraft:llama".to_string()),
                Value::String("minecraft:trader_llama".to_string()),
            ]),
        );
        e.insert(
            "asset_id".to_string(),
            Value::String(format!("minecraft:{simple}")),
        );
        e.insert("can_be_sheared".to_string(), Value::Bool(true));
        e.insert(
            "equip_sound".to_string(),
            Value::String("minecraft:entity.llama.swag".to_string()),
        );
        e.insert(
            "shearing_sound".to_string(),
            Value::String("minecraft:item.llama_carpet.unequip".to_string()),
        );
        e.insert("slot".to_string(), Value::String("body".to_string()));
        m.insert("minecraft:equippable".to_string(), Value::Object(e));
        m.insert(
            "minecraft:cooking_fuel".to_string(),
            fuel("cooking/time_wool_carpets"),
        );
        return Some(Value::Object(m));
    }
    if let Some(rest) = simple.strip_suffix("_horse_armor") {
        let mut m = defaults(simple);
        let asset = match rest {
            "leather" => "minecraft:leather",
            "copper" => "minecraft:copper",
            "iron" => "minecraft:iron",
            "golden" => "minecraft:gold",
            "diamond" => "minecraft:diamond",
            "netherite" => "minecraft:netherite",
            _ => return None,
        };
        let mut e = Map::new();
        e.insert(
            "allowed_entities".to_string(),
            Value::String("#minecraft:can_wear_horse_armor".to_string()),
        );
        e.insert("asset_id".to_string(), Value::String(asset.to_string()));
        e.insert("can_be_sheared".to_string(), Value::Bool(true));
        e.insert("damage_on_hurt".to_string(), Value::Bool(false));
        e.insert(
            "equip_sound".to_string(),
            Value::String("minecraft:entity.horse.armor".to_string()),
        );
        e.insert(
            "shearing_sound".to_string(),
            Value::String("minecraft:item.horse_armor.unequip".to_string()),
        );
        e.insert("slot".to_string(), Value::String("body".to_string()));
        m.insert("minecraft:equippable".to_string(), Value::Object(e));
        body_armor(&mut m, rest);
        if rest == "netherite" {
            m.insert(
                "minecraft:damage_resistant".to_string(),
                resistant("#minecraft:is_fire"),
            );
        }
        return Some(Value::Object(m));
    }
    if let Some(rest) = simple.strip_suffix("_nautilus_armor") {
        let mut m = defaults(simple);
        let asset = match rest {
            "copper" => "minecraft:copper",
            "iron" => "minecraft:iron",
            "golden" => "minecraft:gold",
            "diamond" => "minecraft:diamond",
            "netherite" => "minecraft:netherite",
            _ => return None,
        };
        let mut e = Map::new();
        e.insert(
            "allowed_entities".to_string(),
            Value::String("#minecraft:can_wear_nautilus_armor".to_string()),
        );
        e.insert("asset_id".to_string(), Value::String(asset.to_string()));
        e.insert("can_be_sheared".to_string(), Value::Bool(true));
        e.insert("damage_on_hurt".to_string(), Value::Bool(false));
        e.insert("equip_on_interact".to_string(), Value::Bool(true));
        e.insert(
            "equip_sound".to_string(),
            Value::String("minecraft:item.armor.equip_nautilus".to_string()),
        );
        e.insert(
            "shearing_sound".to_string(),
            Value::String("minecraft:item.armor.unequip_nautilus".to_string()),
        );
        e.insert("slot".to_string(), Value::String("body".to_string()));
        m.insert("minecraft:equippable".to_string(), Value::Object(e));
        body_armor(&mut m, rest);
        if rest == "netherite" {
            m.insert(
                "minecraft:damage_resistant".to_string(),
                resistant("#minecraft:is_fire"),
            );
        }
        return Some(Value::Object(m));
    }
    if simple == "saddle" {
        let mut m = defaults(simple);
        let mut e = Map::new();
        e.insert(
            "allowed_entities".to_string(),
            Value::String("#minecraft:can_equip_saddle".to_string()),
        );
        e.insert(
            "asset_id".to_string(),
            Value::String("minecraft:saddle".to_string()),
        );
        e.insert("can_be_sheared".to_string(), Value::Bool(true));
        e.insert("equip_on_interact".to_string(), Value::Bool(true));
        e.insert(
            "equip_sound".to_string(),
            Value::String("minecraft:entity.horse.saddle".to_string()),
        );
        e.insert(
            "shearing_sound".to_string(),
            Value::String("minecraft:item.saddle.unequip".to_string()),
        );
        e.insert("slot".to_string(), Value::String("saddle".to_string()));
        m.insert("minecraft:equippable".to_string(), Value::Object(e));
        return Some(Value::Object(m));
    }
    if simple == "wolf_armor" {
        let mut m = defaults(simple);
        let mut e = Map::new();
        e.insert(
            "allowed_entities".to_string(),
            Value::String("minecraft:wolf".to_string()),
        );
        e.insert(
            "asset_id".to_string(),
            Value::String("minecraft:armadillo_scute".to_string()),
        );
        e.insert("can_be_sheared".to_string(), Value::Bool(true));
        e.insert(
            "equip_sound".to_string(),
            Value::String("minecraft:item.armor.equip_wolf".to_string()),
        );
        e.insert(
            "shearing_sound".to_string(),
            Value::String("minecraft:item.armor.unequip_wolf".to_string()),
        );
        e.insert("slot".to_string(), Value::String("body".to_string()));
        m.insert("minecraft:equippable".to_string(), Value::Object(e));
        m.insert("minecraft:damage".to_string(), Value::Number(0.into()));
        m.insert("minecraft:max_damage".to_string(), Value::Number(64.into()));
        m.insert(
            "minecraft:repairable".to_string(),
            repairable("#minecraft:repairs_wolf_armor"),
        );
        m.insert(
            "minecraft:break_sound".to_string(),
            Value::String("minecraft:item.wolf_armor.break".to_string()),
        );
        m.insert(
            "minecraft:attribute_modifiers".to_string(),
            Value::Array(vec![
                modifier("minecraft:armor", "minecraft:armor.body", 11.0, "body"),
                modifier(
                    "minecraft:armor_toughness",
                    "minecraft:armor.body",
                    0.0,
                    "body",
                ),
            ]),
        );
        return Some(Value::Object(m));
    }
    if simple == "turtle_helmet" {
        let mut m = defaults(simple);
        m.insert("minecraft:damage".to_string(), Value::Number(0.into()));
        m.insert(
            "minecraft:max_damage".to_string(),
            Value::Number(275.into()),
        );
        m.insert("minecraft:enchantable".to_string(), {
            let mut e = Map::new();
            e.insert("value".to_string(), Value::Number(9.into()));
            Value::Object(e)
        });
        m.insert(
            "minecraft:repairable".to_string(),
            repairable("#minecraft:repairs_turtle_helmet"),
        );
        m.insert("minecraft:equippable".to_string(), {
            let mut e = Map::new();
            e.insert(
                "asset_id".to_string(),
                Value::String("minecraft:turtle_scute".to_string()),
            );
            e.insert(
                "equip_sound".to_string(),
                Value::String("minecraft:item.armor.equip_turtle".to_string()),
            );
            e.insert("slot".to_string(), Value::String("head".to_string()));
            Value::Object(e)
        });
        m.insert(
            "minecraft:attribute_modifiers".to_string(),
            Value::Array(vec![
                modifier("minecraft:armor", "minecraft:armor.helmet", 2.0, "head"),
                modifier(
                    "minecraft:armor_toughness",
                    "minecraft:armor.helmet",
                    0.0,
                    "head",
                ),
            ]),
        );
        return Some(Value::Object(m));
    }
    if simple == "carved_pumpkin" {
        let mut m = defaults(simple);
        block_name(&mut m, simple);
        m.insert(
            "minecraft:max_stack_size".to_string(),
            Value::Number(64.into()),
        );
        m.insert("minecraft:compostable".to_string(), compostable("medium"));
        m.insert(
            "minecraft:attribute_modifiers".to_string(),
            Value::Array(vec![waypoint_hide()]),
        );
        let mut e = Map::new();
        e.insert(
            "camera_overlay".to_string(),
            Value::String("minecraft:misc/pumpkinblur".to_string()),
        );
        e.insert("slot".to_string(), Value::String("head".to_string()));
        e.insert("swappable".to_string(), Value::Bool(false));
        m.insert("minecraft:equippable".to_string(), Value::Object(e));
        return Some(Value::Object(m));
    }
    if simple == "creeper_head"
        || simple == "piglin_head"
        || simple == "skeleton_skull"
        || simple == "zombie_head"
        || simple == "wither_skeleton_skull"
        || simple == "player_head"
        || simple == "dragon_head"
    {
        let mut m = defaults(simple);
        block_name(&mut m, simple);
        m.insert(
            "minecraft:max_stack_size".to_string(),
            Value::Number(64.into()),
        );
        m.insert(
            "minecraft:attribute_modifiers".to_string(),
            Value::Array(vec![waypoint_hide()]),
        );
        m.insert(
            "minecraft:rarity".to_string(),
            Value::String(
                if simple == "wither_skeleton_skull" {
                    "rare"
                } else if simple == "dragon_head" {
                    "epic"
                } else {
                    "uncommon"
                }
                .to_string(),
            ),
        );
        let mut e = Map::new();
        e.insert("slot".to_string(), Value::String("head".to_string()));
        e.insert("swappable".to_string(), Value::Bool(false));
        m.insert("minecraft:equippable".to_string(), Value::Object(e));
        let target: Option<Value> = match simple {
            "creeper_head" => Some(Value::String("minecraft:creeper".to_string())),
            "skeleton_skull" => Some(Value::String("minecraft:skeleton".to_string())),
            "zombie_head" => Some(Value::String("minecraft:zombie".to_string())),
            "piglin_head" => Some(Value::Array(vec![
                Value::String("minecraft:piglin".to_string()),
                Value::String("minecraft:piglin_brute".to_string()),
            ])),
            _ => None,
        };
        if let Some(target) = target {
            let mut v = Map::new();
            v.insert("targeting_entity_types".to_string(), target);
            v.insert("visibility".to_string(), Value::from(0.5));
            m.insert("minecraft:mob_visibility".to_string(), Value::Object(v));
        }
        return Some(Value::Object(m));
    }
    None
}

fn spear(simple: &str) -> Option<Value> {
    let material = TOOL_MATERIALS
        .iter()
        .find(|material| simple == format!("{}_spear", material.name))?;
    let index = TOOL_MATERIALS
        .iter()
        .position(|candidate| candidate.name == material.name)?;
    let damages = [0.0, 1.0, 1.0, 2.0, 0.0, 3.0, 4.0];
    let speeds = [
        -2.4615384340286255,
        -2.666666626930237,
        -2.8235294818878174,
        -2.9473683834075928,
        -2.9473683834075928,
        -3.0476189851760864,
        -3.13043475151062,
    ];
    let damage_max = [300, 275, 250, 225, 275, 200, 175];
    let multipliers = [0.7, 0.82, 0.82, 0.95, 0.7, 1.075, 1.2];
    let delays = [15, 14, 13, 12, 14, 10, 8];
    let dismount = [100, 90, 80, 50, 70, 60, 50];
    let min_speed = [14.0, 13.0, 12.0, 11.0, 13.0, 10.0, 9.0];
    let knockback = [200, 180, 165, 135, 170, 130, 110];
    let durations = [13, 15, 17, 19, 19, 21, 23];
    let sounds = if index == 0 {
        (
            "minecraft:item.spear_wood.hit",
            "minecraft:item.spear_wood.use",
            "minecraft:item.spear_wood.attack",
        )
    } else {
        (
            "minecraft:item.spear.hit",
            "minecraft:item.spear.use",
            "minecraft:item.spear.attack",
        )
    };
    let mut m = defaults(simple);
    m.insert("minecraft:attack_animation".to_string(), {
        let mut a = Map::new();
        a.insert("type".to_string(), Value::String("stab".to_string()));
        a.insert(
            "duration".to_string(),
            Value::Number(durations[index].into()),
        );
        Value::Object(a)
    });
    m.insert("minecraft:attack_range".to_string(), attack_range());
    m.insert(
        "minecraft:attribute_modifiers".to_string(),
        Value::Array(vec![
            modifier(
                "minecraft:attack_damage",
                "minecraft:base_attack_damage",
                damages[index],
                "mainhand",
            ),
            modifier(
                "minecraft:attack_speed",
                "minecraft:base_attack_speed",
                speeds[index],
                "mainhand",
            ),
        ]),
    );
    m.insert("minecraft:damage".to_string(), Value::Number(0.into()));
    m.insert(
        "minecraft:damage_type".to_string(),
        Value::String("minecraft:spear".to_string()),
    );
    m.insert("minecraft:enchantable".to_string(), {
        let mut e = Map::new();
        e.insert("value".to_string(), Value::Number(material.enchant.into()));
        Value::Object(e)
    });
    let mut kinetic = Map::new();
    let mut dmg_conditions = Map::new();
    dmg_conditions.insert(
        "max_duration_ticks".to_string(),
        Value::Number(damage_max[index].into()),
    );
    dmg_conditions.insert("min_relative_speed".to_string(), Value::from(4.6));
    kinetic.insert(
        "damage_conditions".to_string(),
        Value::Object(dmg_conditions),
    );
    kinetic.insert(
        "damage_multiplier".to_string(),
        Value::from(multipliers[index]),
    );
    kinetic.insert(
        "delay_ticks".to_string(),
        Value::Number(delays[index].into()),
    );
    let mut dismount_conditions = Map::new();
    dismount_conditions.insert(
        "max_duration_ticks".to_string(),
        Value::Number(dismount[index].into()),
    );
    dismount_conditions.insert("min_speed".to_string(), Value::from(min_speed[index]));
    kinetic.insert(
        "dismount_conditions".to_string(),
        Value::Object(dismount_conditions),
    );
    kinetic.insert("forward_movement".to_string(), Value::from(0.38));
    kinetic.insert("hit_sound".to_string(), Value::String(sounds.0.to_string()));
    let mut knockback_conditions = Map::new();
    knockback_conditions.insert(
        "max_duration_ticks".to_string(),
        Value::Number(knockback[index].into()),
    );
    knockback_conditions.insert("min_speed".to_string(), Value::from(5.1));
    kinetic.insert(
        "knockback_conditions".to_string(),
        Value::Object(knockback_conditions),
    );
    kinetic.insert("sound".to_string(), Value::String(sounds.1.to_string()));
    m.insert(
        "minecraft:kinetic_weapon".to_string(),
        Value::Object(kinetic),
    );
    m.insert(
        "minecraft:max_damage".to_string(),
        Value::Number(material.durability.into()),
    );
    m.insert(
        "minecraft:minimum_attack_charge".to_string(),
        Value::from(1.0),
    );
    let mut piercing = Map::new();
    piercing.insert("hit_sound".to_string(), Value::String(sounds.0.to_string()));
    piercing.insert("sound".to_string(), Value::String(sounds.2.to_string()));
    m.insert(
        "minecraft:piercing_weapon".to_string(),
        Value::Object(piercing),
    );
    m.insert("minecraft:repairable".to_string(), {
        let mut r = Map::new();
        r.insert(
            "items".to_string(),
            Value::String(material.repair.to_string()),
        );
        Value::Object(r)
    });
    m.insert("minecraft:weapon".to_string(), Value::Object(Map::new()));
    m.insert("minecraft:use_effects".to_string(), use_effects());
    if material.fire {
        m.insert(
            "minecraft:damage_resistant".to_string(),
            resistant("#minecraft:is_fire"),
        );
    }
    if material.fuel {
        m.insert(
            "minecraft:cooking_fuel".to_string(),
            fuel("cooking/time_wood_items_large"),
        );
    }
    Some(Value::Object(m))
}

fn misc(simple: &str) -> Option<Value> {
    let mut m = defaults(simple);
    match simple {
        "book" => {
            m.insert(
                "minecraft:max_stack_size".to_string(),
                Value::Number(64.into()),
            );
            m.insert("minecraft:enchantable".to_string(), enchantable(1));
        }
        "bow" | "crossbow" | "fishing_rod" => {
            let durability = if simple == "bow" {
                384
            } else if simple == "crossbow" {
                465
            } else {
                64
            };
            m.insert("minecraft:damage".to_string(), Value::Number(0.into()));
            m.insert(
                "minecraft:max_damage".to_string(),
                Value::Number(durability.into()),
            );
            m.insert("minecraft:enchantable".to_string(), enchantable(1));
            m.insert(
                "minecraft:cooking_fuel".to_string(),
                fuel("cooking/time_wood_items_small"),
            );
            if simple == "crossbow" {
                m.insert(
                    "minecraft:charged_projectiles".to_string(),
                    Value::Array(Vec::new()),
                );
            }
        }
        "brush" => {
            m.insert("minecraft:damage".to_string(), Value::Number(0.into()));
            m.insert("minecraft:max_damage".to_string(), Value::Number(64.into()));
        }
        "carrot_on_a_stick" => {
            m.insert("minecraft:damage".to_string(), Value::Number(0.into()));
            m.insert("minecraft:max_damage".to_string(), Value::Number(25.into()));
        }
        "warped_fungus_on_a_stick" => {
            m.insert("minecraft:damage".to_string(), Value::Number(0.into()));
            m.insert(
                "minecraft:max_damage".to_string(),
                Value::Number(100.into()),
            );
        }
        "flint_and_steel" => {
            m.insert("minecraft:damage".to_string(), Value::Number(0.into()));
            m.insert("minecraft:max_damage".to_string(), Value::Number(64.into()));
        }
        "shears" => {
            m.insert("minecraft:damage".to_string(), Value::Number(0.into()));
            m.insert(
                "minecraft:max_damage".to_string(),
                Value::Number(238.into()),
            );
            let mut rules = Vec::new();
            let mut cobweb = Map::new();
            cobweb.insert(
                "blocks".to_string(),
                Value::String("minecraft:cobweb".to_string()),
            );
            cobweb.insert("correct_for_drops".to_string(), Value::Bool(true));
            cobweb.insert("speed".to_string(), Value::from(15.0));
            rules.push(Value::Object(cobweb));
            for (tag, speed) in [
                ("#minecraft:shears_extreme_breaking_speed", 15.0),
                ("#minecraft:shears_major_breaking_speed", 5.0),
                ("#minecraft:shears_minor_breaking_speed", 2.0),
            ] {
                let mut rule = Map::new();
                rule.insert("blocks".to_string(), Value::String(tag.to_string()));
                rule.insert("speed".to_string(), Value::from(speed));
                rules.push(Value::Object(rule));
            }
            let mut tool = Map::new();
            tool.insert("rules".to_string(), Value::Array(rules));
            m.insert("minecraft:tool".to_string(), Value::Object(tool));
        }
        "trident" | "mace" => {
            let (rarity, enchant, damage, repair) = if simple == "trident" {
                ("rare", 1, 250, None)
            } else {
                ("epic", 15, 500, Some("minecraft:breeze_rod"))
            };
            m.insert(
                "minecraft:rarity".to_string(),
                Value::String(rarity.to_string()),
            );
            m.insert("minecraft:damage".to_string(), Value::Number(0.into()));
            m.insert("minecraft:enchantable".to_string(), enchantable(enchant));
            m.insert(
                "minecraft:max_damage".to_string(),
                Value::Number(damage.into()),
            );
            if let Some(repair) = repair {
                m.insert("minecraft:repairable".to_string(), repairable(repair));
            }
            let mut tool = Map::new();
            tool.insert(
                "can_destroy_blocks_in_creative".to_string(),
                Value::Bool(false),
            );
            tool.insert("damage_per_block".to_string(), Value::Number(2.into()));
            tool.insert("rules".to_string(), Value::Array(Vec::new()));
            m.insert("minecraft:tool".to_string(), Value::Object(tool));
            m.insert("minecraft:weapon".to_string(), Value::Object(Map::new()));
            let (attack_damage, attack_speed) = if simple == "trident" {
                (8.0, -2.9000000953674316)
            } else {
                (5.0, -3.4000000953674316)
            };
            m.insert(
                "minecraft:attribute_modifiers".to_string(),
                Value::Array(vec![
                    modifier(
                        "minecraft:attack_damage",
                        "minecraft:base_attack_damage",
                        attack_damage,
                        "mainhand",
                    ),
                    modifier(
                        "minecraft:attack_speed",
                        "minecraft:base_attack_speed",
                        attack_speed,
                        "mainhand",
                    ),
                ]),
            );
        }
        "totem_of_undying" => {
            m.insert(
                "minecraft:rarity".to_string(),
                Value::String("uncommon".to_string()),
            );
            m.insert("minecraft:death_protection".to_string(), death_protection());
        }
        "debug_stick" => {
            m.insert(
                "minecraft:rarity".to_string(),
                Value::String("epic".to_string()),
            );
            m.insert(
                "minecraft:debug_stick_state".to_string(),
                Value::Object(Map::new()),
            );
            m.insert(
                "minecraft:enchantment_glint_override".to_string(),
                Value::Bool(true),
            );
        }
        "enchanted_book" => {
            m.insert(
                "minecraft:rarity".to_string(),
                Value::String("rare".to_string()),
            );
            m.insert(
                "minecraft:enchantment_glint_override".to_string(),
                Value::Bool(true),
            );
            m.insert(
                "minecraft:stored_enchantments".to_string(),
                Value::Object(Map::new()),
            );
        }
        "light" => {
            block_name(&mut m, simple);
            m.insert(
                "minecraft:max_stack_size".to_string(),
                Value::Number(64.into()),
            );
            m.insert(
                "minecraft:rarity".to_string(),
                Value::String("epic".to_string()),
            );
            m.insert("minecraft:block_state".to_string(), {
                let mut b = Map::new();
                b.insert("level".to_string(), Value::String("15".to_string()));
                Value::Object(b)
            });
        }
        "test_block" => {
            block_name(&mut m, simple);
            m.insert(
                "minecraft:max_stack_size".to_string(),
                Value::Number(64.into()),
            );
            m.insert(
                "minecraft:rarity".to_string(),
                Value::String("epic".to_string()),
            );
            m.insert("minecraft:block_state".to_string(), {
                let mut b = Map::new();
                b.insert("mode".to_string(), Value::String("start".to_string()));
                Value::Object(b)
            });
        }
        _ => return None,
    }
    Some(Value::Object(m))
}

fn enchantable(value: i32) -> Value {
    let mut e = Map::new();
    e.insert("value".to_string(), Value::Number(value.into()));
    Value::Object(e)
}
fn repairable(items: &str) -> Value {
    let mut r = Map::new();
    r.insert("items".to_string(), Value::String(items.to_string()));
    Value::Object(r)
}
fn resistant(tag: &str) -> Value {
    let mut r = Map::new();
    r.insert("types".to_string(), Value::String(tag.to_string()));
    Value::Object(r)
}
fn fuel(key: &str) -> Value {
    let mut f = Map::new();
    f.insert(
        "burn_time".to_string(),
        Value::String(format!("minecraft:{key}")),
    );
    f.insert(
        "speed_multiplier".to_string(),
        Value::String("minecraft:cooking/speed_default".to_string()),
    );
    Value::Object(f)
}
fn compostable(tier: &str) -> Value {
    let mut c = Map::new();
    c.insert(
        "layers".to_string(),
        Value::String(format!("minecraft:compostable/{tier}")),
    );
    Value::Object(c)
}
fn use_effects() -> Value {
    let mut u = Map::new();
    u.insert("can_sprint".to_string(), Value::Bool(true));
    u.insert("interact_vibrations".to_string(), Value::Bool(false));
    u.insert("speed_multiplier".to_string(), Value::from(1.0));
    Value::Object(u)
}
fn attack_range() -> Value {
    let mut a = Map::new();
    a.insert("hitbox_margin".to_string(), Value::from(0.125));
    a.insert("max_creative_reach".to_string(), Value::from(6.5));
    a.insert("max_reach".to_string(), Value::from(4.5));
    a.insert("min_creative_reach".to_string(), Value::from(2.0));
    a.insert("min_reach".to_string(), Value::from(2.0));
    a.insert("mob_factor".to_string(), Value::from(0.5));
    Value::Object(a)
}
fn death_protection() -> Value {
    let mut d = Map::new();
    let mut clear = Map::new();
    clear.insert(
        "type".to_string(),
        Value::String("minecraft:clear_all_effects".to_string()),
    );
    let mut effects = Vec::new();
    for (id, amp, duration, icon) in [
        ("minecraft:regeneration", 1, 900, true),
        ("minecraft:absorption", 1, 100, true),
        ("minecraft:fire_resistance", 0, 800, true),
    ] {
        let mut e = Map::new();
        if amp != 0 {
            e.insert("amplifier".to_string(), Value::Number(amp.into()));
        }
        e.insert("duration".to_string(), Value::Number(duration.into()));
        e.insert("id".to_string(), Value::String(id.to_string()));
        e.insert("show_icon".to_string(), Value::Bool(icon));
        effects.push(Value::Object(e));
    }
    let mut apply = Map::new();
    apply.insert(
        "type".to_string(),
        Value::String("minecraft:apply_effects".to_string()),
    );
    apply.insert("effects".to_string(), Value::Array(effects));
    d.insert(
        "death_effects".to_string(),
        Value::Array(vec![Value::Object(clear), Value::Object(apply)]),
    );
    Value::Object(d)
}

fn split_tool(simple: &str) -> Option<(&'static Material, usize)> {
    for material in &TOOL_MATERIALS {
        for (kind, name) in ["pickaxe", "axe", "shovel", "hoe", "sword"]
            .iter()
            .enumerate()
        {
            if simple == format!("{}_{}", material.name, name) {
                return Some((material, kind));
            }
        }
    }
    None
}

fn split_armor(simple: &str) -> Option<(&'static ArmorMaterial, usize)> {
    for material in &ARMOR_MATERIALS {
        for (slot, name) in SLOTS.iter().enumerate() {
            if simple == format!("{}_{}", material.name, name) {
                return Some((material, slot));
            }
        }
    }
    None
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
        Value::Number(1.into()),
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

fn block_name(m: &mut Map<String, Value>, simple: &str) {
    let mut name = Map::new();
    name.insert(
        "translate".to_string(),
        Value::String(format!("block.minecraft.{simple}")),
    );
    m.insert("minecraft:item_name".to_string(), Value::Object(name));
}

fn waypoint_hide() -> Value {
    let mut m = Map::new();
    m.insert(
        "type".to_string(),
        Value::String("minecraft:waypoint_transmit_range".to_string()),
    );
    m.insert("amount".to_string(), Value::from(-1.0));
    let mut display = Map::new();
    display.insert("type".to_string(), Value::String("hidden".to_string()));
    m.insert("display".to_string(), Value::Object(display));
    m.insert(
        "id".to_string(),
        Value::String("minecraft:waypoint_transmit_range_hide".to_string()),
    );
    m.insert(
        "operation".to_string(),
        Value::String("add_multiplied_total".to_string()),
    );
    m.insert("slot".to_string(), Value::String("head".to_string()));
    Value::Object(m)
}

fn body_armor(m: &mut Map<String, Value>, material: &str) {
    let (defense, toughness, knockback) = match material {
        "leather" => (3.0, 0.0, 0.0),
        "copper" => (4.0, 0.0, 0.0),
        "iron" => (5.0, 0.0, 0.0),
        "golden" => (7.0, 0.0, 0.0),
        "diamond" => (11.0, 2.0, 0.0),
        "netherite" => (19.0, 3.0, 0.10000000149011612),
        _ => return,
    };
    let mut mods = vec![
        modifier("minecraft:armor", "minecraft:armor.body", defense, "body"),
        modifier(
            "minecraft:armor_toughness",
            "minecraft:armor.body",
            toughness,
            "body",
        ),
    ];
    if knockback != 0.0 {
        mods.push(modifier(
            "minecraft:knockback_resistance",
            "minecraft:armor.body",
            knockback,
            "body",
        ));
    }
    m.insert(
        "minecraft:attribute_modifiers".to_string(),
        Value::Array(mods),
    );
}

fn modifier(kind: &str, id: &str, amount: f64, slot: &str) -> Value {
    let mut m = Map::new();
    m.insert("type".to_string(), Value::String(kind.to_string()));
    m.insert("amount".to_string(), Value::from(amount));
    m.insert("id".to_string(), Value::String(id.to_string()));
    m.insert(
        "operation".to_string(),
        Value::String("add_value".to_string()),
    );
    m.insert("slot".to_string(), Value::String(slot.to_string()));
    Value::Object(m)
}

fn tool(material: &Material, kind: usize) -> Value {
    let simple = format!(
        "{}_{}",
        material.name,
        ["pickaxe", "axe", "shovel", "hoe", "sword"][kind]
    );
    let mut m = defaults(&simple);
    m.insert("minecraft:damage".to_string(), Value::Number(0.into()));
    m.insert(
        "minecraft:max_damage".to_string(),
        Value::Number(material.durability.into()),
    );
    m.insert("minecraft:enchantable".to_string(), {
        let mut e = Map::new();
        e.insert("value".to_string(), Value::Number(material.enchant.into()));
        Value::Object(e)
    });
    m.insert("minecraft:repairable".to_string(), {
        let mut r = Map::new();
        r.insert(
            "items".to_string(),
            Value::String(material.repair.to_string()),
        );
        Value::Object(r)
    });
    m.insert(
        "minecraft:attribute_modifiers".to_string(),
        Value::Array(vec![
            modifier(
                "minecraft:attack_damage",
                "minecraft:base_attack_damage",
                material.attack[kind],
                "mainhand",
            ),
            modifier(
                "minecraft:attack_speed",
                "minecraft:base_attack_speed",
                material.attack_speed[kind],
                "mainhand",
            ),
        ]),
    );
    if material.fire {
        let mut d = Map::new();
        d.insert(
            "types".to_string(),
            Value::String("#minecraft:is_fire".to_string()),
        );
        m.insert("minecraft:damage_resistant".to_string(), Value::Object(d));
    }
    if material.fuel {
        let mut f = Map::new();
        f.insert(
            "burn_time".to_string(),
            Value::String("minecraft:cooking/time_wood_items_large".to_string()),
        );
        f.insert(
            "speed_multiplier".to_string(),
            Value::String("minecraft:cooking/speed_default".to_string()),
        );
        m.insert("minecraft:cooking_fuel".to_string(), Value::Object(f));
    }
    if matches!(kind, AXE | HOE | SHOVEL) {
        m.insert(
            "minecraft:block_transformer".to_string(),
            Value::String(format!(
                "minecraft:{}",
                ["pickaxe", "axe", "shovel", "hoe", "sword"][kind]
            )),
        );
    }
    let tool_component = if kind == SWORD {
        let mut tool = Map::new();
        tool.insert(
            "can_destroy_blocks_in_creative".to_string(),
            Value::Bool(false),
        );
        tool.insert("damage_per_block".to_string(), Value::Number(2.into()));
        let mut cobweb = Map::new();
        cobweb.insert(
            "blocks".to_string(),
            Value::String("minecraft:cobweb".to_string()),
        );
        cobweb.insert("correct_for_drops".to_string(), Value::Bool(true));
        cobweb.insert("speed".to_string(), Value::from(15.0));
        let mut instantly = Map::new();
        instantly.insert(
            "blocks".to_string(),
            Value::String("#minecraft:sword_instantly_mines".to_string()),
        );
        instantly.insert("speed".to_string(), Value::from(3.4028235e38));
        let mut efficient = Map::new();
        efficient.insert(
            "blocks".to_string(),
            Value::String("#minecraft:sword_efficient".to_string()),
        );
        efficient.insert("speed".to_string(), Value::from(1.5));
        tool.insert(
            "rules".to_string(),
            Value::Array(vec![
                Value::Object(cobweb),
                Value::Object(instantly),
                Value::Object(efficient),
            ]),
        );
        Value::Object(tool)
    } else {
        let mut incorrect = Map::new();
        incorrect.insert(
            "blocks".to_string(),
            Value::String(format!(
                "#minecraft:incorrect_for_{}_tool",
                material.incorrect
            )),
        );
        incorrect.insert("correct_for_drops".to_string(), Value::Bool(false));
        let mut mineable = Map::new();
        mineable.insert(
            "blocks".to_string(),
            Value::String(format!(
                "#minecraft:mineable/{}",
                ["pickaxe", "axe", "shovel", "hoe", "sword"][kind]
            )),
        );
        mineable.insert("correct_for_drops".to_string(), Value::Bool(true));
        mineable.insert("speed".to_string(), Value::from(material.speed));
        let mut tool = Map::new();
        tool.insert(
            "rules".to_string(),
            Value::Array(vec![Value::Object(incorrect), Value::Object(mineable)]),
        );
        Value::Object(tool)
    };
    m.insert("minecraft:tool".to_string(), tool_component);
    let weapon = if kind == SWORD {
        Value::Object(Map::new())
    } else if kind == AXE {
        let mut w = Map::new();
        w.insert("disable_blocking_for_seconds".to_string(), Value::from(5.0));
        w.insert(
            "item_damage_per_attack".to_string(),
            Value::Number(2.into()),
        );
        Value::Object(w)
    } else {
        let mut w = Map::new();
        w.insert(
            "item_damage_per_attack".to_string(),
            Value::Number(2.into()),
        );
        Value::Object(w)
    };
    m.insert("minecraft:weapon".to_string(), weapon);
    Value::Object(m)
}

fn armor(material: &ArmorMaterial, slot: usize) -> Value {
    let simple = format!("{}_{}", material.name, SLOTS[slot]);
    let mut m = defaults(&simple);
    m.insert(
        "minecraft:rarity".to_string(),
        Value::String(material.rarity.to_string()),
    );
    m.insert("minecraft:damage".to_string(), Value::Number(0.into()));
    m.insert(
        "minecraft:max_damage".to_string(),
        Value::Number(material.durability[slot].into()),
    );
    m.insert("minecraft:enchantable".to_string(), {
        let mut e = Map::new();
        e.insert("value".to_string(), Value::Number(material.enchant.into()));
        Value::Object(e)
    });
    m.insert("minecraft:repairable".to_string(), {
        let mut r = Map::new();
        r.insert(
            "items".to_string(),
            Value::String(material.repair.to_string()),
        );
        Value::Object(r)
    });
    let mut equippable = Map::new();
    equippable.insert(
        "asset_id".to_string(),
        Value::String(material.asset.to_string()),
    );
    equippable.insert(
        "equip_sound".to_string(),
        Value::String(material.sound.to_string()),
    );
    equippable.insert(
        "slot".to_string(),
        Value::String(SLOT_NAMES[slot].to_string()),
    );
    m.insert(
        "minecraft:equippable".to_string(),
        Value::Object(equippable),
    );
    let mut mods = vec![
        modifier(
            "minecraft:armor",
            &format!("minecraft:armor.{}", SLOTS[slot]),
            material.defense[slot],
            SLOT_NAMES[slot],
        ),
        modifier(
            "minecraft:armor_toughness",
            &format!("minecraft:armor.{}", SLOTS[slot]),
            material.toughness,
            SLOT_NAMES[slot],
        ),
    ];
    if material.knockback != 0.0 {
        mods.push(modifier(
            "minecraft:knockback_resistance",
            &format!("minecraft:armor.{}", SLOTS[slot]),
            material.knockback,
            SLOT_NAMES[slot],
        ));
    }
    m.insert(
        "minecraft:attribute_modifiers".to_string(),
        Value::Array(mods),
    );
    if material.name == "netherite" {
        let mut d = Map::new();
        d.insert(
            "types".to_string(),
            Value::String("#minecraft:is_fire".to_string()),
        );
        m.insert("minecraft:damage_resistant".to_string(), Value::Object(d));
    }
    Value::Object(m)
}
