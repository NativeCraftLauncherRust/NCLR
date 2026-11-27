//! Close the last `commands.json` gap: the `gamerule` command.
//!
//! `commands_more` reproduces 93 of vanilla's 94 root commands; `gamerule` is
//! dropped there because `GameRuleCommand.register` builds its children through
//! `GameRules.visitGameRuleTypes`, which walks the `BuiltInRegistries.GAME_RULE`
//! registry through a `GameRuleMap` built from a `Stream` and dispatches to an
//! anonymous `GameRuleTypeVisitor` whose default method the evaluator cannot enter.
//!
//! The rules themselves are not data: `GameRules.<clinit>` registers each one with
//! `registerBoolean("advance_time", …)`, `registerInteger("random_tick_speed", …)`,
//! and the visitor turns each rule into
//!
//! ```text
//! <name>       literal, executable, child "value": argument(rule.argument())
//! minecraft:<name>  the same node under the rule's resource location
//! ```
//!
//! So this module reads `GameRules.<clinit>` from the player's own jar with a tiny
//! stack machine (the registration calls' names and integer bounds are constants
//! there), builds that one subtree, and merges it into the table `commands_more`
//! produces. Nothing is read from `ship.hzr3`.
//!
//! Shape of the subtree, as `ArgumentUtils.serializeNodeToJson` writes it: the
//! `gamerule` literal carries the `gamemasters` permission; every rule literal and
//! every `value` argument is executable; `brigadier:bool` has no properties and
//! `brigadier:integer` carries `min`/`max` (omitted when the argument type's
//! defaults are used).

use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::Path;

use serde_json::{Map as JsonMap, Value as Json};

#[path = "../../hzr-bytecode/src/classfile.rs"]
#[allow(dead_code)]
mod classfile;
use classfile::ClassFile;

/// `commands_more` builds the other 93 commands; this module adds the last one.
pub const TABLE: &str = commands_more::TABLE;
pub use commands_more::Coverage;

#[path = "commands_more.rs"]
mod commands_more;

/// One gamerule, as `GameRules.<clinit>` registers it.
#[derive(Clone, Debug, PartialEq)]
pub struct Rule {
    pub name: String,
    /// `None` for `brigadier:bool`, `Some((min, max))` for `brigadier:integer`.
    pub bounds: Option<(i64, i64)>,
}

impl Rule {
    fn parser(&self) -> &'static str {
        match self.bounds {
            None => "brigadier:bool",
            Some(_) => "brigadier:integer",
        }
    }

    fn properties(&self) -> Option<Json> {
        let (min, max) = self.bounds?;
        let mut object = JsonMap::new();
        if min != i32::MIN as i64 {
            object.insert("min".into(), Json::from(min));
        }
        if max != i32::MAX as i64 {
            object.insert("max".into(), Json::from(max));
        }
        if object.is_empty() {
            None
        } else {
            Some(Json::Object(object))
        }
    }
}

/// The player's `client.jar`, just enough of it to read `GameRules.class`.
struct Jar {
    classes: BTreeMap<String, Vec<u8>>,
}

impl Jar {
    fn open(path: &Path) -> Result<Self, String> {
        let file = fs::File::open(path).map_err(|error| format!("{}: {error}", path.display()))?;
        let mut zip = zip::ZipArchive::new(file)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        let mut classes = BTreeMap::new();
        for index in 0..zip.len() {
            let mut entry = zip.by_index(index).map_err(|error| error.to_string())?;
            if entry.is_dir() {
                continue;
            }
            let name = entry.name().map_err(|error| error.to_string())?.to_string();
            if !name.ends_with(".class") {
                continue;
            }
            let mut data = Vec::new();
            entry
                .read_to_end(&mut data)
                .map_err(|error| format!("{name}: {error}"))?;
            classes.insert(name, data);
        }
        Ok(Jar { classes })
    }

    fn class(&self, internal: &str) -> Result<ClassFile, String> {
        let name = format!("{internal}.class");
        let bytes = self
            .classes
            .get(&name)
            .ok_or_else(|| format!("{name}: not in the jar"))?;
        classfile::parse(bytes).map_err(|error| format!("{name}: {error}"))
    }
}

// ---------------------------------------------------------------------------
// Bytecode helpers
// ---------------------------------------------------------------------------

fn u16_at(code: &[u8], at: usize) -> Result<usize, String> {
    if at + 2 > code.len() {
        return Err(format!("bytecode ends inside an operand at {at}"));
    }
    Ok(((code[at] as usize) << 8) | code[at + 1] as usize)
}

fn i16_at(code: &[u8], at: usize) -> Result<i32, String> {
    Ok(u16_at(code, at)? as i16 as i32)
}

fn i32_at(code: &[u8], at: usize) -> Result<i32, String> {
    if at + 4 > code.len() {
        return Err(format!("bytecode ends inside an operand at {at}"));
    }
    Ok(i32::from_be_bytes([
        code[at],
        code[at + 1],
        code[at + 2],
        code[at + 3],
    ]))
}

/// The length of the instruction at `at`.
fn instruction_len(code: &[u8], at: usize) -> Result<usize, String> {
    let opcode = *code.get(at).ok_or_else(|| format!("bytecode ends at {at}"))?;
    let length = match opcode {
        0x00..=0x0f => 1,
        0x10 => 2,
        0x11 => 3,
        0x12 => 2,
        0x13 | 0x14 => 3,
        0x15..=0x19 => 2,
        0x1a..=0x35 => 1,
        0x36..=0x3a => 2,
        0x3b..=0x83 => 1,
        0x84 => 3,
        0x85..=0x98 => 1,
        0x99..=0xa8 => 3,
        0xa9 => 2,
        0xaa => {
            let pad = (4 - ((at + 1) % 4)) % 4;
            let body = at + 1 + pad;
            let low = i32_at(code, body + 4)?;
            let high = i32_at(code, body + 8)?;
            1 + pad + 12 + ((high - low + 1).max(0) as usize) * 4
        }
        0xab => {
            let pad = (4 - ((at + 1) % 4)) % 4;
            let body = at + 1 + pad;
            let pairs = i32_at(code, body + 4)?;
            1 + pad + 8 + (pairs.max(0) as usize) * 8
        }
        0xac..=0xb1 => 1,
        0xb2..=0xb8 => 3,
        0xb9 | 0xba => 5,
        0xbb => 3,
        0xbc => 2,
        0xbd => 3,
        0xbe | 0xbf => 1,
        0xc0 | 0xc1 => 3,
        0xc2 | 0xc3 => 1,
        0xc4 => {
            let next = *code.get(at + 1).ok_or("wide out of range")?;
            if next == 0x84 {
                6
            } else {
                4
            }
        }
        0xc5 => 4,
        0xc6 | 0xc7 => 3,
        0xc8 | 0xc9 => 5,
        other => return Err(format!("unmodelled opcode {other:#04x} at {at}")),
    };
    if at + length > code.len() {
        return Err(format!("instruction at {at} runs past the code"));
    }
    Ok(length)
}

fn parameter_count(descriptor: &str) -> usize {
    let bytes = descriptor.as_bytes();
    let mut at = 1;
    let mut count = 0;
    while at < bytes.len() && bytes[at] != b')' {
        match bytes[at] {
            b'[' => {
                while at < bytes.len() && bytes[at] == b'[' {
                    at += 1;
                }
                if at < bytes.len() && bytes[at] == b'L' {
                    while at < bytes.len() && bytes[at] != b';' {
                        at += 1;
                    }
                }
                at += 1;
            }
            b'L' => {
                while at < bytes.len() && bytes[at] != b';' {
                    at += 1;
                }
                at += 1;
            }
            _ => at += 1,
        }
        count += 1;
    }
    count
}

fn returns_value(descriptor: &str) -> bool {
    match descriptor.rfind(')') {
        Some(at) => !descriptor[at + 1..].starts_with('V'),
        None => true,
    }
}

// ---------------------------------------------------------------------------
// The stack machine that runs `GameRules.<clinit>`
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
enum Val {
    Int(i64),
    Str(String),
    Other,
}

fn int_of(value: &Val) -> i64 {
    match value {
        Val::Int(number) => *number,
        _ => i64::MIN,
    }
}

fn str_of(value: &Val) -> Option<String> {
    match value {
        Val::Str(text) => Some(text.clone()),
        _ => None,
    }
}

fn pop(stack: &mut Vec<Val>) -> Val {
    stack.pop().unwrap_or(Val::Other)
}

fn pop_n(stack: &mut Vec<Val>, count: usize) -> Vec<Val> {
    let mut values = vec![Val::Other; count];
    for slot in (0..count).rev() {
        values[slot] = pop(stack);
    }
    values
}

fn constant(class: &ClassFile, index: usize) -> Val {
    match class.constants.get(index) {
        Some(classfile::Constant::Integer(number)) => Val::Int(*number as i64),
        Some(classfile::Constant::Long(number)) => Val::Int(*number),
        Some(classfile::Constant::String(_)) | Some(classfile::Constant::Utf8(_)) => {
            match classfile::string_constant(&class.constants, index) {
                Some(text) => Val::Str(text),
                None => Val::Other,
            }
        }
        _ => Val::Other,
    }
}

/// Read every `registerBoolean`/`registerInteger` call out of `GameRules.<clinit>`.
fn rules(jar: &Jar) -> Result<Vec<Rule>, String> {
    const GAME_RULES: &str = "net/minecraft/world/level/gamerules/GameRules";
    let class = jar.class(GAME_RULES)?;
    let initializer = class
        .method("<clinit>")
        .ok_or("GameRules.<clinit> is missing")?;
    let code = &initializer
        .code
        .as_ref()
        .ok_or("GameRules.<clinit> has no code")?
        .bytes;
    let mut out = Vec::new();
    let mut stack: Vec<Val> = Vec::new();
    let mut at = 0usize;
    while at < code.len() {
        let opcode = code[at];
        let length = instruction_len(code, at)?;
        let mut jumped = false;
        match opcode {
            0x01 => stack.push(Val::Other),
            0x02 => stack.push(Val::Int(-1)),
            0x03..=0x08 => stack.push(Val::Int(opcode as i64 - 3)),
            0x09..=0x0f => stack.push(Val::Other),
            0x10 => stack.push(Val::Int(code[at + 1] as i8 as i64)),
            0x11 => stack.push(Val::Int(i16_at(code, at + 1)? as i64)),
            0x12 | 0x13 => {
                let index = if opcode == 0x12 {
                    code[at + 1] as usize
                } else {
                    u16_at(code, at + 1)?
                };
                stack.push(constant(&class, index));
            }
            0x14 => stack.push(Val::Other),
            // loads / stores: the values are not needed, only the stack shape
            0x15..=0x19 => stack.push(Val::Other),
            0x1a..=0x2d => stack.push(Val::Other),
            0x2e..=0x35 => {
                pop(&mut stack);
                pop(&mut stack);
                stack.push(Val::Other);
            }
            0x36..=0x4e => {
                pop(&mut stack);
            }
            0x53..=0x56 => {
                pop(&mut stack);
                pop(&mut stack);
                pop(&mut stack);
            }
            0x57 => {
                pop(&mut stack);
            }
            0x58 => {
                pop(&mut stack);
                pop(&mut stack);
            }
            0x59 => {
                let value = stack.last().cloned().unwrap_or(Val::Other);
                stack.push(value);
            }
            0x5a => {
                let a = pop(&mut stack);
                let b = pop(&mut stack);
                stack.push(a.clone());
                stack.push(b);
                stack.push(a);
            }
            0x60..=0x83 => {
                pop(&mut stack);
                pop(&mut stack);
                stack.push(Val::Other);
            }
            0x84 => {}
            0x85..=0x93 => {
                pop(&mut stack);
                stack.push(Val::Other);
            }
            0x94..=0x98 => {
                pop(&mut stack);
                pop(&mut stack);
                stack.push(Val::Other);
            }
            0x99..=0x9e => {
                pop(&mut stack);
            }
            0x9f..=0xa4 => {
                pop(&mut stack);
                pop(&mut stack);
            }
            0xa5 | 0xa6 => {
                pop(&mut stack);
                pop(&mut stack);
            }
            0xa7 => {
                // `goto`: follow it, or dead code would unbalance the stack.
                at = (at as i64 + i16_at(code, at + 1)? as i64) as usize;
                jumped = true;
            }
            0xa8 | 0xa9 | 0xc8 | 0xc9 => return Err("subroutine opcode".into()),
            0xaa | 0xab => {
                // A switch: we only need the stack shape, and the clinit has none.
                pop(&mut stack);
            }
            0xac..=0xb1 => break,
            0xb2 => stack.push(Val::Other),
            0xb3 => {
                pop(&mut stack);
            }
            0xb4 => {
                pop(&mut stack);
                stack.push(Val::Other);
            }
            0xb5 => {
                pop(&mut stack);
                pop(&mut stack);
            }
            0xb6 | 0xb7 | 0xb8 | 0xb9 => {
                let index = u16_at(code, at + 1)?;
                let Some((owner, name, descriptor)) =
                    classfile::method_ref(&class.constants, index)
                else {
                    return Err(format!("no method ref at {at}"));
                };
                let count = parameter_count(&descriptor);
                let arguments = pop_n(&mut stack, count);
                if opcode != 0xb8 {
                    pop(&mut stack); // receiver
                }
                let is_registration = opcode == 0xb8 && owner == GAME_RULES;
                if is_registration {
                    let registration = match name.as_str() {
                        "registerBoolean" => Some(registration_boolean(&arguments)),
                        "registerInteger" => Some(registration_integer(&arguments)),
                        _ => None,
                    };
                    if let Some(rule) = registration.flatten() {
                        out.push(rule);
                    }
                } else if returns_value(&descriptor) {
                    stack.push(Val::Other);
                }
            }
            0xba => {
                let index = u16_at(code, at + 1)?;
                if let Some(descriptor) = invokedynamic_descriptor(&class, index) {
                    let count = parameter_count(&descriptor);
                    pop_n(&mut stack, count);
                }
                stack.push(Val::Other);
            }
            0xbb | 0xbd | 0xc0 | 0xc1 => {
                // `new`/`anewarray` push; `checkcast`/`instanceof` leave one value.
                if opcode == 0xbb {
                    stack.push(Val::Other);
                } else if opcode == 0xbd {
                    pop(&mut stack);
                    stack.push(Val::Other);
                }
            }
            0xbc => {
                pop(&mut stack);
                stack.push(Val::Other);
            }
            0xbe => {
                pop(&mut stack);
                stack.push(Val::Other);
            }
            0xbf => return Err("exception thrown".into()),
            0xc2 | 0xc3 => {
                pop(&mut stack);
            }
            0xc4 => {
                let next = code[at + 1];
                if next == 0x15 || next == 0x19 {
                    stack.push(Val::Other);
                } else if next == 0x36 || next == 0x3a {
                    pop(&mut stack);
                }
            }
            0xc5 => {
                pop(&mut stack);
                pop(&mut stack);
                stack.push(Val::Other);
            }
            0xc6 | 0xc7 => {
                pop(&mut stack);
            }
            _ => {}
        }
        if !jumped {
            at += length;
        }
    }
    Ok(out)
}

fn registration_boolean(arguments: &[Val]) -> Option<Rule> {
    let name = str_of(arguments.first()?)?;
    Some(Rule { name, bounds: None })
}

fn registration_integer(arguments: &[Val]) -> Option<Rule> {
    let name = str_of(arguments.first()?)?;
    // `registerInteger(name, category, default, min)` fills in `Integer.MAX_VALUE`
    // as the maximum; the five/six-argument overloads pass both bounds.
    let min = arguments.get(3).map(int_of).unwrap_or(i32::MIN as i64);
    let max = arguments
        .get(4)
        .map(int_of)
        .unwrap_or(i32::MAX as i64);
    Some(Rule {
        name,
        bounds: Some((min, max)),
    })
}

fn invokedynamic_descriptor(class: &ClassFile, index: usize) -> Option<String> {
    let Some(classfile::Constant::InvokeDynamic { name, .. }) = class.constants.get(index) else {
        return None;
    };
    let name_and_type: usize = name.trim_start_matches('#').parse().ok()?;
    let Some(classfile::Constant::NameAndType { kind, .. }) = class.constants.get(name_and_type)
    else {
        return None;
    };
    let descriptor_index: usize = kind.trim_start_matches('#').parse().ok()?;
    match class.constants.get(descriptor_index) {
        Some(classfile::Constant::Utf8(descriptor)) => Some(descriptor.clone()),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// The subtree
// ---------------------------------------------------------------------------

/// The `gamerule` literal node, built from the rules `GameRules.<clinit>` registers.
pub fn gamerule_node(rules: &[Rule]) -> Json {
    let mut children = JsonMap::new();
    for rule in rules {
        let mut value = JsonMap::new();
        value.insert("type".into(), Json::String("argument".into()));
        value.insert("executable".into(), Json::Bool(true));
        value.insert("parser".into(), Json::String(rule.parser().into()));
        if let Some(properties) = rule.properties() {
            value.insert("properties".into(), properties);
        }
        let mut node = JsonMap::new();
        node.insert("type".into(), Json::String("literal".into()));
        node.insert("executable".into(), Json::Bool(true));
        let mut node_children = JsonMap::new();
        node_children.insert("value".into(), Json::Object(value));
        node.insert("children".into(), Json::Object(node_children));
        let node = Json::Object(node);
        // The visitor adds the rule under both its bare name and its resource location.
        children.insert(rule.name.clone(), node.clone());
        children.insert(format!("minecraft:{}", rule.name), node);
    }
    let mut root = JsonMap::new();
    root.insert("type".into(), Json::String("literal".into()));
    root.insert(
        "permissions".into(),
        serde_json::json!({
            "type": "minecraft:require",
            "permission": { "type": "minecraft:command_level", "level": "gamemasters" },
        }),
    );
    root.insert("children".into(), Json::Object(children));
    Json::Object(root)
}

/// The `commands.json` document, with `gamerule` added to `commands_more`'s tree.
pub fn commands_tables(jar_path: &Path) -> Result<(Json, Coverage), String> {
    let jar = Jar::open(jar_path)?;
    let rules = rules(&jar)?;
    let (mut document, mut coverage) = commands_more::commands_tables(jar_path)?;
    let object = document
        .as_object_mut()
        .ok_or("commands_more returned something that is not an object")?;
    let children = object
        .get_mut("children")
        .and_then(Json::as_object_mut)
        .ok_or("commands_more returned no children")?;
    children.insert("gamerule".into(), gamerule_node(&rules));
    if !coverage.produced.iter().any(|name| name == "gamerule") {
        coverage.produced.push("gamerule".to_string());
        coverage.produced.sort();
    }
    coverage
        .skipped
        .retain(|skipped| !skipped.contains("GameRuleCommand"));
    coverage.notes.push(format!(
        "gamerule: {} rules read from GameRules.<clinit>",
        rules.len()
    ));
    Ok((document, coverage))
}

/// Write [`TABLE`] into `out_dir` and return what was produced.
pub fn write(jar_path: &Path, out_dir: &Path) -> Result<Coverage, String> {
    let (document, coverage) = commands_tables(jar_path)?;
    fs::create_dir_all(out_dir).map_err(|error| format!("{}: {error}", out_dir.display()))?;
    let path = out_dir.join(TABLE);
    let text = serde_json::to_string_pretty(&document)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    fs::write(&path, text + "\n").map_err(|error| format!("{}: {error}", path.display()))?;
    Ok(coverage)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_boolean_rule_is_an_executable_literal_with_a_value_argument() {
        let node = gamerule_node(&[Rule {
            name: "advance_time".into(),
            bounds: None,
        }]);
        let advance = &node["children"]["advance_time"];
        assert_eq!(advance["type"], "literal");
        assert_eq!(advance["executable"], true);
        assert_eq!(advance["children"]["value"]["parser"], "brigadier:bool");
        assert_eq!(advance["children"]["value"]["executable"], true);
        assert!(advance["children"]["value"].get("properties").is_none());
        // The same node under the rule's resource location.
        assert_eq!(node["children"]["minecraft:advance_time"], *advance);
        assert_eq!(node["type"], "literal");
        assert!(node.get("executable").is_none());
        assert_eq!(
            node["permissions"]["permission"]["level"],
            Json::String("gamemasters".into())
        );
    }

    #[test]
    fn integer_rule_bounds_are_the_argument_types_bounds() {
        let node = gamerule_node(&[Rule {
            name: "random_tick_speed".into(),
            bounds: Some((0, 8)),
        }]);
        let value = &node["children"]["random_tick_speed"]["children"]["value"];
        assert_eq!(value["parser"], "brigadier:integer");
        assert_eq!(value["properties"], serde_json::json!({"min": 0, "max": 8}));
        // A default maximum is left out, like `IntegerArgumentType.integer`.
        let node = gamerule_node(&[Rule {
            name: "respawn_radius".into(),
            bounds: Some((0, i32::MAX as i64)),
        }]);
        assert_eq!(
            node["children"]["respawn_radius"]["children"]["value"]["properties"],
            serde_json::json!({"min": 0})
        );
    }
}
