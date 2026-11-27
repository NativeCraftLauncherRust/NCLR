//! Rebuild vanilla's `commands.json` from the player's own `client.jar`.
//!
//! `horizon-prepare` takes `commands.json` from vanilla's **data generator**
//! (`net.minecraft.data.info.CommandsReport`): build
//! `new Commands(CommandSelection.ALL, buildContext)`, then serialize
//! `getDispatcher().getRoot()` with `ArgumentUtils.serializeNodeToJson`. So the
//! whole table is the Brigadier tree that the 97 `XCommand.register(dispatcher)`
//! calls in `Commands.<init>` assemble — code, not data.
//!
//! This module re-assembles that tree by *executing* the registrations with a
//! small symbolic JVM:
//!
//! * the opcode loop runs one method at a time (`<clinit>`, a command class's
//!   `register`, a helper it calls), with objects, arrays, `String`/`int`
//!   constants, branches and loops;
//! * Brigadier is modelled (`LiteralArgumentBuilder`, `RequiredArgumentBuilder`,
//!   `CommandDispatcher`, `CommandNode`) because the library's class files are not
//!   in the game jar — only their method references are;
//! * vanilla's argument types are recognised by the class the factory returns,
//!   with the parser id taken from `ArgumentTypeInfos.bootstrap` (the class/name
//!   pairs it registers) and the properties from the factory's own arguments;
//! * `ResourceKey`s come from executing `Registries.<clinit>`, and the permission
//!   levels from `Commands`' own `LEVEL_*` constants.
//!
//! A registration that the interpreter cannot follow is rolled back (its nodes are
//! discarded) and reported, so the produced tree only ever holds subtrees that were
//! fully assembled: [`Coverage`] says which commands are in it and which are not.
//! Taint tracking decides that: if a registration branches on a value the evaluator
//! does not model (`CommandBuildContext` feature flags, registry contents, an
//! unmodelled lambda being invoked by a helper), whatever it built afterwards may not
//! be what vanilla builds, and the whole registration is dropped.
//!
//! What is still missing (10 of vanilla's 94 root commands), and why:
//!
//! * `execute` — its conditions are assembled through `ExecuteCommand`'s own
//!   helper/lambda machinery and every condition redirects to the `execute` node;
//! * `data`, `function`, `item`, `scoreboard`, `compute`, `difficulty`, `gamerule` —
//!   their subtrees are enumerated at registration time from `GameRules`
//!   (`visitGameRuleTypes`), built-in registries, or enum/`ByIdMap` lookups the
//!   evaluator turns into `Other`, so a branch depends on unknown data;
//! * `loot`, `test` — registration passes a *lambda* into a helper that invokes it
//!   (`LootContextSources.addContextSources`, `TestCommand`'s case builder), which the
//!   evaluator cannot enter.
//!
//! Brigadier's own sources are not in the jar, so its merge rules (`addChild`
//! merging a node with an existing child of the same name, `build()` recursively)
//! are reimplemented from the published behaviour; `classfile.rs` is shared with
//! `hzr-bytecode` by path.

use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::Path;

use serde_json::{Map as JsonMap, Value as Json};

#[path = "../../hzr-bytecode/src/classfile.rs"]
#[allow(dead_code)]
mod classfile;
use classfile::{ClassFile, Member};

/// What `ship.hzr3` calls this table.
pub const TABLE: &str = "commands.json";

const DISPATCHER: &str = "com/mojang/brigadier/CommandDispatcher";
const LITERAL_BUILDER: &str = "com/mojang/brigadier/builder/LiteralArgumentBuilder";
const REQUIRED_BUILDER: &str = "com/mojang/brigadier/builder/RequiredArgumentBuilder";
const COMMANDS: &str = "net/minecraft/commands/Commands";
const ARGUMENT_INFOS: &str = "net/minecraft/commands/synchronization/ArgumentTypeInfos";
const REGISTRIES: &str = "net/minecraft/core/registries/Registries";
const PERMISSIONS: &str = "net/minecraft/server/permissions/Permissions";

/// What the build produced, and which root commands it could not.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Coverage {
    /// Root command names present in the produced tree.
    pub produced: Vec<String>,
    /// Registration classes that were rolled back, with the reason.
    pub skipped: Vec<String>,
    /// Diagnostic counts (nodes, steps, …).
    pub notes: Vec<String>,
}

impl Coverage {
    fn note(&mut self, note: String) {
        self.notes.push(note);
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

    pub fn has(&self, internal: &str) -> bool {
        self.files.contains_key(&format!("{internal}.class"))
    }

    pub fn class(&self, internal: &str) -> Result<std::rc::Rc<ClassFile>, String> {
        if let Some(class) = self.classes.borrow().get(internal) {
            return Ok(class.clone());
        }
        let name = format!("{internal}.class");
        let bytes = self
            .files
            .get(&name)
            .ok_or_else(|| format!("{name}: not in the jar"))?;
        let class =
            std::rc::Rc::new(classfile::parse(bytes).map_err(|error| format!("{name}: {error}"))?);
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
            1 + pad + 8 + (pairs.max(0) as usize) * 4 * 2
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

/// The return type of a method descriptor, as an internal class name when it is
/// an object type (`Lnet/minecraft/...;` → `net/minecraft/...`).
fn return_class(descriptor: &str) -> Option<String> {
    let at = descriptor.rfind(')')?;
    let rest = &descriptor[at + 1..];
    let inner = rest.strip_prefix('L')?.strip_suffix(';')?;
    Some(inner.to_string())
}

fn member_locals(member: &Member) -> usize {
    member
        .code
        .as_ref()
        .map(|code| code.max_locals as usize)
        .unwrap_or(0)
        .max(8)
}

// ---------------------------------------------------------------------------
// Symbolic values
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
enum Value {
    Int(i64),
    Float(f64),
    Str(String),
    Null,
    Other,
    /// An instance; `new` allocates the slot and `<init>` fills it in.
    Object(usize),
    /// A built command-tree node.
    Node(usize),
    /// An argument type instance: parser id plus its serialized properties.
    Argument(usize),
    /// A Brigadier builder's `requires` value.
    Requirement(Option<String>),
    /// A `ResourceKey` (`minecraft:advancement`, …).
    Key(String),
    /// A `Commands.CommandSelection` constant, with the two flags it carries.
    Selection { dedicated: bool, integrated: bool },
    /// A lambda: the implementation method name.
    Lambda(String),
    List(usize),
}

#[derive(Clone, Debug, Default)]
struct Obj {
    class: String,
    fields: BTreeMap<String, Value>,
}

#[derive(Clone, Debug)]
struct ArgumentDef {
    parser: String,
    properties: Option<Json>,
}

#[derive(Clone, Debug, PartialEq)]
enum NodeKind {
    Root,
    Literal(String),
    Argument {
        name: String,
        parser: String,
        properties: Option<Json>,
    },
}

#[derive(Clone, Debug)]
struct Node {
    kind: NodeKind,
    children: Vec<(String, usize)>,
    command: bool,
    /// `Some(level)` when the requirement is a `PermissionProviderCheck`;
    /// `None` when there is no such requirement.
    permission: Option<Option<String>>,
    redirect: Option<usize>,
}

impl Node {
    fn name(&self) -> String {
        match &self.kind {
            NodeKind::Root => String::new(),
            NodeKind::Literal(name) => name.clone(),
            NodeKind::Argument { name, .. } => name.clone(),
        }
    }
}

/// The heap and every static field touched by the evaluation.
#[derive(Default)]
struct Heap {
    statics: BTreeMap<(String, String), Value>,
    objects: Vec<Obj>,
    arguments: Vec<ArgumentDef>,
    lists: Vec<Vec<Value>>,
    nodes: Vec<Node>,
    /// Classes whose static initialiser has run (JVM class initialisation).
    initialized: std::collections::BTreeSet<String>,
    /// Diagnostics: how many bytecode instructions ran.
    steps: usize,
    /// How many builders were converted into nodes (guards runaway merging).
    builds: usize,
    /// A branch was taken on a value the evaluator does not model, so whatever the
    /// registration did after it may not be what vanilla does.
    tainted: bool,
    depth: usize,
}

impl Heap {
    fn object(&self, value: &Value) -> Option<&Obj> {
        match value {
            Value::Object(slot) => self.objects.get(*slot),
            _ => None,
        }
    }

    fn fields<'a>(&'a self, value: &'a Value) -> Option<&'a BTreeMap<String, Value>> {
        self.object(value).map(|object| &object.fields)
    }

    fn new_object(&mut self, class: &str) -> Value {
        self.objects.push(Obj {
            class: class.to_string(),
            fields: BTreeMap::new(),
        });
        Value::Object(self.objects.len() - 1)
    }

    fn new_list(&mut self, entries: Vec<Value>) -> Value {
        self.lists.push(entries);
        Value::List(self.lists.len() - 1)
    }

    fn list(&self, value: &Value) -> Option<&Vec<Value>> {
        match value {
            Value::List(slot) => self.lists.get(*slot),
            _ => None,
        }
    }

    fn new_node(&mut self, node: Node) -> usize {
        self.nodes.push(node);
        self.nodes.len() - 1
    }

    fn root(&self) -> Option<usize> {
        self.nodes
            .iter()
            .position(|node| matches!(node.kind, NodeKind::Root))
    }

    fn new_argument(&mut self, parser: String, properties: Option<Json>) -> Value {
        self.arguments.push(ArgumentDef { parser, properties });
        Value::Argument(self.arguments.len() - 1)
    }

    /// Set a field on a modeled object.
    fn set(&mut self, value: &Value, field: &str, entry: Value) {
        if let Value::Object(slot) = value {
            if let Some(object) = self.objects.get_mut(*slot) {
                object.fields.insert(field.to_string(), entry);
            }
        }
    }

    fn get(&self, value: &Value, field: &str) -> Value {
        self.fields(value)
            .and_then(|fields| fields.get(field).cloned())
            .unwrap_or(Value::Null)
    }
}

fn int_of(value: &Value) -> i64 {
    match value {
        Value::Int(number) => *number,
        _ => 0,
    }
}

fn float_of(value: &Value) -> f64 {
    match value {
        Value::Float(number) => *number,
        Value::Int(number) => *number as f64,
        _ => 0.0,
    }
}

fn str_of(value: &Value) -> Option<String> {
    match value {
        Value::Str(text) => Some(text.clone()),
        _ => None,
    }
}

fn key_of(value: &Value) -> Option<String> {
    match value {
        Value::Key(id) => Some(id.clone()),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// The evaluator
// ---------------------------------------------------------------------------

struct Machine<'a> {
    jar: &'a Jar,
    /// Internal argument-type class -> parser id (`ArgumentTypeInfos.bootstrap`).
    argument_ids: BTreeMap<String, String>,
    /// Registrations that were rolled back, with the reason.
    skipped: Vec<String>,
    /// The builder chain currently being built (`a/b/c`), for error messages.
    path: Vec<String>,
    /// The interpreted call stack, for error messages.
    trace: Vec<String>,
    /// Recursion depth of `CommandNode.addChild` merging.
    merge_depth: usize,
    heap: Heap,
}

impl<'a> Machine<'a> {
    fn new(jar: &'a Jar) -> Self {
        Machine {
            jar,
            argument_ids: BTreeMap::new(),
            skipped: Vec::new(),
            path: Vec::new(),
            trace: Vec::new(),
            merge_depth: 0,
            heap: Heap::default(),
        }
    }

    // -- method execution ---------------------------------------------------

    /// Run a class's `<clinit>` once, the way the JVM does before a static member
    /// of it is read. Classes that would bootstrap the world are skipped.
    fn init_class(&mut self, name: &str) -> Result<(), String> {
        if name.starts_with('[')
            || name.starts_with("com/mojang/brigadier")
            || name.starts_with("java/")
            || !self.jar.has(name)
            || skip_initialisation(name)
        {
            return Ok(());
        }
        if !self.heap.initialized.insert(name.to_string()) {
            return Ok(());
        }
        let class = self.jar.class(name)?;
        if let Some(initializer) = class.method("<clinit>").cloned() {
            self.run(&class, &initializer, Vec::new())?;
        }
        Ok(())
    }

    /// Run one registration, rolling it back if the evaluator cannot follow it:
    /// the tree must never keep half a command.
    fn run_maybe_transactional(
        &mut self,
        owner: &str,
        class: &std::rc::Rc<ClassFile>,
        member: &Member,
        values: Vec<Value>,
    ) -> Result<Option<Value>, String> {
        if member.name != "register" {
            return Ok(Some(self.run(class, member, values)?));
        }
        let saved_nodes = self.heap.nodes.clone();
        let saved_statics = self.heap.statics.clone();
        let saved_taint = self.heap.tainted;
        self.heap.tainted = false;
        let result = self.run(class, member, values);
        let tainted = self.heap.tainted;
        self.heap.tainted = saved_taint;
        match result {
            Ok(value) if !tainted => Ok(Some(value)),
            Ok(_) => {
                // The registration took a decision on a value the evaluator does
                // not know: its subtree cannot be trusted, so it is dropped.
                self.heap.nodes = saved_nodes;
                self.heap.statics = saved_statics;
                self.path.clear();
                self.skipped.push(format!(
                    "{owner}: incomplete (a branch depended on an unmodelled value)"
                ));
                Ok(Some(Value::Other))
            }
            Err(error) => {
                self.heap.nodes = saved_nodes;
                self.heap.statics = saved_statics;
                self.path.clear();
                self.skipped.push(format!("{owner}: {error}"));
                Ok(Some(Value::Other))
            }
        }
    }

    fn run(&mut self, class: &ClassFile, member: &Member, args: Vec<Value>) -> Result<Value, String> {
        self.heap.depth += 1;
        if self.heap.depth > 48 {
            let tail: Vec<String> = self
                .trace
                .iter()
                .rev()
                .take(14)
                .cloned()
                .collect();
            self.heap.depth -= 1;
            return Err(format!("recursion limit at {}", tail.join(" <- ")));
        }
        self.trace.push(format!("{}.{}", class.name, member.name));
        let result = self.run_inner(class, member, args);
        self.trace.pop();
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
        let mut locals: Vec<Value> = vec![Value::Other; member_locals(member)];
        for (slot, value) in args.into_iter().enumerate() {
            if slot < locals.len() {
                locals[slot] = value;
            }
        }
        let mut stack: Vec<Value> = Vec::new();
        let mut at = 0usize;
        while at < code.len() {
            self.heap.steps += 1;
            if self.heap.steps > 20_000_000 {
                return Err("step limit".into());
            }
            let opcode = code[at];
            let length = instruction_len(&code, at)?;
            let mut jumped = false;
            match opcode {
                0x01 => stack.push(Value::Null),
                0x02 => stack.push(Value::Int(-1)),
                0x03..=0x08 => stack.push(Value::Int(opcode as i64 - 3)),
                0x09 | 0x0a => stack.push(Value::Other),
                0x0b => stack.push(Value::Float(0.0)),
                0x0c => stack.push(Value::Float(1.0)),
                0x0d => stack.push(Value::Float(2.0)),
                0x0e => stack.push(Value::Float(0.0)),
                0x0f => stack.push(Value::Float(1.0)),
                0x10 => stack.push(Value::Int(code[at + 1] as i8 as i64)),
                0x11 => stack.push(Value::Int(i16_at(&code, at + 1)? as i64)),
                0x12 => {
                    let index = code[at + 1] as usize;
                    stack.push(constant(class, index));
                }
                0x13 => {
                    let index = u16_at(&code, at + 1)?;
                    stack.push(constant(class, index));
                }
                0x14 => {
                    // `ldc2_w` always loads a `long`/`double`.
                    let index = u16_at(&code, at + 1)?;
                    stack.push(two_slot_constant(class, index));
                }
                0x15 => {
                    let index = code[at + 1] as usize;
                    stack.push(local(&locals, index));
                }
                0x16..=0x18 => stack.push(Value::Other),
                0x19 => {
                    let index = code[at + 1] as usize;
                    stack.push(local(&locals, index));
                }
                0x1a..=0x1d => stack.push(local(&locals, (opcode - 0x1a) as usize)),
                0x1e..=0x21 => stack.push(Value::Other),
                0x2a..=0x2d => stack.push(local(&locals, (opcode - 0x2a) as usize)),
                0x2e..=0x35 => {
                    let index = pop(&mut stack);
                    let array = pop(&mut stack);
                    let element = match self.heap.list(&array) {
                        Some(entries) => entries
                            .get(int_of(&index).max(0) as usize)
                            .cloned()
                            .unwrap_or(Value::Other),
                        None => {
                            self.heap.tainted = true;
                            Value::Other
                        }
                    };
                    stack.push(element);
                }
                0x36..=0x3a => {
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
                0x50..=0x56 => {
                    // aastore and primitive stores: array, index, value
                    let value = pop(&mut stack);
                    let index = pop(&mut stack);
                    let array = pop(&mut stack);
                    if let Value::List(slot) = array {
                        if let Some(entries) = self.heap.lists.get_mut(slot) {
                            let position = int_of(&index).max(0) as usize;
                            if let Some(slot) = entries.get_mut(position) {
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
                    pop(&mut stack);
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
                        (a, b) if matches!(a, _) && matches!(b, _) => match opcode {
                            0x60 => Value::Int(a.wrapping_add(b)),
                            0x64 => Value::Int(a.wrapping_sub(b)),
                            0x68 => Value::Int(a.wrapping_mul(b)),
                            0x6c if b != 0 => Value::Int(a.wrapping_div(b)),
                            0x70 if b != 0 => Value::Int(a.wrapping_rem(b)),
                            0x7e => Value::Int(a & b),
                            0x80 => Value::Int(a | b),
                            0x82 => Value::Int(a ^ b),
                            0x78 => Value::Int(a.wrapping_shl(b as u32)),
                            0x7a => Value::Int(a.wrapping_shr(b as u32)),
                            0x7c => Value::Int(((a as u64).wrapping_shr(b as u32)) as i64),
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
                    let a = pop(&mut stack);
                    let b = if (0x94..=0x98).contains(&opcode) {
                        pop(&mut stack)
                    } else {
                        Value::Other
                    };
                    let value = match (opcode, int_of(&a), int_of(&b)) {
                        (0x85, _, _) => Value::Other,
                        (0x88, a, _) => Value::Int(a as i32 as i64),
                        (0x91, a, _) => Value::Int(a as i8 as i64),
                        (0x92, a, _) => Value::Int(a as i16 as i64),
                        (0x93, a, _) => Value::Int(a as i32 as i64),
                        (0x94, a, b) => Value::Int(if a == b { 0 } else if a < b { -1 } else { 1 }),
                        (0x95, a, b) => Value::Int(if a < b { -1 } else if a == b { 0 } else { 1 }),
                        (0x96, a, b) => Value::Int(if a > b { 1 } else if a == b { 0 } else { -1 }),
                        _ => Value::Other,
                    };
                    stack.push(value);
                }
                0x99..=0x9e => {
                    let value = pop(&mut stack);
                    if matches!(value, Value::Other) {
                        self.heap.tainted = true;
                    }
                    let taken = match opcode {
                        0x99 => int_of(&value) == 0,
                        0x9a => int_of(&value) != 0,
                        0x9b => int_of(&value) < 0,
                        0x9c => int_of(&value) >= 0,
                        0x9d => int_of(&value) > 0,
                        _ => int_of(&value) <= 0,
                    };
                    if taken {
                        at = jump(&code, at, 1)?;
                        jumped = true;
                    }
                }
                0x9f..=0xa4 => {
                    let b = pop(&mut stack);
                    let a = pop(&mut stack);
                    if matches!(a, Value::Other) || matches!(b, Value::Other) {
                        self.heap.tainted = true;
                    }
                    let taken = match opcode {
                        0x9f => a == b,
                        0xa0 => a != b,
                        0xa1 => int_of(&a) < int_of(&b),
                        0xa2 => int_of(&a) >= int_of(&b),
                        0xa3 => int_of(&a) > int_of(&b),
                        _ => int_of(&a) <= int_of(&b),
                    };
                    if taken {
                        at = jump(&code, at, 1)?;
                        jumped = true;
                    }
                }
                0xa5 | 0xa6 => {
                    let b = pop(&mut stack);
                    let a = pop(&mut stack);
                    if matches!(a, Value::Other) || matches!(b, Value::Other) {
                        self.heap.tainted = true;
                    }
                    let taken = if opcode == 0xa5 { a == b } else { a != b };
                    if taken {
                        at = jump(&code, at, 1)?;
                        jumped = true;
                    }
                }
                0xa7 => {
                    at = jump(&code, at, 1)?;
                    jumped = true;
                }
                0xaa => {
                    let selector = pop(&mut stack);
                    if matches!(selector, Value::Other) {
                        self.heap.tainted = true;
                    }
                    let index = int_of(&selector);
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
                    let selector = pop(&mut stack);
                    if matches!(selector, Value::Other) {
                        self.heap.tainted = true;
                    }
                    let index = int_of(&selector);
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
                0xac..=0xaf => return Ok(pop(&mut stack)),
                0xb0 => return Ok(pop(&mut stack)),
                0xb1 => return Ok(Value::Other),
                0xb2 => {
                    let index = u16_at(&code, at + 1)?;
                    let value = self.static_field(class, index);
                    stack.push(value);
                }
                0xb3 => {
                    let index = u16_at(&code, at + 1)?;
                    let value = pop(&mut stack);
                    if let Some((owner, name, _)) = classfile::field_ref(&class.constants, index) {
                        self.heap.statics.insert((owner, name), value);
                    }
                }
                0xb4 => {
                    let object = pop(&mut stack);
                    let index = u16_at(&code, at + 1)?;
                    let field = classfile::field_ref(&class.constants, index)
                        .map(|(_, name, _)| name)
                        .unwrap_or_default();
                    let value = match (&object, field.as_str()) {
                        (Value::Selection { dedicated, .. }, "includeDedicated") => {
                            Value::Int(*dedicated as i64)
                        }
                        (Value::Selection { integrated, .. }, "includeIntegrated") => {
                            Value::Int(*integrated as i64)
                        }
                        _ => self.heap.get(&object, &field),
                    };
                    stack.push(value);
                }
                0xb5 => {
                    let value = pop(&mut stack);
                    let object = pop(&mut stack);
                    if let Some((_, name, _)) = classfile::field_ref(&class.constants, u16_at(&code, at + 1)?)
                    {
                        self.heap.set(&object, &name, value);
                    }
                }
                0xb6 | 0xb7 | 0xb8 | 0xb9 => {
                    let index = u16_at(&code, at + 1)?;
                    let Some((owner, name, descriptor)) =
                        classfile::method_ref(&class.constants, index)
                    else {
                        return Err(format!("no method ref at {at}"));
                    };
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
                    let virtual_call = opcode == 0xb6 || opcode == 0xb9;
                    let result = self.call(
                        class,
                        &owner,
                        &name,
                        &descriptor,
                        receiver,
                        arguments,
                        virtual_call,
                    )?;
                    if returns_value(&descriptor) {
                        stack.push(result.unwrap_or(Value::Other));
                    }
                }
                0xba => {
                    let index = u16_at(&code, at + 1)?;
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
                    let value = self.heap.new_object(&name);
                    stack.push(value);
                }
                0xbc | 0xbd => {
                    let count = int_of(&pop(&mut stack)).max(0) as usize;
                    let value = self.heap.new_list(vec![Value::Null; count]);
                    stack.push(value);
                }
                0xbe => {
                    let array = pop(&mut stack);
                    let length = match self.heap.list(&array) {
                        Some(list) => list.len() as i64,
                        None => {
                            self.heap.tainted = true;
                            0
                        }
                    };
                    stack.push(Value::Int(length));
                }
                0xbf => {
                    return Err(format!(
                        "exception thrown [{}]",
                        self.trace.iter().rev().take(8).cloned().collect::<Vec<_>>().join(" <- ")
                    ))
                }
                0xc0 => {}
                0xc1 => {
                    pop(&mut stack);
                    stack.push(Value::Int(0));
                }
                0xc2 | 0xc3 => {
                    pop(&mut stack);
                }
                0xc4 => {
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
                    let dimensions = int_of(&pop(&mut stack)).max(0) as usize;
                    let value = self.heap.new_list(vec![Value::Null; dimensions]);
                    stack.push(value);
                }
                0xc6 | 0xc7 => {
                    let value = pop(&mut stack);
                    if matches!(value, Value::Other) {
                        self.heap.tainted = true;
                    }
                    let taken = match opcode {
                        0xc6 => matches!(value, Value::Null),
                        _ => !matches!(value, Value::Null),
                    };
                    if taken {
                        at = jump(&code, at, 1)?;
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

    fn static_field(&mut self, class: &ClassFile, index: usize) -> Value {
        let Some((owner, name, _)) = classfile::field_ref(&class.constants, index) else {
            return Value::Other;
        };
        if let Err(_error) = self.init_class(&owner) {
            return Value::Other;
        }
        if let Some(value) = self.heap.statics.get(&(owner.clone(), name.clone())) {
            return value.clone();
        }
        // Constants the tree needs that are not worth initialising a whole class for.
        match owner.as_str() {
            COMMANDS => match name.as_str() {
                "LEVEL_ALL" => Value::Requirement(None),
                "LEVEL_MODERATORS" => Value::Requirement(Some("moderators".into())),
                "LEVEL_GAMEMASTERS" => Value::Requirement(Some("gamemasters".into())),
                "LEVEL_ADMINS" => Value::Requirement(Some("admins".into())),
                "LEVEL_OWNERS" => Value::Requirement(Some("owners".into())),
                _ => Value::Other,
            },
            "net/minecraft/commands/Commands$CommandSelection" => match name.as_str() {
                "ALL" => Value::Selection {
                    dedicated: true,
                    integrated: true,
                },
                "DEDICATED" => Value::Selection {
                    dedicated: true,
                    integrated: false,
                },
                "INTEGRATED" => Value::Selection {
                    dedicated: false,
                    integrated: true,
                },
                _ => Value::Other,
            },
            // The datagen JVM sets no debug system properties and is not an IDE.
            "net/minecraft/SharedConstants" => match name.as_str() {
                "DEBUG_CHASE_COMMAND" | "DEBUG_DEV_COMMANDS" | "IS_RUNNING_IN_IDE"
                | "DEBUG_ENABLED" => Value::Int(0),
                _ => Value::Other,
            },
            PERMISSIONS => match name.as_str() {
                "COMMANDS_MODERATOR" => Value::Requirement(Some("moderators".into())),
                "COMMANDS_GAMEMASTER" => Value::Requirement(Some("gamemasters".into())),
                "COMMANDS_ADMIN" => Value::Requirement(Some("admins".into())),
                "COMMANDS_OWNER" => Value::Requirement(Some("owners".into())),
                _ => Value::Other,
            },
            _ => Value::Other,
        }
    }

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

    // -- calls --------------------------------------------------------------

    fn call(
        &mut self,
        _class: &ClassFile,
        owner: &str,
        name: &str,
        descriptor: &str,
        receiver: Option<Value>,
        arguments: Vec<Value>,
        virtual_call: bool,
    ) -> Result<Option<Value>, String> {
        let mut values = arguments;
        if let Some(receiver) = receiver {
            values.insert(0, receiver);
        }

        // `array.clone()` (what an enum's `values()` returns).
        if name == "clone" && owner.starts_with('[') {
            if let Some(entries) = values.first().and_then(|value| self.heap.list(value)).cloned() {
                return Ok(Some(self.heap.new_list(entries)));
            }
        }

        // A receiver of a modelled object: dispatch to its own class's method.
        // Only `invokevirtual`/`invokeinterface` do this; `invokespecial` (a
        // constructor's `super(...)`/`this(...)`) and static calls resolve on the
        // declared owner in the blocks below.
        if virtual_call {
            if let Some(Value::Object(slot)) = values.first() {
                let class_name = self
                    .heap
                    .objects
                    .get(*slot)
                    .map(|object| object.class.clone())
                    .unwrap_or_default();
                if interpretable_object(&class_name) && self.jar.has(&class_name) {
                    let target = self.jar.class(&class_name)?;
                    if let Some(member) = target
                        .methods
                        .iter()
                        .find(|member| member.name == name && member.descriptor == descriptor)
                    {
                        if member.code.is_some() {
                            self.init_class(&class_name)?;
                            return Ok(Some(self.run(&target, member, values)?));
                        }
                    }
                }
            }
        }

        // A lambda passed to a helper that then calls it cannot be run: whatever it
        // would have contributed is missing, so the registration is incomplete.
        if virtual_call && matches!(values.first(), Some(Value::Lambda(_))) {
            self.heap.tainted = true;
            return Ok(Some(Value::Other));
        }

        // ---- `Commands`: the factories every registration uses -------------
        if owner == COMMANDS {
            if name == "<init>" {
                let target = self.jar.class(COMMANDS)?;
                if let Some(member) = target
                    .methods
                    .iter()
                    .find(|member| member.name == "<init>" && member.descriptor == descriptor)
                {
                    return Ok(Some(self.run(&target, member, values)?));
                }
            }
            return Ok(match (name, values.len()) {
                ("literal", 1) => Some(self.literal_builder(&values[0])),
                ("argument", 2) => Some(self.argument_builder(&values[0], &values[1])),
                ("hasPermission", 1) => Some(match &values[0] {
                    Value::Requirement(level) => Value::Requirement(level.clone()),
                    Value::Object(slot) => {
                        let level = self
                            .heap
                            .objects
                            .get(*slot)
                            .and_then(|object| object.fields.get("level"))
                            .and_then(|level| match level {
                                Value::Requirement(level) => level.clone(),
                                _ => None,
                            });
                        Value::Requirement(level)
                    }
                    _ => {
                        self.heap.tainted = true;
                        Value::Other
                    }
                }),
                ("createValidationContext", _) => Some(Value::Other),
                _ => Some(Value::Other),
            });
        }

        // `PermissionCheck$Require(Permission)` carries the permission it requires.
        if owner.ends_with("PermissionCheck$Require") && name == "<init>" {
            if let (Some(Value::Object(slot)), Some(level)) = (values.first(), values.get(1)) {
                let level = level.clone();
                if let Some(object) = self.heap.objects.get_mut(*slot) {
                    object.fields.insert("level".to_string(), level);
                }
            }
            return Ok(None);
        }

        // ---- Brigadier builders -------------------------------------------
        if owner == LITERAL_BUILDER {
            if name == "literal" {
                return Ok(Some(self.literal_builder(values.first().unwrap_or(&Value::Other))));
            }
            return self.builder_call(name, &values);
        }
        if owner == REQUIRED_BUILDER {
            if name == "argument" && values.len() >= 2 {
                return Ok(Some(self.argument_builder(&values[0], &values[1])));
            }
            return self.builder_call(name, &values);
        }
        if owner == "com/mojang/brigadier/builder/ArgumentBuilder" {
            return self.builder_call(name, &values);
        }

        // ---- Brigadier dispatcher and nodes -------------------------------
        if owner == DISPATCHER {
            return Ok(match name {
                "<init>" => {
                    if !self.heap.nodes.iter().any(|node| matches!(node.kind, NodeKind::Root)) {
                        self.heap.new_node(Node {
                            kind: NodeKind::Root,
                            children: Vec::new(),
                            command: false,
                            permission: None,
                            redirect: None,
                        });
                    }
                    if let Some(target) = values.first() {
                        self.heap.set(target, "root", Value::Node(self.heap.root().unwrap_or(0)));
                    }
                    None
                }
                "register" => {
                    let dispatcher = values.first().cloned().unwrap_or(Value::Other);
                    let builder = values.get(1).cloned().unwrap_or(Value::Other);
                    let root = self
                        .heap
                        .fields(&dispatcher)
                        .and_then(|fields| match fields.get("root") {
                            Some(Value::Node(id)) => Some(*id),
                            _ => None,
                        })
                        .or_else(|| self.heap.root())
                        .ok_or("no root node")?;
                    let node = self.build(&builder)?;
                    self.add_child(root, node);
                    Some(Value::Node(node))
                }
                "getRoot" => {
                    let dispatcher = values.first().cloned().unwrap_or(Value::Other);
                    let root = self
                        .heap
                        .fields(&dispatcher)
                        .and_then(|fields| match fields.get("root") {
                            Some(Value::Node(id)) => Some(*id),
                            _ => None,
                        })
                        .or_else(|| self.heap.root());
                    Some(root.map(Value::Node).unwrap_or(Value::Other))
                }
                _ => Some(Value::Other),
            });
        }
        if owner.starts_with("com/mojang/brigadier/tree/") {
            let node = values.first().cloned().unwrap_or(Value::Other);
            return Ok(match name {
                "getChild" => {
                    let wanted = values.get(1).and_then(str_of);
                    let child = match (&node, wanted) {
                        (Value::Node(id), Some(wanted)) => self
                            .heap
                            .nodes
                            .get(*id)
                            .and_then(|node| {
                                node.children
                                    .iter()
                                    .find(|(name, _)| name == &wanted)
                                    .map(|(_, id)| *id)
                            }),
                        _ => None,
                    };
                    Some(child.map(Value::Node).unwrap_or(Value::Null))
                }
                "getChildren" | "getName" | "getCommand" | "getRequirement" => {
                    Some(match (&node, name) {
                        (Value::Node(id), "getName") => self
                            .heap
                            .nodes
                            .get(*id)
                            .map(|node| Value::Str(node.name()))
                            .unwrap_or(Value::Other),
                        _ => Value::Other,
                    })
                }
                "getRedirect" => {
                    let target = match &node {
                        Value::Node(id) => self
                            .heap
                            .nodes
                            .get(*id)
                            .and_then(|node| node.redirect)
                            .map(Value::Node),
                        _ => None,
                    };
                    Some(target.unwrap_or(Value::Null))
                }
                _ => Some(Value::Other),
            });
        }

        // ---- argument type factories --------------------------------------
        if is_argument_factory(owner) {
            if let Some(value) = self.argument_factory(owner, name, &values, descriptor) {
                return Ok(Some(value));
            }
        }

        // ---- other game classes the registrations call --------------------
        if interpretable_object(owner) && self.jar.has(owner) && !skip_initialisation(owner) {
            let target = self.jar.class(owner)?;
            if let Some(member) = target
                .methods
                .iter()
                .find(|member| member.name == name && member.descriptor == descriptor)
            {
                if member.code.is_some() {
                    self.init_class(owner)?;
                    return self.run_maybe_transactional(owner, &target, member, values);
                }
            }
        }

        // ---- the JDK's JFR is present, so `jfr` registers ------------------
        if owner == "net/minecraft/util/profiling/jfr/JvmProfiler" && name == "isAvailable" {
            return Ok(Some(Value::Int(1)));
        }

        // ---- vanilla registry keys ----------------------------------------
        if owner == REGISTRIES && name == "createRegistryKey" {
            let id = values.first().and_then(str_of).map(|name| {
                if name.contains(':') {
                    name
                } else {
                    format!("minecraft:{name}")
                }
            });
            return Ok(Some(id.map(Value::Key).unwrap_or(Value::Other)));
        }

        // ---- a helper we can execute --------------------------------------
        if interpretable(owner) {
            if self.jar.has(owner) {
                let target = self.jar.class(owner)?;
                if let Some(member) = target
                    .methods
                    .iter()
                    .find(|member| member.name == name && member.descriptor == descriptor)
                {
                    if member.code.is_some() {
                        return self.run_maybe_transactional(owner, &target, member, values);
                    }
                }
            }
        }

        Ok(Some(Value::Other))
    }

    fn literal_builder(&mut self, name: &Value) -> Value {
        let value = self.heap.new_object(LITERAL_BUILDER);
        self.init_builder(&value);
        self.heap.set(&value, "literal", Value::Str(str_of(name).unwrap_or_default()));
        value
    }

    fn argument_builder(&mut self, name: &Value, argument: &Value) -> Value {
        let value = self.heap.new_object(REQUIRED_BUILDER);
        self.init_builder(&value);
        self.heap
            .set(&value, "name", Value::Str(str_of(name).unwrap_or_default()));
        // An argument object from a factory we did not model still knows its class,
        // and the class is enough to name its parser.
        let argument = match argument.clone() {
            Value::Object(_) => match self
                .heap
                .object(argument)
                .map(|object| object.class.clone())
                .and_then(|class| self.argument_ids.get(&class).cloned())
            {
                Some(parser) => self.heap.new_argument(parser, None),
                None => argument.clone(),
            },
            other => other,
        };
        self.heap.set(&value, "argument", argument);
        value
    }

    fn init_builder(&mut self, value: &Value) {
        let children = self.heap.new_list(Vec::new());
        self.heap.set(value, "children", children);
        self.heap.set(value, "command", Value::Null);
        self.heap.set(value, "requirement", Value::Null);
        self.heap.set(value, "redirect", Value::Null);
    }

    fn builder_call(&mut self, name: &str, values: &[Value]) -> Result<Option<Value>, String> {
        let this = values.first().cloned().unwrap_or(Value::Other);
        Ok(match name {
            "<init>" => {
                if self.heap.object(&this).is_some() {
                    self.init_builder(&this);
                }
                None
            }
            "then" => {
                let child = values.get(1).cloned().unwrap_or(Value::Other);
                let children = self.heap.get(&this, "children");
                if let Value::List(slot) = children {
                    if let Some(entries) = self.heap.lists.get_mut(slot) {
                        entries.push(child);
                    }
                }
                Some(this)
            }
            "executes" => {
                self.heap.set(&this, "command", Value::Int(1));
                Some(this)
            }
            "requires" => {
                let requirement = values.get(1).cloned().unwrap_or(Value::Null);
                self.heap.set(&this, "requirement", requirement);
                Some(this)
            }
            "suggests" | "fork" | "forward" => Some(this),
            "redirect" => {
                let target = values.get(1).cloned().unwrap_or(Value::Null);
                self.heap.set(&this, "redirect", target);
                Some(this)
            }
            "getRedirect" => Some(self.heap.get(&this, "redirect")),
            "getChildren" => Some(self.heap.get(&this, "children")),
            "getName" => {
                let literal = self
                    .heap
                    .object(&this)
                    .is_some_and(|object| object.class == LITERAL_BUILDER);
                let field = if literal { "literal" } else { "name" };
                Some(self.heap.get(&this, field))
            }
            "getCommand" => {
                let command = self.heap.get(&this, "command");
                Some(if matches!(command, Value::Null) {
                    Value::Null
                } else {
                    Value::Other
                })
            }
            "build" => Some(Value::Node(self.build(&this)?)),
            _ => Some(Value::Other),
        })
    }

    /// Brigadier's `ArgumentBuilder.build()`: the node, then every child built and
    /// merged into it.
    fn build(&mut self, builder: &Value) -> Result<usize, String> {
        self.heap.builds += 1;
        if self.heap.builds > 200_000 {
            return Err(format!("build limit (building {})", self.path.join("/")));
        }
        if self.path.len() > 64 {
            return Err(format!("builder nesting too deep ({})", self.path.join("/")));
        }
        let label = self.builder_label(builder);
        self.path.push(label);
        let result = self.build_here(builder);
        self.path.pop();
        result
    }

    fn builder_label(&self, value: &Value) -> String {
        match self.heap.object(value) {
            Some(object) => match object.class.as_str() {
                LITERAL_BUILDER => object
                    .fields
                    .get("literal")
                    .and_then(str_of)
                    .unwrap_or_default(),
                REQUIRED_BUILDER => object
                    .fields
                    .get("name")
                    .and_then(str_of)
                    .unwrap_or_default(),
                other => other.to_string(),
            },
            None => format!("{value:?}"),
        }
    }

    fn build_here(&mut self, builder: &Value) -> Result<usize, String> {
        let Some(object) = self.heap.object(builder) else {
            return Err(format!(
                "not a builder: {builder:?} (building {}) [{}]",
                self.path.join("/"),
                self.trace.iter().rev().take(6).cloned().collect::<Vec<_>>().join(" <-")
            ));
        };
        let class = object.class.clone();
        let fields = object.fields.clone();
        let kind = if class == LITERAL_BUILDER {
            let literal = fields
                .get("literal")
                .and_then(str_of)
                .ok_or("literal builder without a name")?;
            NodeKind::Literal(literal)
        } else if class == REQUIRED_BUILDER {
            let argument = fields
                .get("argument")
                .cloned()
                .ok_or("argument builder without a type")?;
            let Value::Argument(slot) = argument else {
                return Err(format!("unknown argument type: {argument:?}"));
            };
            let definition = self
                .heap
                .arguments
                .get(slot)
                .cloned()
                .ok_or("argument definition missing")?;
            NodeKind::Argument {
                name: fields
                    .get("name")
                    .and_then(str_of)
                    .unwrap_or_else(|| definition.parser.clone()),
                parser: definition.parser.clone(),
                properties: definition.properties.clone(),
            }
        } else {
            return Err(format!("not a Brigadier builder: {class}"));
        };
        let command = !matches!(fields.get("command"), Some(Value::Null) | None);
        let permission = match fields.get("requirement") {
            Some(Value::Requirement(level)) => Some(level.clone()),
            _ => None,
        };
        let redirect = match fields.get("redirect") {
            Some(Value::Node(id)) => Some(*id),
            _ => None,
        };
        let node = self.heap.new_node(Node {
            kind,
            children: Vec::new(),
            command,
            permission,
            redirect,
        });
        // Children are built and merged the way Brigadier's `addChild` does it.
        if let Some(children) = fields.get("children").and_then(|value| self.heap.list(value)) {
            for child in children.clone() {
                let child_node = self.build(&child)?;
                self.add_child(node, child_node);
            }
        }
        Ok(node)
    }

    /// `CommandNode.addChild`: an existing child of the same name is merged into.
    fn add_child(&mut self, parent: usize, child: usize) {
        self.merge_depth += 1;
        if self.merge_depth > 256 {
            self.merge_depth -= 1;
            return;
        }
        self.add_child_inner(parent, child);
        self.merge_depth -= 1;
    }

    fn add_child_inner(&mut self, parent: usize, child: usize) {
        let (name, child_children, child_command, child_redirect) = {
            let Some(node) = self.heap.nodes.get(child) else {
                return;
            };
            let name = match &node.kind {
                NodeKind::Literal(name) | NodeKind::Argument { name, .. } => name.clone(),
                NodeKind::Root => return,
            };
            (
                name,
                node.children.clone(),
                node.command,
                node.redirect,
            )
        };
        let existing = self
            .heap
            .nodes
            .get(parent)
            .and_then(|node| {
                node.children
                    .iter()
                    .find(|(existing, _)| existing == &name)
                    .map(|(_, id)| *id)
            });
        match existing {
            None => {
                if let Some(node) = self.heap.nodes.get_mut(parent) {
                    node.children.push((name, child));
                }
            }
            Some(existing) => {
                // `CommandNode.merge`: the incoming command and redirect win, the
                // children merge recursively, the requirement stays.
                if child_command {
                    if let Some(node) = self.heap.nodes.get_mut(existing) {
                        node.command = true;
                    }
                }
                if child_redirect.is_some() {
                    if let Some(node) = self.heap.nodes.get_mut(existing) {
                        node.redirect = child_redirect;
                    }
                }
                for (_, grandchild) in child_children {
                    self.add_child_inner(existing, grandchild);
                }
            }
        }
    }

    // -- argument types -----------------------------------------------------

    fn argument_factory(
        &mut self,
        owner: &str,
        name: &str,
        values: &[Value],
        descriptor: &str,
    ) -> Option<Value> {
        let class = return_class(descriptor).unwrap_or_else(|| owner.to_string());
        let parser = self
            .argument_ids
            .get(&class)
            .cloned()
            .or_else(|| self.argument_ids.get(owner).cloned())?;
        let simple = owner.rsplit('/').next().unwrap_or(owner);
        let properties = match (simple, name) {
            ("IntegerArgumentType", "integer") => {
                int_bounds(values, i32::MIN as i64, i32::MAX as i64)
            }
            ("FloatArgumentType", "floatArg") => float_bounds(values),
            ("DoubleArgumentType", "doubleArg") => double_bounds(values),
            ("StringArgumentType", "word") => Some(serde_json::json!({"type": "word"})),
            ("StringArgumentType", "phrase") => Some(serde_json::json!({"type": "phrase"})),
            ("StringArgumentType", "greedyString") => {
                Some(serde_json::json!({"type": "greedy"}))
            }
            ("StringArgumentType", "string") => Some(serde_json::json!({"type": "phrase"})),
            ("EntityArgument", "entity") => entity("entities", "single"),
            ("EntityArgument", "entities") => entity("entities", "multiple"),
            ("EntityArgument", "player") => entity("players", "single"),
            ("EntityArgument", "players") => entity("players", "multiple"),
            ("ScoreHolderArgument", "scoreHolder") => {
                Some(serde_json::json!({"amount": "single"}))
            }
            ("ScoreHolderArgument", "scoreHolders") => {
                Some(serde_json::json!({"amount": "multiple"}))
            }
            ("TimeArgument", "time") => {
                let min = values
                    .first()
                    .map(int_of)
                    .unwrap_or(0);
                Some(serde_json::json!({ "min": min }))
            }
            ("ResourceArgument", "resource")
            | ("ResourceKeyArgument", "key")
            | ("ResourceOrTagArgument", "resourceOrTag")
            | ("ResourceOrTagKeyArgument", "resourceOrTagKey")
            | ("ResourceSelectorArgument", "resourceSelector") => values
                .iter()
                .rev()
                .find_map(key_of)
                .map(|registry| serde_json::json!({ "registry": registry })),
            _ => None,
        };
        Some(self.heap.new_argument(parser, properties))
    }

    // -- evaluation of a whole registration ---------------------------------

}

fn int_bounds(values: &[Value], min_default: i64, max_default: i64) -> Option<Json> {
    let min = values.first().map(int_of).unwrap_or(min_default);
    let max = values.get(1).map(int_of).unwrap_or(max_default);
    let mut object = JsonMap::new();
    if min != min_default {
        object.insert("min".into(), Json::from(min));
    }
    if max != max_default {
        object.insert("max".into(), Json::from(max));
    }
    if object.is_empty() {
        None
    } else {
        Some(Json::Object(object))
    }
}

/// `FloatArgumentType` serialises the values as Java floats, so the numbers are the
/// shortest decimal that round-trips through `float`, not the exact `f32` value.
fn float_bounds(values: &[Value]) -> Option<Json> {
    let min = values.first().map(|value| float_of(value) as f32).unwrap_or(f32::MAX);
    let max = values.get(1).map(|value| float_of(value) as f32).unwrap_or(f32::MAX);
    let mut object = JsonMap::new();
    if values.first().is_some() && min != -f32::MAX {
        object.insert("min".into(), float_number(min));
    }
    if values.get(1).is_some() && max != f32::MAX {
        object.insert("max".into(), float_number(max));
    }
    if object.is_empty() {
        None
    } else {
        Some(Json::Object(object))
    }
}

fn double_bounds(values: &[Value]) -> Option<Json> {
    let mut object = JsonMap::new();
    if let Some(value) = values.first() {
        if float_of(value) != -f64::MAX {
            object.insert("min".into(), number(float_of(value)));
        }
    }
    if let Some(value) = values.get(1) {
        if float_of(value) != f64::MAX {
            object.insert("max".into(), number(float_of(value)));
        }
    }
    if object.is_empty() {
        None
    } else {
        Some(Json::Object(object))
    }
}

/// A float as Java's `Float.toString` writes it, parsed back as the JSON number.
fn float_number(value: f32) -> Json {
    match value.to_string().parse::<f64>() {
        Ok(number_value) => number(number_value),
        Err(_) => number(value as f64),
    }
}

fn number(value: f64) -> Json {
    serde_json::Number::from_f64(value)
        .map(Json::Number)
        .unwrap_or(Json::Null)
}

fn entity(kind: &str, amount: &str) -> Option<Json> {
    Some(serde_json::json!({ "type": kind, "amount": amount }))
}

/// A `long`/`double` constant, wherever the class reader put it.
///
/// `hzr-bytecode`'s reader reserves the second constant-pool slot after it has
/// already written the value, so a two-slot constant lands one index late; read
/// both slots so the operand index resolves either way.
fn two_slot_constant(class: &ClassFile, index: usize) -> Value {
    for candidate in [index, index.saturating_add(1), index.saturating_sub(1)] {
        match class.constants.get(candidate) {
            Some(classfile::Constant::Double(number)) => return Value::Float(*number),
            Some(classfile::Constant::Long(number)) => return Value::Int(*number),
            _ => {}
        }
    }
    constant(class, index)
}

fn constant(class: &ClassFile, index: usize) -> Value {
    match class.constants.get(index) {
        Some(classfile::Constant::Integer(number)) => Value::Int(*number as i64),
        Some(classfile::Constant::Float(number)) => Value::Float(*number as f64),
        Some(classfile::Constant::Long(number)) => Value::Int(*number),
        Some(classfile::Constant::Double(number)) => Value::Float(*number),
        Some(classfile::Constant::Class(reference)) => {
            let inner: usize = reference.trim_start_matches('#').parse().unwrap_or(0);
            Value::Str(classfile::utf8(&class.constants, inner))
        }
        Some(classfile::Constant::String(_)) | Some(classfile::Constant::Utf8(_)) => {
            match classfile::string_constant(&class.constants, index) {
                Some(text) => Value::Str(text),
                None => Value::Other,
            }
        }
        _ => Value::Other,
    }
}

fn invokedynamic_descriptor(class: &ClassFile, index: usize) -> Option<String> {
    let Some(classfile::Constant::InvokeDynamic { name, .. }) = class.constants.get(index) else {
        return None;
    };
    let name_and_type: usize = name.trim_start_matches('#').parse().ok()?;
    let classfile::Constant::NameAndType { kind, .. } = class.constants.get(name_and_type)? else {
        return None;
    };
    let descriptor_index: usize = kind.trim_start_matches('#').parse().ok()?;
    match class.constants.get(descriptor_index) {
        Some(classfile::Constant::Utf8(descriptor)) => Some(descriptor.clone()),
        _ => None,
    }
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

fn jump(code: &[u8], at: usize, operand: usize) -> Result<usize, String> {
    let offset = i16_at(code, at + operand)?;
    Ok((at as i64 + offset as i64) as usize)
}

/// Classes whose own methods the evaluator executes (the command registrations
/// and the helpers they call while building their trees).
/// Heavy bootstrap classes: their static initialisers build the whole game and
/// would either loop or need facilities the evaluator does not model. Everything
/// else a registration touches is initialised on demand, as the JVM does.
fn skip_initialisation(name: &str) -> bool {
    name == "net/minecraft/SharedConstants"
        || name == COMMANDS
        || name.starts_with("net/minecraft/core/registries/BuiltInRegistries")
        || name == REGISTRIES
        || name.starts_with("net/minecraft/server/Bootstrap")
        || name.starts_with("net/minecraft/util/datafix")
        || name.starts_with("net/minecraft/world/level/block/Blocks")
        || name.starts_with("net/minecraft/world/item/Items")
        || name.starts_with("net/minecraft/core/component/DataComponents")
        || name.starts_with("net/minecraft/server/ReloadableServerResources")
        || name.starts_with("net/minecraft/world/level/gamerules/GameRules")
        || name.starts_with("net/minecraft/util/profiling/jfr/")
        || name.starts_with("net/minecraft/commands/synchronization/")
        || name.starts_with("net/minecraft/server/permissions/Permissions")
}

/// Object classes whose methods the evaluator runs when they are called on a
/// modelled value (enums and small value classes the registrations read).
fn interpretable_object(class: &str) -> bool {
    if class.starts_with("net/minecraft/commands/synchronization/")
        || class.starts_with("net/minecraft/util/profiling/jfr/")
        || class.starts_with("net/minecraft/util/ByIdMap")
    {
        return false;
    }
    class.starts_with("net/minecraft/world/")
        || class.starts_with("net/minecraft/sounds/")
        || class.starts_with("net/minecraft/server/")
        || class.starts_with("net/minecraft/commands/")
        || class.starts_with("net/minecraft/util/")
        || class.starts_with("net/minecraft/core/")
        || class.starts_with("net/minecraft/advancements/")
        || class.starts_with("net/minecraft/gametest/")
}


fn interpretable(owner: &str) -> bool {
    owner.starts_with("net/minecraft/server/commands/")
        || owner.starts_with("net/minecraft/gametest/framework/")
        || owner.starts_with("net/minecraft/commands/arguments/")
}

/// Classes whose static methods are argument-type factories.
fn is_argument_factory(owner: &str) -> bool {
    owner.starts_with("com/mojang/brigadier/arguments/")
        || owner.starts_with("net/minecraft/commands/arguments/")
}

// ---------------------------------------------------------------------------
// The table
// ---------------------------------------------------------------------------

/// The `ArgumentTypeInfos.bootstrap(registry)` registrations, as
/// `internal class name -> parser id`.
///
/// The method is a straight line of `ldc "id"; ldc Class; …; invokestatic
/// register(registry, id, class, info)` calls, so the pairs can be read off the
/// constant stream without running the registry.
fn argument_ids(jar: &Jar) -> Result<BTreeMap<String, String>, String> {
    let class = jar.class(ARGUMENT_INFOS)?;
    let Some(member) = class.method("bootstrap") else {
        return Err("ArgumentTypeInfos.bootstrap".into());
    };
    let Some(code) = member.code.as_ref() else {
        return Err("ArgumentTypeInfos.bootstrap has no code".into());
    };
    let bytes = &code.bytes;
    let mut out = BTreeMap::new();
    let mut last_string: Option<String> = None;
    let mut last_class: Option<String> = None;
    let mut at = 0usize;
    while at < bytes.len() {
        let opcode = bytes[at];
        let length = instruction_len(bytes, at)?;
        match opcode {
            0x12 | 0x13 => {
                let index = if opcode == 0x12 {
                    bytes[at + 1] as usize
                } else {
                    u16_at(bytes, at + 1)?
                };
                match class.constants.get(index) {
                    Some(classfile::Constant::Class(reference)) => {
                        let inner: usize = reference.trim_start_matches('#').parse().unwrap_or(0);
                        let name = classfile::utf8(&class.constants, inner);
                        if name.starts_with("com/mojang/brigadier/arguments/")
                            || name.starts_with("net/minecraft/commands/arguments/")
                        {
                            last_class = Some(name);
                        }
                    }
                    _ => {
                        if let Some(text) = classfile::string_constant(&class.constants, index) {
                            last_string = Some(text);
                        }
                    }
                }
            }
            0xb8 => {
                let index = u16_at(bytes, at + 1)?;
                if let Some((owner, name, _)) = classfile::method_ref(&class.constants, index) {
                    if owner == class.name && name == "register" {
                        if let (Some(id), Some(clazz)) = (last_string.take(), last_class.take()) {
                            let id = if id.contains(':') {
                                id
                            } else {
                                format!("minecraft:{id}")
                            };
                            out.insert(clazz, id);
                        }
                    }
                }
            }
            _ => {}
        }
        at += length;
    }
    Ok(out)
}

/// The `commands.json` document this jar can reproduce, plus what it could not.
pub fn commands_data(jar_path: &Path) -> Result<(Json, Coverage), String> {
    let jar = Jar::open(jar_path)?;
    let mut machine = Machine::new(&jar);
    machine.argument_ids = argument_ids(&jar)?;
    let argument_type_count = machine.argument_ids.len();

    // `Registries`' own constants (the `ResourceKey`s argument types carry).
    let registries = jar.class(REGISTRIES)?;
    if let Some(initializer) = registries.method("<clinit>").cloned() {
        machine.run(&registries, &initializer, Vec::new())?;
    }

    // `new Commands(ALL, buildContext)` is what `CommandsReport` does; the
    // constructor is where the 97 registrations (and their conditions) live.
    eprintln!("phase: constructor");
    let commands_class = jar.class(COMMANDS)?;
    let initializer = commands_class
        .method("<init>")
        .cloned()
        .ok_or("Commands.<init>")?;
    let instance = machine.heap.new_object(COMMANDS);
    machine.run(
        &commands_class,
        &initializer,
        vec![
            instance.clone(),
            Value::Selection {
                dedicated: true,
                integrated: true,
            },
            Value::Other,
        ],
    )?;
    eprintln!("phase: constructor done");
    let dispatcher = machine.heap.get(&instance, "dispatcher");
    let root = match &dispatcher {
        Value::Object(_) => match machine.heap.get(&dispatcher, "root") {
            Value::Node(root) => root,
            _ => return Err("Commands.<init> left no dispatcher root".into()),
        },
        _ => return Err("Commands.<init> left no dispatcher".into()),
    };
    let mut coverage = Coverage::default();
    coverage.skipped = machine.skipped.clone();
    eprintln!("phase: serialize");
    let document = machine.serialize(root);
    let children = document
        .get("children")
        .and_then(Json::as_object)
        .map(|children| children.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    coverage.produced = children;
    coverage.note(format!("{argument_type_count} argument types mapped"));
    coverage.note(format!("{} nodes", machine.heap.nodes.len()));
    coverage.note(format!("{} bytecode instructions", machine.heap.steps));
    Ok((document, coverage))
}

// ---------------------------------------------------------------------------
// Serialization (`ArgumentUtils.serializeNodeToJson`)
// ---------------------------------------------------------------------------

impl<'a> Machine<'a> {
    fn serialize(&self, root: usize) -> Json {
        self.node_json(root)
    }

    fn node_json(&self, id: usize) -> Json {
        let Some(node) = self.heap.nodes.get(id) else {
            return Json::Null;
        };
        let mut object = JsonMap::new();
        match &node.kind {
            NodeKind::Root => {
                object.insert("type".into(), Json::String("root".into()));
            }
            NodeKind::Literal(_) => {
                object.insert("type".into(), Json::String("literal".into()));
            }
            NodeKind::Argument { parser, properties, .. } => {
                object.insert("type".into(), Json::String("argument".into()));
                object.insert("parser".into(), Json::String(parser.clone()));
                if let Some(properties) = properties {
                    object.insert("properties".into(), properties.clone());
                }
            }
        }
        if !node.children.is_empty() {
            let mut children = JsonMap::new();
            for (name, child) in &node.children {
                children.insert(name.clone(), self.node_json(*child));
            }
            object.insert("children".into(), Json::Object(children));
        }
        if node.command {
            object.insert("executable".into(), Json::Bool(true));
        }
        if let Some(Some(level)) = &node.permission {
            object.insert(
                "permissions".into(),
                serde_json::json!({
                    "type": "minecraft:require",
                    "permission": { "type": "minecraft:command_level", "level": level },
                }),
            );
        }
        if let Some(redirect) = node.redirect {
            if let Some(path) = self.path_to(redirect) {
                object.insert("redirect".into(), Json::Array(
                    path.into_iter().map(Json::String).collect(),
                ));
            }
        }
        Json::Object(object)
    }

    /// The names from the root down to `target` (Brigadier's `getPath`).
    fn path_to(&self, target: usize) -> Option<Vec<String>> {
        let root = self.heap.root()?;
        let mut path = Vec::new();
        if self.search_path(root, target, &mut path) {
            Some(path)
        } else {
            None
        }
    }

    fn search_path(&self, node: usize, target: usize, path: &mut Vec<String>) -> bool {
        if node == target {
            return true;
        }
        if path.len() > 64 {
            return false;
        }
        let Some(children) = self.heap.nodes.get(node).map(|node| node.children.clone()) else {
            return false;
        };
        for (name, child) in children {
            path.push(name);
            if self.search_path(child, target, path) {
                return true;
            }
            path.pop();
        }
        false
    }
}

/// Write [`TABLE`] into `out_dir` and return what was produced.
pub fn write(jar_path: &Path, out_dir: &Path) -> Result<Coverage, String> {
    let (document, coverage) = commands_data(jar_path)?;
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
    fn a_literal_builder_becomes_a_literal_node() {
        let jar_path = std::env::var_os("HZR_CLIENT_JAR");
        let Some(jar_path) = jar_path else {
            return;
        };
        let jar = Jar::open(Path::new(&jar_path)).unwrap();
        let mut machine = Machine::new(&jar);
        let builder = machine.literal_builder(&Value::Str("hello".into()));
        machine.heap.set(&builder, "command", Value::Int(1));
        let node = machine.build(&builder).unwrap();
        let json = machine.node_json(node);
        assert_eq!(json["type"], "literal");
        assert_eq!(json["executable"], true);
    }

    #[test]
    fn integer_bounds_omit_the_defaults() {
        assert!(int_bounds(&[], i32::MIN as i64, i32::MAX as i64).is_none());
        let bounds = int_bounds(&[Value::Int(0)], i32::MIN as i64, i32::MAX as i64).unwrap();
        assert_eq!(bounds, serde_json::json!({"min": 0}));
        let both = int_bounds(&[Value::Int(1), Value::Int(99)], i32::MIN as i64, i32::MAX as i64)
            .unwrap();
        assert_eq!(both, serde_json::json!({"min": 1, "max": 99}));
    }
}
