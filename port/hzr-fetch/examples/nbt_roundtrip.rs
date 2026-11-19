//! Parse each block table and write it back; the bytes must be identical.
//!
//!   cargo run --offline --example nbt_roundtrip -- <dir with the tables>

use std::path::Path;

fn main() {
    let dir = std::env::args().nth(1).expect("a folder with the .nbt tables");
    let dir = Path::new(&dir);
    let mut bad = 0;
    for name in [
        "occlusion.nbt",
        "cached-collision.nbt",
        "movement-properties.nbt",
        "worldgen/blocks/cached-support.nbt",
        "worldgen/blocks/block-properties.nbt",
    ] {
        let path = dir.join(name);
        let Ok(bytes) = std::fs::read(&path) else {
            println!("{name:<40} missing");
            continue;
        };
        let (root_name, tag) = match hzr_fetch::nbt_value::parse(&bytes) {
            Ok(parsed) => parsed,
            Err(error) => {
                println!("{name:<40} parse failed: {error}");
                bad += 1;
                continue;
            }
        };
        let again = hzr_fetch::nbt_value::write(&root_name, &tag);
        let keys = tag.as_compound().map(|e| e.len()).unwrap_or(0);
        let names: Vec<&str> = tag
            .as_compound()
            .map(|entries| entries.iter().map(|(key, _)| key.as_str()).collect())
            .unwrap_or_default();
        if again == bytes {
            println!(
                "{name:<40} round trip exact ({keys} keys: {})",
                names.join(", ")
            );
        } else {
            println!(
                "{name:<40} DIFFERS: {} bytes in, {} out",
                bytes.len(),
                again.len()
            );
            bad += 1;
        }
    }
    if bad > 0 {
        std::process::exit(1);
    }
    println!("every table round trips byte for byte");
}
