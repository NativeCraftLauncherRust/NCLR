//! `hzr-export entity-types|attributes` - entity registry and attribute tables.
//! This group compiles only `entity.rs` (plus the shared class-file reader).

#[path = "../../../hzr-bytecode/src/classfile.rs"]
#[allow(dead_code)]
mod classfile;

#[path = "../entity.rs"]
mod entity;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.as_slice() {
        [cmd, jar, out] if cmd == "entity-types" => entity::write_entity_types(jar, out),
        [cmd, jar, out] if cmd == "attributes" => entity::write_attributes(jar, out),
        _ => {
            eprintln!("hzr-export(entity): usage: hzr-export-entity <entity-types|attributes> <client.jar> <out.json>");
            std::process::exit(2);
        }
    };
    if let Err(error) = result {
        eprintln!("hzr-export: {error}");
        std::process::exit(1);
    }
}
