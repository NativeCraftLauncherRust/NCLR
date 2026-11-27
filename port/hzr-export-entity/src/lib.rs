//! `hzr-export entity-types|attributes` - entity registry and attribute tables.
//! This group compiles only `entity.rs` (plus the shared class-file reader).

#[path = "../../hzr-bytecode/src/classfile.rs"]
#[allow(dead_code)]
mod classfile;

#[path = "../../hzr-export/src/entity.rs"]
mod entity;

/// Run one of this group's commands; `args` are the CLI arguments.
pub fn run(args: &[String]) -> Result<(), String> {
    match args {
        [cmd, jar, out] if cmd == "entity-types" => entity::write_entity_types(jar, out),
        [cmd, jar, out] if cmd == "attributes" => entity::write_attributes(jar, out),
        _ => {
            eprintln!("hzr-export(entity): usage: hzr-export-entity <entity-types|attributes> <client.jar> <out.json>");
            return Err(String::from("usage"));
        }
    }
}
