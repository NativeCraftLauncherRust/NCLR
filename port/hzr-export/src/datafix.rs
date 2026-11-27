//! Rebuild `datafix-tables.json` from the player's own `client.jar`.
//!
//! `horizon-prepare` runs `VanillaDataFixTablesExporter` against the real 26.3
//! jars; the result is read by `horizon-world`'s `datafix/tables.rs` to upgrade
//! saves made before 1.13. Almost every value in that table is a *static field of
//! a data-fix class*: literal `Map`/`Set`/array initialisers plus the
//! `BlockStateData.register` calls. All of it is bytecode the player already has,
//! so this module re-evaluates those initialisers instead of shipping the table.
//!
//! The evaluator is deliberately small: it runs one method with a symbolic stack
//! and models only the JDK/Guava/fastutil constructors vanilla's initialisers use
//! (`Map.of`, `ImmutableMap.builder()`, `Maps.newHashMap()`,
//! `DataFixUtils.make(x, lambda)`, `Int2ObjectOpenHashMap#put`, …). Anything it
//! does not model becomes [`Value::Other`], which is enough because every key this
//! table carries is either a literal or a `register` call.
//!
//! `classfile.rs` is shared with `hzr-bytecode` (a manifest edit is out of scope),
//! so it is included by path.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::Path;

use serde_json::{Map as JsonMap, Value as Json};

#[path = "../../hzr-bytecode/src/classfile.rs"]
#[allow(dead_code)]
mod classfile;
use classfile::{ClassFile, Member};

/// What `ship.hzr3` calls this table.
pub const TABLE: &str = "datafix-tables.json";

/// Every top-level key `VanillaDataFixTablesExporter` writes, in its order.
pub const KEYS: [&str; 32] = [
    "ITEM_FLATTENING",
    "ITEM_DAMAGE_IDS",
    "BLOCK_IDS",
    "ITEM_IDS",
    "SPECIAL_OBJECTIVE_CRITERIA",
    "STATS_SKIP",
    "STATS_CUSTOM",
    "STATS_ITEM_KEYS",
    "STATS_ENTITY_KEYS",
    "STATS_ENTITIES",
    "RECIPES",
    "RECIPES_RENAMENING",
    "ADVANCEMENTS",
    "GENERATOR_BIOMES",
    "SPAWN_EGGS",
    "ENCHANTMENTS",
    "CORAL",
    "CORAL_FANS",
    "THE_RENAMENING_ENTITIES",
    "THE_RENAMENING_BLOCKS",
    "COD_SALMON",
    "COD_SALMON_EGGS",
    "ENTITIES_V1460",
    "ENTITIES_V99",
    "BLOCK_ENTITIES_V99",
    "SPAWN_EGG_ENTITIES",
    "ENTITY_IDS",
    "BLOCK_ENTITY_IDS",
    "ITEM_TO_BLOCK_ENTITY_V99",
    "ITEM_POTIONS",
    "STRUCTURE_TEMPLATE_RENAMES",
    "BLOCK_STATE_DATA",
];

const FIXES: &str = "net/minecraft/util/datafix/fixes";

/// Which of [`KEYS`] this build produced, and what it could not.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Coverage {
    pub produced: Vec<String>,
    pub missing: Vec<String>,
    pub notes: Vec<String>,
}

impl Coverage {
    pub fn empty() -> Self {
        Coverage {
            produced: Vec::new(),
            missing: KEYS.iter().map(|key| (*key).to_string()).collect(),
            notes: Vec::new(),
        }
    }

    fn add(&mut self, key: &str, entries: usize) {
        self.produced.push(key.to_string());
        self.missing.retain(|name| name != key);
        self.notes.push(format!("{key}: {entries} entries"));
    }

}

// ---------------------------------------------------------------------------
// The jar
// ---------------------------------------------------------------------------

/// The player's `client.jar`, held in memory.
pub struct Jar {
    files: BTreeMap<String, Vec<u8>>,
    classes: std::cell::RefCell<BTreeMap<String, std::rc::Rc<ClassFile>>>,
}

impl Jar {
    pub fn open(path: &Path) -> Result<Self, String> {
        let file = fs::File::open(path).map_err(|error| format!("{}: {error}", path.display()))?;
        let mut zip = zip::ZipArchive::new(file)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        let mut files = BTreeMap::new();
        for index in 0..zip.len() {
            let mut entry = zip.by_index(index).map_err(|error| error.to_string())?;
            if entry.is_dir() {
                continue;
            }
            let name = entry.name().map_err(|error| error.to_string())?.to_string();
            let mut data = Vec::new();
            entry
                .read_to_end(&mut data)
                .map_err(|error| format!("{name}: {error}"))?;
            files.insert(name, data);
        }
        Ok(Jar {
            files,
            classes: std::cell::RefCell::new(BTreeMap::new()),
        })
    }

    /// Parse (and cache) one class by its internal name.
    pub fn class(&self, internal: &str) -> Result<std::rc::Rc<ClassFile>, String> {
        if let Some(class) = self.classes.borrow().get(internal) {
            return Ok(class.clone());
        }
        let name = format!("{internal}.class");
        let bytes = self
            .files
            .get(&name)
            .ok_or_else(|| format!("{name}: not in the jar"))?;
        let class = std::rc::Rc::new(
            classfile::parse(bytes).map_err(|error| format!("{name}: {error}"))?,
        );
        self.classes
            .borrow_mut()
            .insert(internal.to_string(), class.clone());
        Ok(class)
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

/// The length of the instruction at `at` (needed to walk a method).
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

/// How many values a descriptor's parameters occupy on the value stack (a `long`
/// or `double` is one value here; the slot count is irrelevant to us).
fn parameter_count(descriptor: &str) -> usize {
    let bytes = descriptor.as_bytes();
    let mut at = 1; // after '('
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
// Symbolic values
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
enum Value {
    Int(i64),
    Str(String),
    Null,
    Other,
    Map(usize),
    Set(usize),
    Array(usize),
    Builder(usize),
    /// An instance from `new` whose `<init>` has not run yet; `dup` shares it.
    Object(usize),
    /// A method reference from `invokedynamic`/`MethodHandle`.
    Lambda(String),
    /// `com.mojang.datafixers.util.Pair`.
    Pair(Box<Value>, Box<Value>),
    /// `BlockStateData`'s state: a name and its sorted properties.
    State(String, Vec<(String, String)>),
}

#[derive(Clone, Copy, PartialEq)]
enum BuilderKind {
    Map,
    Set,
}

struct Builder {
    kind: BuilderKind,
    entries: Vec<(Value, Value)>,
}

/// The heap and the static fields of one class under evaluation.
#[derive(Default)]
struct Heap {
    statics: BTreeMap<String, Value>,
    maps: Vec<Vec<(Value, Value)>>,
    sets: Vec<Vec<Value>>,
    arrays: Vec<Vec<Value>>,
    builders: Vec<Builder>,
    /// `new` instances: what the constructor left behind, and its class name.
    objects: Vec<Value>,
    object_class: Vec<String>,
    /// The `register` lines of `BlockStateData`, in call order.
    lines: Vec<String>,
    /// Guards runaway recursion through lambdas.
    depth: usize,
}

impl Heap {
    /// Follow `new` instances to what their constructors built.
    fn resolve(&self, value: &Value) -> Value {
        match value {
            Value::Object(slot) => match self.objects.get(*slot) {
                Some(inner) => self.resolve(inner),
                None => Value::Other,
            },
            Value::Pair(key, value) => Value::Pair(
                Box::new(self.resolve(key)),
                Box::new(self.resolve(value)),
            ),
            other => other.clone(),
        }
    }

    fn entries(&self, value: &Value) -> Result<Vec<(Value, Value)>, String> {
        let resolved = self.resolve(value);
        match resolved {
            Value::Map(index) => Ok(self
                .maps
                .get(index)
                .map(|entries| {
                    entries
                        .iter()
                        .map(|(key, value)| (self.resolve(key), self.resolve(value)))
                        .collect()
                })
                .unwrap_or_default()),
            other => Err(format!("expected a map, got {other:?}")),
        }
    }

    fn set_entries(&self, value: &Value) -> Result<Vec<Value>, String> {
        let resolved = self.resolve(value);
        match resolved {
            Value::Set(index) => Ok(self
                .sets
                .get(index)
                .map(|entries| entries.iter().map(|value| self.resolve(value)).collect())
                .unwrap_or_default()),
            other => Err(format!("expected a set, got {other:?}")),
        }
    }

    fn array_entries(&self, value: &Value) -> Result<Vec<Value>, String> {
        let resolved = self.resolve(value);
        match resolved {
            Value::Array(index) => Ok(self
                .arrays
                .get(index)
                .map(|entries| entries.iter().map(|value| self.resolve(value)).collect())
                .unwrap_or_default()),
            other => Err(format!("expected an array, got {other:?}")),
        }
    }

    fn as_string(&self, value: &Value) -> Result<String, String> {
        match self.resolve(value) {
            Value::Str(text) => Ok(text),
            other => Err(format!("expected a string, got {other:?}")),
        }
    }

    fn as_int(&self, value: &Value) -> Result<i64, String> {
        match self.resolve(value) {
            Value::Int(number) => Ok(number),
            other => Err(format!("expected an integer, got {other:?}")),
        }
    }

    fn new_object(&mut self, class: String) -> Value {
        self.objects.push(Value::Other);
        self.object_class.push(class);
        Value::Object(self.objects.len() - 1)
    }

    fn new_map(&mut self, entries: Vec<(Value, Value)>) -> Value {
        self.maps.push(entries);
        Value::Map(self.maps.len() - 1)
    }

    fn new_set(&mut self, entries: Vec<Value>) -> Value {
        self.sets.push(entries);
        Value::Set(self.sets.len() - 1)
    }

    fn new_array(&mut self, entries: Vec<Value>) -> Value {
        self.arrays.push(entries);
        Value::Array(self.arrays.len() - 1)
    }
}

fn int_of(value: &Value) -> Result<i64, String> {
    match value {
        Value::Int(number) => Ok(*number),
        other => Err(format!("expected an integer, got {other:?}")),
    }
}

// ---------------------------------------------------------------------------
// The evaluator
// ---------------------------------------------------------------------------

/// Runs static initialisers and lambdas symbolically.
struct Machine {
    heap: Heap,
}

impl Machine {
    fn new() -> Self {
        Machine {
            heap: Heap::default(),
        }
    }

    /// Run one method with `args` bound to its first locals.
    fn run(&mut self, class: &ClassFile, member: &Member, args: Vec<Value>) -> Result<Value, String> {
        self.heap.depth += 1;
        if self.heap.depth > 64 {
            self.heap.depth -= 1;
            return Err("recursion limit".into());
        }
        let result = self.run_inner(class, member, args);
        self.heap.depth -= 1;
        result
    }

    fn run_inner(
        &mut self,
        class: &ClassFile,
        member: &Member,
        args: Vec<Value>,
    ) -> Result<Value, String> {
        let Some(code) = member.code.as_ref().map(|code| code.bytes.clone()) else {
            return Ok(Value::Other);
        };
        let mut locals: Vec<Value> = vec![Value::Other; member_code_locals(class, member)];
        for (slot, value) in args.into_iter().enumerate() {
            if slot < locals.len() {
                locals[slot] = value;
            }
        }
        let mut stack: Vec<Value> = Vec::new();
        let mut at = 0usize;
        while at < code.len() {
            let opcode = code[at];
            let length = instruction_len(&code, at)?;
            let mut jumped = false;
            match opcode {
                0x01 => stack.push(Value::Null),
                0x02 => stack.push(Value::Int(-1)),
                0x03..=0x08 => stack.push(Value::Int(opcode as i64 - 3)),
                0x09..=0x0f => stack.push(Value::Other),
                0x10 => stack.push(Value::Int(code[at + 1] as i8 as i64)),
                0x11 => stack.push(Value::Int(i16_at(&code, at + 1)? as i64)),
                0x12 => {
                    let index = code[at + 1] as usize;
                    stack.push(self.constant(class, index));
                }
                0x13 => {
                    let index = u16_at(&code, at + 1)?;
                    stack.push(self.constant(class, index));
                }
                0x14 => stack.push(Value::Other),
                0x15 => {
                    let index = code[at + 1] as usize;
                    stack.push(local(&locals, index));
                }
                0x16 | 0x17 | 0x18 => stack.push(Value::Other),
                0x19 => {
                    let index = code[at + 1] as usize;
                    stack.push(local(&locals, index));
                }
                0x1a..=0x1d => stack.push(local(&locals, (opcode - 0x1a) as usize)),
                0x1e..=0x21 => stack.push(Value::Other),
                0x2a..=0x2d => stack.push(local(&locals, (opcode - 0x2a) as usize)),
                0x2e..=0x35 => {
                    // array load: array, index -> value
                    let index = pop(&mut stack);
                    let array = pop(&mut stack);
                    let element = match (&array, int_of(&index)) {
                        (Value::Array(slot), Ok(index)) => self
                            .heap
                            .arrays
                            .get(*slot)
                            .and_then(|entries| entries.get(index as usize))
                            .cloned()
                            .unwrap_or(Value::Other),
                        _ => Value::Other,
                    };
                    stack.push(element);
                }
                0x36 | 0x37 | 0x38 | 0x39 | 0x3a => {
                    let index = code[at + 1] as usize;
                    let value = pop(&mut stack);
                    store(&mut locals, index, value);
                }
                0x3b..=0x3e => {
                    let value = pop(&mut stack);
                    store(&mut locals, (opcode - 0x3b) as usize, value);
                }
                0x4b..=0x4e => {
                    let value = pop(&mut stack);
                    store(&mut locals, (opcode - 0x4b) as usize, value);
                }
                0x53 => {
                    // aastore: array, index, value
                    let value = pop(&mut stack);
                    let index = pop(&mut stack);
                    let array = pop(&mut stack);
                    if let (Value::Array(slot), Ok(index)) = (&array, int_of(&index)) {
                        if let Some(entries) = self.heap.arrays.get_mut(*slot) {
                            if let Some(slot) = entries.get_mut(index as usize) {
                                *slot = value;
                            }
                        }
                    }
                }
                0x57 => {
                    pop(&mut stack);
                }
                0x58 => {
                    pop(&mut stack);
                    let _ = pop(&mut stack);
                }
                0x59 => {
                    let value = stack.last().cloned().unwrap_or(Value::Other);
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
                    let b = pop(&mut stack);
                    let a = pop(&mut stack);
                    let value = match (int_of(&a), int_of(&b)) {
                        (Ok(a), Ok(b)) => match opcode {
                            0x60 => Value::Int(a.wrapping_add(b)),
                            0x64 => Value::Int(a.wrapping_sub(b)),
                            0x68 => Value::Int(a.wrapping_mul(b)),
                            0x6c => {
                                if b == 0 {
                                    Value::Other
                                } else {
                                    Value::Int(a.wrapping_div(b))
                                }
                            }
                            0x70 => {
                                if b == 0 {
                                    Value::Other
                                } else {
                                    Value::Int(a.wrapping_rem(b))
                                }
                            }
                            0x78 => Value::Int(a.wrapping_shl(b as u32)),
                            0x7a => Value::Int(a.wrapping_shr(b as u32)),
                            0x7c => Value::Int(((a as u64).wrapping_shr(b as u32)) as i64),
                            0x7e => Value::Int(a & b),
                            0x80 => Value::Int(a | b),
                            0x82 => Value::Int(a ^ b),
                            _ => Value::Other,
                        },
                        _ => Value::Other,
                    };
                    stack.push(value);
                }
                0x84 => {
                    let index = code[at + 1] as usize;
                    let delta = code[at + 2] as i8 as i64;
                    if let Some(Value::Int(value)) = locals.get(index).cloned() {
                        store(&mut locals, index, Value::Int(value.wrapping_add(delta)));
                    } else {
                        store(&mut locals, index, Value::Other);
                    }
                }
                0x85..=0x98 => {
                    let b = pop(&mut stack);
                    let a = pop(&mut stack);
                    let value = match (opcode, int_of(&a), int_of(&b)) {
                        (0x85, Ok(_), _) => Value::Other,
                        (0x88, Ok(a), _) => Value::Int(a as i32 as i64),
                        (0x91, Ok(a), _) => Value::Int(a as i8 as i64),
                        (0x92, Ok(a), _) => Value::Int(a as i16 as i64),
                        (0x93, Ok(a), _) => Value::Int(a as i32 as i64),
                        (0x94, Ok(a), Ok(b)) => Value::Int(if a == b { 0 } else if a < b { -1 } else { 1 }),
                        (0x95, Ok(a), Ok(b)) => Value::Int(if a < b { -1 } else if a == b { 0 } else { 1 }),
                        (0x96, Ok(a), Ok(b)) => Value::Int(if a > b { 1 } else if a == b { 0 } else { -1 }),
                        _ => Value::Other,
                    };
                    stack.push(value);
                }
                0x99..=0x9e => {
                    let value = pop(&mut stack);
                    let taken = match opcode {
                        0x99 => int_of(&value) == Ok(0),
                        0x9a => int_of(&value) != Ok(0),
                        0x9b => int_of(&value).map(|v| v < 0) == Ok(true),
                        0x9c => int_of(&value).map(|v| v >= 0) == Ok(true),
                        0x9d => int_of(&value).map(|v| v > 0) == Ok(true),
                        _ => int_of(&value).map(|v| v <= 0) == Ok(true),
                    };
                    if taken {
                        at = jump_target(&code, at, 1)?;
                        jumped = true;
                    }
                }
                0x9f..=0xa4 => {
                    let b = pop(&mut stack);
                    let a = pop(&mut stack);
                    let taken = match (int_of(&a), int_of(&b)) {
                        (Ok(a), Ok(b)) => match opcode {
                            0x9f => a == b,
                            0xa0 => a != b,
                            0xa1 => a < b,
                            0xa2 => a >= b,
                            0xa3 => a > b,
                            _ => a <= b,
                        },
                        _ => false,
                    };
                    if taken {
                        at = jump_target(&code, at, 1)?;
                        jumped = true;
                    }
                }
                0xa5 | 0xa6 => {
                    pop(&mut stack);
                    pop(&mut stack);
                }
                0xa7 => {
                    at = jump_target(&code, at, 1)?;
                    jumped = true;
                }
                0xaa => {
                    let index = int_of(&pop(&mut stack)).unwrap_or(0);
                    let pad = (4 - ((at + 1) % 4)) % 4;
                    let body = at + 1 + pad;
                    let default = i32_at(&code, body)?;
                    let low = i32_at(&code, body + 4)?;
                    let high = i32_at(&code, body + 8)?;
                    let offset = if index >= low as i64 && index <= high as i64 {
                        i32_at(&code, body + 12 + ((index - low as i64) as usize) * 4)?
                    } else {
                        default
                    };
                    at = (at as i64 + offset as i64) as usize;
                    jumped = true;
                }
                0xab => {
                    let index = int_of(&pop(&mut stack)).unwrap_or(0);
                    let pad = (4 - ((at + 1) % 4)) % 4;
                    let body = at + 1 + pad;
                    let default = i32_at(&code, body)?;
                    let pairs = i32_at(&code, body + 4)?;
                    let mut offset = default;
                    for pair in 0..pairs.max(0) as usize {
                        let key = i32_at(&code, body + 8 + pair * 8)?;
                        if key as i64 == index {
                            offset = i32_at(&code, body + 12 + pair * 8)?;
                            break;
                        }
                    }
                    at = (at as i64 + offset as i64) as usize;
                    jumped = true;
                }
                0xa8 | 0xa9 | 0xc8 | 0xc9 => return Err("subroutine opcode".into()),
                0xac | 0xad | 0xae | 0xaf => {
                    return Ok(pop(&mut stack));
                }
                0xb0 => {
                    return Ok(pop(&mut stack));
                }
                0xb1 => return Ok(Value::Other),
                0xb2 => {
                    let index = u16_at(&code, at + 1)?;
                    let value = match classfile::field_ref(&class.constants, index) {
                        Some((_, name, _)) => {
                            self.heap.statics.get(&name).cloned().unwrap_or(Value::Other)
                        }
                        None => Value::Other,
                    };
                    stack.push(value);
                }
                0xb3 => {
                    let index = u16_at(&code, at + 1)?;
                    let value = self.heap.resolve(&pop(&mut stack));
                    if let Some((_, name, _)) = classfile::field_ref(&class.constants, index) {
                        self.heap.statics.insert(name, value);
                    }
                }
                0xb4 => {
                    pop(&mut stack);
                    stack.push(Value::Other);
                }
                0xb5 => {
                    pop(&mut stack);
                    pop(&mut stack);
                }
                0xb6 | 0xb7 | 0xb8 | 0xb9 => {
                    let index = u16_at(&code, at + 1)?;
                    let Some((owner, name, descriptor)) =
                        classfile::method_ref(&class.constants, index)
                    else {
                        return Err(format!("no method ref at {at}"));
                    };
                    // Stack order is `..., receiver, arg0, arg1`; the receiver sits
                    // below the arguments, so pop the arguments first.
                    let count = parameter_count(&descriptor);
                    let mut arguments = vec![Value::Other; count];
                    for slot in (0..count).rev() {
                        arguments[slot] = pop(&mut stack);
                    }
                    let receiver = if opcode == 0xb8 {
                        None
                    } else {
                        Some(pop(&mut stack))
                    };
                    let result = self.call(
                        class,
                        &owner,
                        &name,
                        &descriptor,
                        receiver,
                        arguments,
                    )?;
                    if returns_value(&descriptor) {
                        stack.push(result.unwrap_or(Value::Other));
                    }
                }
                0xba => {
                    let index = u16_at(&code, at + 1)?;
                    // The site's own descriptor says how many captured values it pops.
                    if let Some(descriptor) = invokedynamic_descriptor(class, index) {
                        for _ in 0..parameter_count(&descriptor) {
                            pop(&mut stack);
                        }
                    }
                    stack.push(self.lambda(class, index));
                }
                0xbb => {
                    let index = u16_at(&code, at + 1)?;
                    let name = match class.constants.get(index) {
                        Some(classfile::Constant::Class(reference)) => {
                            let inner: usize = reference.trim_start_matches('#').parse().unwrap_or(0);
                            classfile::utf8(&class.constants, inner)
                        }
                        _ => String::new(),
                    };
                    let value = self.heap.new_object(name);
                    stack.push(value);
                }
                0xbc | 0xbd => {
                    let count = int_of(&pop(&mut stack)).unwrap_or(0).max(0) as usize;
                    let value = self.heap.new_array(vec![Value::Null; count]);
                    stack.push(value);
                }
                0xbe => {
                    let value = pop(&mut stack);
                    let length = match value {
                        Value::Array(index) => self.heap.arrays[index].len() as i64,
                        _ => 0,
                    };
                    stack.push(Value::Int(length));
                }
                0xbf => return Err("exception thrown".into()),
                0xc0 => {}
                0xc1 => {
                    pop(&mut stack);
                    stack.push(Value::Int(0));
                }
                0xc2 | 0xc3 => {
                    pop(&mut stack);
                }
                0xc4 => {
                    // wide <load|store|iinc>
                    let next = code[at + 1];
                    let index = u16_at(&code, at + 2)?;
                    match next {
                        0x15 | 0x19 => stack.push(local(&locals, index)),
                        0x36 | 0x3a => {
                            let value = pop(&mut stack);
                            store(&mut locals, index, value);
                        }
                        0x84 => {
                            let delta = i16_at(&code, at + 4)? as i64;
                            if let Some(Value::Int(value)) = locals.get(index).cloned() {
                                store(&mut locals, index, Value::Int(value.wrapping_add(delta)));
                            }
                        }
                        _ => {}
                    }
                }
                0xc5 => {
                    let dimensions = pop(&mut stack);
                    let dimensions = int_of(&dimensions).unwrap_or(0).max(0) as usize;
                    let value = self.heap.new_array(vec![Value::Null; dimensions]);
                    stack.push(value);
                }
                0xc6 | 0xc7 => {
                    let value = pop(&mut stack);
                    let taken = match opcode {
                        0xc6 => matches!(value, Value::Null),
                        _ => !matches!(value, Value::Null),
                    };
                    if taken {
                        at = jump_target(&code, at, 1)?;
                        jumped = true;
                    }
                }
                _ => {}
            }
            if !jumped {
                at += length;
            }
        }
        Ok(Value::Other)
    }

    fn constant(&self, class: &ClassFile, index: usize) -> Value {
        match class.constants.get(index) {
            Some(classfile::Constant::Integer(number)) => Value::Int(*number as i64),
            Some(classfile::Constant::String(_)) | Some(classfile::Constant::Utf8(_)) => {
                match classfile::string_constant(&class.constants, index) {
                    Some(text) => Value::Str(text),
                    None => Value::Other,
                }
            }
            _ => Value::Other,
        }
    }

    /// The implementation method behind an `invokedynamic` site.
    fn lambda(&self, class: &ClassFile, index: usize) -> Value {
        let Some(classfile::Constant::InvokeDynamic { bootstrap, .. }) =
            class.constants.get(index)
        else {
            return Value::Other;
        };
        let Some(row) = class.bootstraps.get(*bootstrap) else {
            return Value::Other;
        };
        for argument in &row.arguments {
            let Some(classfile::Constant::MethodHandle { reference }) = class.constants.get(*argument)
            else {
                continue;
            };
            if let Some((owner, name, _)) = classfile::method_ref(&class.constants, *reference) {
                if owner == class.name {
                    return Value::Lambda(name);
                }
            }
        }
        Value::Other
    }

    /// Dispatch one call; `receiver` is `None` for `invokestatic`.
    #[allow(clippy::too_many_arguments)]
    fn call(
        &mut self,
        class: &ClassFile,
        owner: &str,
        name: &str,
        descriptor: &str,
        receiver: Option<Value>,
        arguments: Vec<Value>,
    ) -> Result<Option<Value>, String> {
        // A call back into the class under evaluation (helper or lambda body).
        if owner == class.name {
            if let Some(member) = class.method(name) {
                let reentrant = name.starts_with("lambda$") || name.starts_with("bootstrap");
                if reentrant && member.descriptor == descriptor {
                    return Ok(Some(self.run(class, member, arguments)?));
                }
            }
        }

        let last = owner.rsplit('/').next().unwrap_or(owner);
        // A `new` instance is a slot shared by every `dup` of it; `<init>` writes
        // into that slot, so resolve the call's own values but keep the slot.
        let receiver_slot = match &receiver {
            Some(Value::Object(slot)) => Some(*slot),
            _ => None,
        };
        let mut values: Vec<Value> = arguments
            .iter()
            .map(|value| self.heap.resolve(value))
            .collect();
        if let Some(receiver) = receiver.clone() {
            values.insert(0, self.heap.resolve(&receiver));
        }
        let short = last.split('$').next().unwrap_or(last);

        let result = match (short, name) {
            // ---- collections --------------------------------------------------
            (_, "<init>") => {
                let built = match last {
                    "Int2ObjectOpenHashMap"
                    | "Object2IntOpenHashMap"
                    | "HashMap"
                    | "LinkedHashMap"
                    | "TreeMap"
                    | "Int2ObjectLinkedOpenHashMap"
                    | "Object2IntLinkedOpenHashMap" => Some(self.heap.new_map(Vec::new())),
                    "HashSet" | "LinkedHashSet" | "TreeSet" => {
                        Some(self.heap.new_set(Vec::new()))
                    }
                    _ => None,
                };
                if let (Some(slot), Some(value)) = (receiver_slot, built.clone()) {
                    if let Some(target) = self.heap.objects.get_mut(slot) {
                        *target = value;
                    }
                }
                built
            }
            ("Map", "of") | ("ImmutableMap", "of") => {
                let mut entries = Vec::new();
                for pair in values.chunks(2) {
                    if pair.len() == 2 {
                        entries.push((pair[0].clone(), pair[1].clone()));
                    }
                }
                Some(self.heap.new_map(entries))
            }
            ("Map", "entry") => {
                if values.len() == 2 {
                    Some(Value::Pair(
                        Box::new(values[0].clone()),
                        Box::new(values[1].clone()),
                    ))
                } else {
                    Some(Value::Other)
                }
            }
            ("ImmutableMap", "builder") => {
                self.heap.builders.push(Builder {
                    kind: BuilderKind::Map,
                    entries: Vec::new(),
                });
                Some(Value::Builder(self.heap.builders.len() - 1))
            }
            ("ImmutableMap", "copyOf") | ("Map", "copyOf") => values.first().cloned(),
            ("ImmutableSet", "builder") => {
                self.heap.builders.push(Builder {
                    kind: BuilderKind::Set,
                    entries: Vec::new(),
                });
                Some(Value::Builder(self.heap.builders.len() - 1))
            }
            ("Set", "of") | ("ImmutableSet", "of") => {
                let mut entries = Vec::new();
                if values.len() == 1 {
                    if let Value::Array(index) = values[0] {
                        entries = self.heap.arrays[index].clone();
                    }
                }
                if entries.is_empty() {
                    entries = values.clone();
                }
                Some(self.heap.new_set(entries))
            }
            ("Set", "copyOf") | ("ImmutableSet", "copyOf") => values.first().cloned(),
            ("List", "of") | ("ImmutableList", "of") => Some(self.heap.new_array(values.clone())),
            ("Maps", "newHashMap") | ("Maps", "newLinkedHashMap") => {
                Some(self.heap.new_map(Vec::new()))
            }
            ("Sets", "newHashSet") | ("Sets", "newLinkedHashSet") => {
                let mut entries = Vec::new();
                if let Some(Value::Array(index)) = values.first() {
                    entries = self.heap.arrays[*index].clone();
                }
                Some(self.heap.new_set(entries))
            }
            ("Lists", "newArrayList") | ("Arrays", "asList") => {
                let mut entries = Vec::new();
                if let Some(Value::Array(index)) = values.first() {
                    entries = self.heap.arrays[*index].clone();
                }
                Some(self.heap.new_array(entries))
            }
            ("Map", "ofEntries") | ("ImmutableMap", "ofEntries") => {
                let mut entries = Vec::new();
                if let Some(Value::Array(index)) = values.first() {
                    for entry in self.heap.arrays[*index].clone() {
                        if let Value::Pair(key, value) = entry {
                            entries.push((*key, *value));
                        }
                    }
                }
                Some(self.heap.new_map(entries))
            }
            // ---- boxed primitives / strings -----------------------------------
            ("Integer", "valueOf")
            | ("Short", "valueOf")
            | ("Byte", "valueOf")
            | ("Long", "valueOf") => values.first().cloned(),
            ("String", "valueOf") => match values.first() {
                Some(Value::Str(text)) => Some(Value::Str(text.clone())),
                Some(Value::Int(number)) => Some(Value::Str(number.to_string())),
                _ => Some(Value::Other),
            },
            // ---- preconditions / builders -------------------------------------
            ("Objects", "requireNonNull") => values.first().cloned(),
            ("DataFixUtils", "make") | ("Util", "make") => {
                let target = values.first().cloned().unwrap_or(Value::Other);
                if let Some(Value::Lambda(method)) = values.get(1).cloned() {
                    if let Some(member) = class.method(&method) {
                        self.run(class, member, vec![target.clone()])?;
                    }
                }
                Some(target)
            }
            ("Pair", "of") if values.len() == 2 => Some(Value::Pair(
                Box::new(values[0].clone()),
                Box::new(values[1].clone()),
            )),
            _ => None,
        };
        if result.is_some() {
            return Ok(result);
        }

        // Methods on a value we model: builders, maps, sets.
        match (name, values.first().cloned()) {
            ("put", Some(Value::Builder(index))) if values.len() >= 3 => {
                self.heap.builders[index]
                    .entries
                    .push((values[1].clone(), values[2].clone()));
                Ok(Some(Value::Builder(index)))
            }
            ("put", Some(Value::Map(index))) if values.len() >= 3 => {
                self.heap.maps[index].push((values[1].clone(), values[2].clone()));
                Ok(Some(Value::Map(index)))
            }
            ("putIfAbsent", Some(Value::Map(index))) if values.len() >= 3 => {
                let key = &values[1];
                if !self.heap.maps[index].iter().any(|(existing, _)| existing == key) {
                    self.heap.maps[index].push((values[1].clone(), values[2].clone()));
                }
                Ok(Some(Value::Map(index)))
            }
            ("add", Some(Value::Builder(index))) if values.len() >= 2 => {
                self.heap.builders[index].entries.push((values[1].clone(), Value::Null));
                Ok(Some(Value::Builder(index)))
            }
            ("add", Some(Value::Set(index))) if values.len() >= 2 => {
                self.heap.sets[index].push(values[1].clone());
                Ok(Some(Value::Set(index)))
            }
            ("build", Some(Value::Builder(index))) => {
                let builder = &self.heap.builders[index];
                match builder.kind {
                    BuilderKind::Map => {
                        let entries = builder.entries.clone();
                        Ok(Some(self.heap.new_map(entries)))
                    }
                    BuilderKind::Set => {
                        let entries: Vec<Value> = builder
                            .entries
                            .iter()
                            .map(|(value, _)| value.clone())
                            .collect();
                        Ok(Some(self.heap.new_set(entries)))
                    }
                }
            }
            // `BlockStateData`
            ("create", _) if class.name.ends_with("BlockStateData") => {
                let name = self.heap.as_string(&values[0])?;
                let properties = match values.get(1) {
                    Some(map) => map_properties(&self.heap, map)?,
                    None => Vec::new(),
                };
                Ok(Some(Value::State(name, properties)))
            }
            ("register", _) if class.name.ends_with("BlockStateData") => {
                let id = self.heap.as_int(&values[0])?;
                let state = state_line(&values[1])?;
                let legacy = self.heap.array_entries(&values[2]).unwrap_or_default();
                let mut line = format!("{id}\t{}", state);
                for entry in legacy {
                    line.push('\t');
                    line.push_str(&state_line(&entry)?);
                }
                self.heap.lines.push(line);
                Ok(None)
            }
            // ---- schema registration helpers ----------------------------------
            // Every helper the schemas register through takes `(Schema?, Map,
            // String, …)` and adds that string to that map.
            _ if owner.contains("datafix")
                && descriptor.contains("Ljava/util/Map;Ljava/lang/String;")
                && values.len() >= 3 =>
            {
                match (values.get(1), values.get(2)) {
                    (Some(Value::Map(index)), Some(Value::Str(name))) => {
                        self.heap.maps[*index].push((Value::Str(name.clone()), Value::Other));
                        Ok(Some(Value::Other))
                    }
                    _ => Ok(Some(Value::Other)),
                }
            }
            // ---- `BlockStateData` ---------------------------------------------
            ("create", _) if class.name.ends_with("BlockStateData") => {
                let name = self.heap.as_string(&values[0])?;
                let properties = match values.get(1) {
                    Some(map) => map_properties(&self.heap, map)?,
                    None => Vec::new(),
                };
                Ok(Some(Value::State(name, properties)))
            }
            ("register", _) if class.name.ends_with("BlockStateData") => {
                let id = self.heap.as_int(&values[0])?;
                let state = state_line(&values[1])?;
                let legacy = self.heap.array_entries(&values[2]).unwrap_or_default();
                let mut line = format!("{id}\t{}", state);
                for entry in legacy {
                    line.push('\t');
                    line.push_str(&state_line(&entry)?);
                }
                self.heap.lines.push(line);
                Ok(None)
            }
            _ => Ok(Some(Value::Other)),
        }
    }
}

/// The descriptor of an `invokedynamic` site: its parameters are the captured
/// values the site pops.
fn invokedynamic_descriptor(class: &ClassFile, index: usize) -> Option<String> {
    let Some(classfile::Constant::InvokeDynamic { name, .. }) = class.constants.get(index) else {
        return None;
    };
    let name_and_type: usize = name.trim_start_matches('#').parse().ok()?;
    let (Some(classfile::Constant::NameAndType { kind, .. }), _) =
        (class.constants.get(name_and_type), ())
    else {
        return None;
    };
    let descriptor_index: usize = kind.trim_start_matches('#').parse().ok()?;
    match class.constants.get(descriptor_index) {
        Some(classfile::Constant::Utf8(descriptor)) => Some(descriptor.clone()),
        _ => None,
    }
}

fn member_code_locals(_class: &ClassFile, member: &Member) -> usize {    member
        .code
        .as_ref()
        .map(|code| code.max_locals as usize)
        .unwrap_or(0)
        .max(8)
}

fn local(locals: &[Value], index: usize) -> Value {
    locals.get(index).cloned().unwrap_or(Value::Other)
}

fn store(locals: &mut [Value], index: usize, value: Value) {
    if let Some(slot) = locals.get_mut(index) {
        *slot = value;
    }
}

fn pop(stack: &mut Vec<Value>) -> Value {
    stack.pop().unwrap_or(Value::Other)
}

fn jump_target(code: &[u8], at: usize, operand: usize) -> Result<usize, String> {
    let offset = i16_at(code, at + operand)?;
    Ok((at as i64 + offset as i64) as usize)
}

fn map_properties(heap: &Heap, value: &Value) -> Result<Vec<(String, String)>, String> {
    let mut properties = Vec::new();
    for (key, value) in heap.entries(value)? {
        properties.push((heap.as_string(&key)?, heap.as_string(&value)?));
    }
    properties.sort();
    Ok(properties)
}

/// `name\tkey=value;key=value`, the shape `BLOCK_STATE_DATA`'s lines use.
fn state_line(value: &Value) -> Result<String, String> {
    match value {
        Value::State(name, properties) => {
            let joined = properties
                .iter()
                .map(|(key, value)| format!("{key}={value}"))
                .collect::<Vec<_>>()
                .join(";");
            Ok(format!("{name}\t{joined}"))
        }
        other => Err(format!("expected a block state, got {other:?}")),
    }
}

// ---------------------------------------------------------------------------
// The table
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Table {
    out: JsonMap<String, Json>,
    coverage: Coverage,
}

impl Table {
    fn evaluate(&mut self, jar: &Jar, class: &str) -> Result<Heap, String> {
        let class = jar.class(class).map_err(|error| format!("{class}: {error}"))?;
        let Some(initializer) = class.method("<clinit>").cloned() else {
            return Err(format!("{}: no <clinit>", class.name));
        };
        let mut machine = Machine::new();
        machine.run(&class, &initializer, Vec::new())?;
        Ok(machine.heap)
    }

    fn put(&mut self, key: &str, value: Json, entries: usize) {
        self.out.insert(key.to_string(), value);
        self.coverage.add(key, entries);
    }
}

fn pairs(heap: &Heap, value: &Value, sort_by_key: bool) -> Result<Json, String> {
    let mut entries = heap.entries(value)?;
    let mut rows: Vec<(String, String)> = Vec::with_capacity(entries.len());
    for (key, value) in entries.drain(..) {
        rows.push((heap.as_string(&key)?, heap.as_string(&value)?));
    }
    if sort_by_key {
        rows.sort();
    }
    Ok(Json::Array(
        rows.into_iter()
            .map(|(key, value)| Json::Array(vec![Json::String(key), Json::String(value)]))
            .collect(),
    ))
}

fn pairs_to_int(heap: &Heap, value: &Value) -> Result<Json, String> {
    let mut rows: Vec<(String, i64)> = Vec::new();
    for (key, value) in heap.entries(value)? {
        rows.push((heap.as_string(&key)?, heap.as_int(&value)?));
    }
    rows.sort();
    Ok(Json::Array(
        rows.into_iter()
            .map(|(key, value)| {
                Json::Array(vec![Json::String(key), Json::Number(value.into())])
            })
            .collect(),
    ))
}

fn int_pairs(heap: &Heap, value: &Value) -> Result<Json, String> {
    let mut rows: Vec<(i64, String)> = Vec::new();
    for (key, value) in heap.entries(value)? {
        rows.push((heap.as_int(&key)?, heap.as_string(&value)?));
    }
    rows.sort();
    Ok(Json::Array(
        rows.into_iter()
            .map(|(key, value)| {
                Json::Array(vec![Json::Number(key.into()), Json::String(value)])
            })
            .collect(),
    ))
}

fn strings(heap: &Heap, value: &Value) -> Result<Json, String> {
    let mut rows: Vec<String> = Vec::new();
    for entry in heap.set_entries(value)? {
        rows.push(heap.as_string(&entry)?);
    }
    rows.sort();
    rows.dedup();
    Ok(Json::Array(rows.into_iter().map(Json::String).collect()))
}

fn indexed(heap: &Heap, value: &Value) -> Result<Json, String> {
    let entries = heap.array_entries(value)?;
    let mut rows: Vec<(i64, String)> = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        if matches!(entry, Value::Null | Value::Other) {
            continue;
        }
        rows.push((index as i64, heap.as_string(entry)?));
    }
    Ok(Json::Array(
        rows.into_iter()
            .map(|(key, value)| {
                Json::Array(vec![Json::Number(key.into()), Json::String(value)])
            })
            .collect(),
    ))
}

fn structure_renames(heap: &Heap, value: &Value) -> Result<Json, String> {
    let mut rows: Vec<(String, String, Vec<(String, String)>)> = Vec::new();
    for (key, value) in heap.entries(value)? {
        let Value::Pair(name, templates) = value else {
            return Err(format!("expected a Pair, got {value:?}"));
        };
        let mut inner: Vec<(String, String)> = Vec::new();
        for (key, value) in heap.entries(&templates)? {
            inner.push((heap.as_string(&key)?, heap.as_string(&value)?));
        }
        inner.sort();
        rows.push((heap.as_string(&key)?, heap.as_string(&name)?, inner));
    }
    rows.sort();
    Ok(Json::Array(
        rows.into_iter()
            .map(|(key, name, templates)| {
                let templates = Json::Array(
                    templates
                        .into_iter()
                        .map(|(key, value)| {
                            Json::Array(vec![Json::String(key), Json::String(value)])
                        })
                        .collect(),
                );
                Json::Array(vec![Json::String(key), Json::String(name), templates])
            })
            .collect(),
    ))
}

/// The `datafix-tables.json` document this jar can reproduce, plus what it could
/// not.
pub fn datafix_tables(jar_path: &Path) -> Result<(Json, Coverage), String> {
    let jar = Jar::open(jar_path)?;
    let mut table = Table::default();
    table.coverage = Coverage::empty();

    // ---- literal maps, sets and arrays of the fixes classes ------------------
    // (class, field, output kind); every one is a static field the exporter reads.
    let flattening = table.evaluate(&jar, &format!("{FIXES}/ItemStackTheFlatteningFix"))?;
    let value = flattening
        .statics
        .get("MAP")
        .cloned()
        .ok_or("ItemStackTheFlatteningFix.MAP")?;
    let json = pairs(&flattening, &value, true)?;
    table.put("ITEM_FLATTENING", json, flattening.entries(&value)?.len());

    let value = flattening
        .statics
        .get("DAMAGE_IDS")
        .cloned()
        .ok_or("ItemStackTheFlatteningFix.DAMAGE_IDS")?;
    let json = strings(&flattening, &value)?;
    table.put("ITEM_DAMAGE_IDS", json, flattening.set_entries(&value)?.len());

    // BLOCK_IDS is the non-null part of EntityBlockStateFix.MAP.
    let blocks = table.evaluate(&jar, &format!("{FIXES}/EntityBlockStateFix"))?;
    let value = blocks
        .statics
        .get("MAP")
        .cloned()
        .ok_or("EntityBlockStateFix.MAP")?;
    let json = pairs_to_int(&blocks, &value)?;
    table.put("BLOCK_IDS", json, blocks.entries(&value)?.len());

    let item_ids = table.evaluate(&jar, &format!("{FIXES}/ItemIdFix"))?;
    let value = item_ids
        .statics
        .get("ITEM_NAMES")
        .cloned()
        .ok_or("ItemIdFix.ITEM_NAMES")?;
    let json = int_pairs(&item_ids, &value)?;
    table.put("ITEM_IDS", json, item_ids.entries(&value)?.len());

    let stats = table.evaluate(&jar, &format!("{FIXES}/StatsCounterFix"))?;
    for (field, output, kind) in [
        ("SPECIAL_OBJECTIVE_CRITERIA", "SPECIAL_OBJECTIVE_CRITERIA", Kind::Set),
        ("SKIP", "STATS_SKIP", Kind::Set),
        ("CUSTOM_MAP", "STATS_CUSTOM", Kind::Pairs),
        ("ITEM_KEYS", "STATS_ITEM_KEYS", Kind::Pairs),
        ("ENTITY_KEYS", "STATS_ENTITY_KEYS", Kind::Pairs),
        ("ENTITIES", "STATS_ENTITIES", Kind::Pairs),
    ] {
        let value = stats.statics.get(field).cloned().ok_or(field)?;
        let (json, entries) = match kind {
            Kind::Set => {
                let entries = stats.set_entries(&value)?.len();
                (strings(&stats, &value)?, entries)
            }
            Kind::Pairs => {
                let entries = stats.entries(&value)?.len();
                (pairs(&stats, &value, true)?, entries)
            }
        };
        table.put(output, json, entries);
    }

    for (class, field, output) in [
        ("RecipesFix", "RECIPES", "RECIPES"),
        ("RecipesRenameningFix", "RECIPES", "RECIPES_RENAMENING"),
        ("AdvancementsFix", "RENAMES", "ADVANCEMENTS"),
        ("LevelDataGeneratorOptionsFix", "MAP", "GENERATOR_BIOMES"),
        ("ItemStackSpawnEggFix", "MAP", "SPAWN_EGGS"),
        ("RenamedCoralFix", "RENAMED_IDS", "CORAL"),
        ("RenamedCoralFansFix", "RENAMED_IDS", "CORAL_FANS"),
        ("EntityTheRenameningFix", "RENAMED_IDS", "THE_RENAMENING_ENTITIES"),
        ("EntityTheRenameningFix", "RENAMED_BLOCKS", "THE_RENAMENING_BLOCKS"),
        ("EntityCodSalmonFix", "RENAMED_IDS", "COD_SALMON"),
        ("EntityCodSalmonFix", "RENAMED_EGG_IDS", "COD_SALMON_EGGS"),
        ("EntityIdFix", "ID_MAP", "ENTITY_IDS"),
        ("BlockEntityIdFix", "ID_MAP", "BLOCK_ENTITY_IDS"),
    ] {
        let heap = table.evaluate(&jar, &format!("{FIXES}/{class}"))?;
        let value = heap.statics.get(field).cloned().ok_or(field)?;
        let json = pairs(&heap, &value, true)?;
        table.put(output, json, heap.entries(&value)?.len());
    }

    let enchantments = table.evaluate(&jar, &format!("{FIXES}/ItemStackEnchantmentNamesFix"))?;
    let value = enchantments
        .statics
        .get("MAP")
        .cloned()
        .ok_or("ItemStackEnchantmentNamesFix.MAP")?;
    let json = int_pairs(&enchantments, &value)?;
    table.put("ENCHANTMENTS", json, enchantments.entries(&value)?.len());

    let spawn_eggs = table.evaluate(&jar, &format!("{FIXES}/ItemSpawnEggFix"))?;
    let value = spawn_eggs
        .statics
        .get("ID_TO_ENTITY")
        .cloned()
        .ok_or("ItemSpawnEggFix.ID_TO_ENTITY")?;
    let json = indexed(&spawn_eggs, &value)?;
    let count = json.as_array().map(|rows| rows.len()).unwrap_or(0);
    table.put("SPAWN_EGG_ENTITIES", json, count);

    let potions = table.evaluate(&jar, &format!("{FIXES}/ItemPotionFix"))?;
    let value = potions
        .statics
        .get("POTIONS")
        .cloned()
        .ok_or("ItemPotionFix.POTIONS")?;
    let json = indexed(&potions, &value)?;
    let count = json.as_array().map(|rows| rows.len()).unwrap_or(0);
    table.put("ITEM_POTIONS", json, count);

    let structures = table.evaluate(&jar, &format!("{FIXES}/ChunkStructuresTemplateRenameFix"))?;
    let value = structures
        .statics
        .get("RENAMES")
        .cloned()
        .ok_or("ChunkStructuresTemplateRenameFix.RENAMES")?;
    let json = structure_renames(&structures, &value)?;
    table.put("STRUCTURE_TEMPLATE_RENAMES", json, structures.entries(&value)?.len());

    let v99 = table.evaluate(&jar, "net/minecraft/util/datafix/schemas/V99")?;
    let value = v99
        .statics
        .get("ITEM_TO_BLOCKENTITY")
        .cloned()
        .ok_or("V99.ITEM_TO_BLOCKENTITY")?;
    let json = pairs(&v99, &value, true)?;
    table.put("ITEM_TO_BLOCK_ENTITY_V99", json, v99.entries(&value)?.len());

    // ---- schema registries ---------------------------------------------------
    for (class, output) in [
        ("V1460", "ENTITIES_V1460"),
        ("V99", "ENTITIES_V99"),
        ("V99", "BLOCK_ENTITIES_V99"),
    ] {
        let internal = format!("net/minecraft/util/datafix/schemas/{class}");
        let run = |method: &str| -> Result<BTreeSet<String>, String> {
            let class_file = jar.class(&internal)?;
            let Some(member) = class_file.method(method).cloned() else {
                return Err(format!("{internal}.{method}"));
            };
            let mut machine = Machine::new();
            let value = machine.run(&class_file, &member, vec![Value::Other, Value::Other])?;
            let mut keys = BTreeSet::new();
            for (key, _) in machine.heap.entries(&value)? {
                keys.insert(machine.heap.as_string(&key)?);
            }
            Ok(keys)
        };
        let keys = if output == "BLOCK_ENTITIES_V99" {
            run("registerBlockEntities")?
        } else {
            run("registerEntities")?
        };
        let json = Json::Array(keys.iter().cloned().map(Json::String).collect());
        table.put(output, json, keys.len());
    }

    // ---- BlockStateData ------------------------------------------------------
    let class = jar.class(&format!("{FIXES}/BlockStateData"))?;
    let Some(initializer) = class.method("<clinit>").cloned() else {
        return Err("BlockStateData has no <clinit>".into());
    };
    let mut machine = Machine::new();
    machine.run(&class, &initializer, Vec::new())?;
    let lines = machine.heap.lines.clone();
    let json = Json::Array(lines.iter().cloned().map(Json::String).collect());
    table.put("BLOCK_STATE_DATA", json, lines.len());

    Ok((Json::Object(table.out), table.coverage))
}

enum Kind {
    Set,
    Pairs,
}

/// Write [`TABLE`] into `out_dir` and return what could not be produced.
pub fn write(jar_path: &Path, out_dir: &Path) -> Result<Coverage, String> {
    let (document, coverage) = datafix_tables(jar_path)?;
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
    fn a_state_line_is_the_name_then_sorted_properties() {
        let state = Value::State(
            "minecraft:oak_door".into(),
            vec![
                ("facing".into(), "east".into()),
                ("open".into(), "true".into()),
            ],
        );
        assert_eq!(
            state_line(&state).unwrap(),
            "minecraft:oak_door\tfacing=east;open=true"
        );
        // No properties: the trailing tab is still there (the field is empty).
        assert_eq!(
            state_line(&Value::State("minecraft:air".into(), Vec::new())).unwrap(),
            "minecraft:air\t"
        );
        assert!(state_line(&Value::Other).is_err());
    }

    #[test]
    fn descriptors_count_values_not_slots() {
        assert_eq!(parameter_count("()V"), 0);
        assert_eq!(parameter_count("(I)V"), 1);
        assert_eq!(
            parameter_count("(Ljava/util/Map;Ljava/lang/String;Ljava/util/function/Function;)V"),
            3
        );
        // A long/double takes two slots but one value here.
        assert_eq!(parameter_count("(JD)V"), 2);
        // Arrays count once, whatever their component.
        assert_eq!(parameter_count("([Ljava/lang/Object;I)V"), 2);
        assert!(returns_value("(I)Ljava/lang/String;"));
        assert!(!returns_value("(I)V"));
    }
}
