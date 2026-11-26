//! The `biome-parameters` tables, checked against the copies `horizon-prepare` produced
//! and against vanilla's own constants.
//!
//! The two parity tests need the player's `mc263/client.jar` and the prepared tables; when
//! either is missing (a machine without a Minecraft install, CI) they say so and pass,
//! because the exporter's contract is "given this jar, this report" and there is nothing
//! to compare without a jar. The formatting test needs neither.

#[path = "../src/worldgen.rs"]
mod worldgen;

use std::path::{Path, PathBuf};

/// `D:\我的世界rust构写`: three levels above this crate.
fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn jar_path() -> PathBuf {
    workspace().join("mc263/client.jar")
}

/// The prepared tables this exporter replaces, or `None` when this machine has none.
fn prepared() -> Option<PathBuf> {
    let root = workspace().join("tmp/ship-extract");
    if root.join("biome-parameters/overworld.json").is_file() {
        Some(root)
    } else {
        None
    }
}

fn open_jar() -> Option<worldgen::Jar> {
    if !jar_path().is_file() {
        eprintln!("skipping: {} is not there", jar_path().display());
        return None;
    }
    Some(worldgen::Jar::open(jar_path().to_str().expect("a utf-8 path")).expect("client.jar opens"))
}

fn parameter(min: i64, max: i64) -> worldgen::Parameter {
    worldgen::Parameter { min, max }
}

#[test]
fn a_span_is_written_as_a_pair_and_a_point_as_one_number() {
    // Vanilla's own report shape, and the 1/10000 grid of `Climate.quantizeCoord`.
    let rows = vec![worldgen::BiomeRow {
        biome: "minecraft:mushroom_fields".to_string(),
        point: worldgen::ParameterPoint {
            temperature: parameter(-10_000, 10_000),
            humidity: parameter(0, 0),
            continentalness: parameter(-12_000, -10_500),
            erosion: parameter(-7_799, -7_799),
            depth: parameter(0, 0),
            weirdness: parameter(0, 0),
            offset: 7_000,
        },
    }];
    let expected = concat!(
        "{\n",
        "  \"biomes\": [\n",
        "    {\n",
        "      \"biome\": \"minecraft:mushroom_fields\",\n",
        "      \"parameters\": {\n",
        "        \"continentalness\": [\n",
        "          -1.2,\n",
        "          -1.05\n",
        "        ],\n",
        "        \"depth\": 0.0,\n",
        "        \"erosion\": -0.7799,\n",
        "        \"humidity\": 0.0,\n",
        "        \"offset\": 0.7,\n",
        "        \"temperature\": [\n",
        "          -1.0,\n",
        "          1.0\n",
        "        ],\n",
        "        \"weirdness\": 0.0\n",
        "      }\n",
        "    }\n",
        "  ]\n",
        "}",
    );
    assert_eq!(worldgen::render(&rows), expected);
}

#[test]
fn the_nether_table_is_vanilla_s_five_points_in_list_order() {
    let Some(jar) = open_jar() else { return };
    let rows = worldgen::table(&jar, "nether").expect("the nether table");
    let summary: Vec<(&str, [i64; 7])> = rows
        .iter()
        .map(|row| {
            let point = &row.point;
            (
                row.biome.as_str(),
                [
                    point.temperature.min,
                    point.humidity.min,
                    point.continentalness.min,
                    point.erosion.min,
                    point.depth.min,
                    point.weirdness.min,
                    point.offset,
                ],
            )
        })
        .collect();
    // `MultiNoiseBiomeSourceParameterList$Preset$1.apply`: five `Climate.parameters(FFFFFFF)`
    // calls, all points, in this order.
    assert_eq!(
        summary,
        vec![
            ("minecraft:nether_wastes", [0, 0, 0, 0, 0, 0, 0]),
            ("minecraft:soul_sand_valley", [0, -5_000, 0, 0, 0, 0, 0]),
            ("minecraft:crimson_forest", [4_000, 0, 0, 0, 0, 0, 0]),
            ("minecraft:warped_forest", [0, 5_000, 0, 0, 0, 0, 3_750]),
            ("minecraft:basalt_deltas", [-5_000, 0, 0, 0, 0, 0, 1_750]),
        ]
    );
}

#[test]
fn the_parameter_tables_match_the_prepared_copies() {
    let (Some(jar), Some(root)) = (open_jar(), prepared()) else {
        return;
    };
    for preset in worldgen::PRESETS {
        let table =
            worldgen::table(&jar, preset).unwrap_or_else(|error| panic!("{preset}: {error}"));
        let rendered = worldgen::render(&table);
        // Vanilla's data generator writes these reports; `horizon-prepare` copies them both
        // into `biome-parameters/` and into `worldgen/biome_parameters/`.
        let reference_path = root.join("biome-parameters").join(format!("{preset}.json"));
        let reference = std::fs::read_to_string(&reference_path)
            .unwrap_or_else(|error| panic!("{}: {error}", reference_path.display()));
        let value: serde_json::Value = serde_json::from_str(&rendered)
            .unwrap_or_else(|error| panic!("{preset}: our report is not JSON: {error}"));
        let expected: serde_json::Value = serde_json::from_str(&reference)
            .unwrap_or_else(|error| panic!("{}: {error}", reference_path.display()));
        assert_eq!(
            value, expected,
            "{preset}: the table differs from the prepared copy"
        );
        // Same formatting too: the report is Gson's pretty printing, which `render` mirrors.
        assert_eq!(
            rendered, reference,
            "{preset}: the bytes differ from the prepared copy"
        );
        let copied = root
            .join("worldgen/biome_parameters")
            .join(format!("{preset}.json"));
        if copied.is_file() {
            let copied_text = std::fs::read_to_string(&copied).expect("the copied table");
            assert_eq!(
                copied_text, reference,
                "{preset}: the two prepared copies should be identical"
            );
        }
    }
    // Vanilla's own point counts, as a check that nothing was dropped.
    assert_eq!(
        worldgen::table(&jar, "overworld").expect("overworld").len(),
        7_594
    );
    assert_eq!(worldgen::table(&jar, "nether").expect("nether").len(), 5);
}

#[test]
fn export_writes_all_four_prepared_paths() {
    if open_jar().is_none() || prepared().is_none() {
        return;
    }
    let Some(root) = prepared() else { return };
    let out = std::env::temp_dir().join(format!(
        "hzr-export-biome-parameters-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&out);
    worldgen::export(jar_path().to_str().expect("a utf-8 path"), &out).expect("the four files");
    for preset in worldgen::PRESETS {
        for relative in [
            format!("biome-parameters/{preset}.json"),
            format!("worldgen/biome_parameters/{preset}.json"),
        ] {
            let written = std::fs::read_to_string(out.join(&relative))
                .unwrap_or_else(|error| panic!("{relative}: {error}"));
            let expected = std::fs::read_to_string(root.join(&relative))
                .unwrap_or_else(|error| panic!("{}: {error}", root.join(&relative).display()));
            assert_eq!(written, expected, "{relative}");
        }
    }
    let _ = std::fs::remove_dir_all(&out);
}
