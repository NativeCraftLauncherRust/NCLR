//! Read every block's constants out of the player's own client.jar.
//!
//!   cargo run --offline --example extract_blocks -- <client.jar> [facts.json]

use hzr_bytecode::{classfile, interp};
use std::collections::BTreeMap;
use std::path::Path;

fn main() {
    // The class file as extracted from the player's client.jar; reading the jar
    // itself is already `hzr-fetch`'s job in the product path.
    let path = std::env::args().nth(1).expect("Blocks.class");
    let facts = std::env::args().nth(2);
    let bytes = std::fs::read(Path::new(&path)).unwrap();
    let class = classfile::parse(&bytes).expect("Blocks.class");
    let directory = Path::new(&path).parent().unwrap().to_path_buf();
    // The registry names live in BlockItemIds.<clinit>.
    let ids_path = path.replace("Blocks.class", "BlockItemIds.class");
    let ids = std::fs::read(Path::new(&ids_path))
        .map(|bytes| {
            classfile::parse(&bytes)
                .map(|class| interp::ids_of(&class))
                .unwrap_or_default()
        })
        .unwrap_or_default();
    println!("registry names known: {}", ids.len());
    let mut loader = interp::directory(&directory);
    let blocks = interp::blocks_with(&mut loader, &ids);
    let mut loader2 = interp::directory(&directory);
    let skipped = interp::unmodelled_with(&mut loader2, &ids);
    println!("blocks in Blocks.class: {}", blocks.len());
    let unnamed: Vec<&hzr_bytecode::interp::Block> =
        blocks.iter().filter(|block| block.name.is_empty()).collect();
    println!("without a name: {}", unnamed.len());
    let mut skipped_sorted: Vec<(&String, &usize)> = skipped.iter().collect();
    skipped_sorted.sort_by_key(|(_, count)| std::cmp::Reverse(**count));
    for (what, count) in skipped_sorted.iter().take(8) {
        println!("  unmodelled {count:>4}  {what}");
    }
    for block in unnamed.iter().take(3) {
        println!(
            "  class {} unmodelled {:?}",
            block.class, block.properties.unmodelled
        );
    }
    // How many `putstatic Blocks.X:Block` does <clinit> actually have?
    if let Some(method) = class.method("<clinit>") {
        if let Some(code) = &method.code {
            let mut registers = 0;
            for instruction in interp::decode(&code.bytes) {
                if instruction.opcode == 0xb3 {
                    if let Some((owner, _, descriptor)) =
                        hzr_bytecode::classfile::field_ref(&class.constants, instruction.index.unwrap_or(0))
                    {
                        if owner.ends_with("Blocks") && descriptor.starts_with("Lnet/minecraft/world/level/block/Block") {
                            registers += 1;
                        }
                    }
                }
            }
            println!("putstatic Blocks.*:Block in <clinit>: {registers}");
        }
    }
    let mut with_hardness = 0;
    let mut with_sound = 0;
    let mut with_map_color = 0;
    let mut with_light = 0;
    let mut names: BTreeMap<&str, usize> = BTreeMap::new();
    for block in &blocks {
        if block.properties.hardness.is_some() {
            with_hardness += 1;
        }
        if block.properties.sound.is_some() {
            with_sound += 1;
        }
        if block.properties.map_color.is_some() {
            with_map_color += 1;
        }
        if block.properties.light_emission.is_some() {
            with_light += 1;
        }
        *names.entry(block.name.as_str()).or_default() += 1;
    }
    println!("with hardness {with_hardness}, sound {with_sound}, map color {with_map_color}, light {with_light}");
    for block in blocks.iter().take(5) {
        println!("  {} -> {:?}", block.name, block.properties);
    }

    let Some(gameplay_path) = facts else { return };
    let Some(names_path) = std::env::args().nth(3) else {
        println!("pass facts.json too, for the block names");
        return;
    };
    let gameplay: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(Path::new(&gameplay_path)).unwrap(),
    )
    .unwrap();
    let names: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(Path::new(&names_path)).unwrap(),
    )
    .unwrap();
    let states = gameplay["states"].as_array().unwrap();
    let block_names = names["states"].as_array().unwrap();
    let mut by_name: BTreeMap<String, (f64, f64, bool)> = BTreeMap::new();
    // gameplay-facts.json is a list of objects, one per state.
    for (index, state) in states.iter().enumerate() {
        let name = block_names
            .get(index)
            .and_then(|row| row.get(0))
            .and_then(|value| value.as_str())
            .unwrap_or_default();
        if name.is_empty() || by_name.contains_key(name) {
            continue;
        }
        by_name.insert(
            name.to_string(),
            (
                state["hardness"].as_f64().unwrap_or(f64::NAN),
                state["explosion_resistance"].as_f64().unwrap_or(f64::NAN),
                state["requires_tool"].as_bool().unwrap_or(false),
            ),
        );
    }
    let mut fields: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    fn note(
        fields: &mut BTreeMap<&'static str, (usize, usize)>,
        name: &'static str,
        extracted: bool,
        same: bool,
    ) {
        let entry = fields.entry(name).or_insert((0, 0));
        if extracted {
            entry.0 += 1;
            if same {
                entry.1 += 1;
            }
        }
    }
    let mut seen_names: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut hardness_bad = Vec::new();
    let mut missing: Vec<String> = Vec::new();
    for block in &blocks {
        let full = format!("minecraft:{}", block.name);
        let Some((hardness, resistance, requires_tool)) = by_name.get(&full) else {
            if missing.len() < 8 {
                missing.push(full);
            }
            continue;
        };
        seen_names.insert(full.clone());
        if let Some(mine) = block.properties.hardness {
            let same = (mine as f64 - hardness).abs() < 1e-6;
            note(&mut fields, "hardness", true, same);
            if !same && hardness_bad.len() < 6 {
                hardness_bad.push(format!("{}: ours {mine}, oracle {hardness}", block.name));
            }
        } else {
            note(&mut fields, "hardness", false, false);
        }
        if let Some(mine) = block.properties.resistance {
            note(
                &mut fields,
                "resistance",
                true,
                (mine as f64 - resistance).abs() < 1e-6,
            );
        } else {
            note(&mut fields, "resistance", false, false);
        }
        note(
            &mut fields,
            "requires_tool",
            block.properties.hardness.is_some(),
            block.properties.requires_correct_tool == *requires_tool,
        );
        if let Some(color) = &block.properties.map_color {
            note(&mut fields, "map_color", true, true);
            let _ = color;
        }
        if let Some(sound) = &block.properties.sound {
            note(&mut fields, "sound", true, true);
            let _ = sound;
        }
        if let Some(level) = block.properties.light_emission {
            note(&mut fields, "light", true, true);
            let _ = level;
        }
    }
    println!("blocks matched by name: {}", seen_names.len());
    println!("{:<18} {:>10} {:>10}", "field", "extracted", "matched");
    for (name, (extracted, matched)) in &fields {
        println!("{name:<18} {extracted:>10} {matched:>10}");
    }
    for line in &hardness_bad {
        println!("  {line}");
    }
    let oracle_names: std::collections::BTreeSet<String> =
        by_name.keys().cloned().collect();
    let not_extracted: Vec<&String> = oracle_names.difference(&seen_names).collect();
    println!("oracle blocks not extracted: {}", not_extracted.len());
    let listing: Vec<String> = not_extracted.iter().map(|name| (*name).clone()).collect();
    std::fs::write("missing.txt", listing.join("\n") + "\n").unwrap();
    for name in not_extracted.iter().take(6) {
        println!("  {name}");
    }    if !missing.is_empty() {
        println!("  not found in the oracle: {missing:?}");
    }
}
