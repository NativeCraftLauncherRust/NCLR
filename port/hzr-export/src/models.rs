//! Rebuild `entity-models.json` from the player's own `client.jar`: evaluate
//! `LayerDefinitions.createRoots()` (and the model classes it names) and bake
//! every `LayerDefinition` into the part/polygon tree the Java exporter writes.
//!
//! # What has to be evaluated
//!
//! * `LayerDefinitions.createRoots()` — a straight-line method (plus two
//!   `tableswitch`es) that builds ~165 model meshes into a map keyed by
//!   `ModelLayerLocation`.
//! * The model classes' `createBodyLayer`/`createMesh`/`createArmorMeshSet`
//!   static methods and their `invokedynamic` lambdas — pure builder code:
//!   `MeshDefinition`, `PartDefinition.addOrReplaceChild`, `CubeListBuilder`,
//!   `PartPose`, `CubeDeformation`, `LayerDefinition`, `ArmorModelSet`.
//! * `ModelLayers.<clinit>` — the `ModelLayerLocation` constants (and their
//!   duplicate check).
//! * The bake: `LayerDefinition.bakeRoot()` → `PartDefinition.bake`, i.e. the
//!   part poses, and `CubeDefinition.bake` → `ModelPart.Cube`'s polygon/UV
//!   generation (ported from `ModelPart$Cube`/`ModelPart$Polygon` bytecode).
//!
//! The module is included by path from `tests/models.rs`, which supplies
//! `crate::classfile` (the reader shared with `hzr-bytecode`), matching how
//! `src/main.rs` declares it.

#![allow(dead_code)]

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use std::rc::Rc;

use serde_json::{Map, Value};

use crate::classfile::{self, ClassFile, Constant};

/// `MeshTransformer.scaling`'s constant, from its bytecode (`ldc 24.016f`).
const SCALING_BASE: f32 = 24.016;

fn blocked_tables() -> &'static [(&'static str, &'static str)] {
    &[]
}

// ---------------------------------------------------------------------------
// jar access
// ---------------------------------------------------------------------------

struct JarFiles {
    files: BTreeMap<String, Vec<u8>>,
    classes: RefCell<BTreeMap<String, Option<Rc<ClassFile>>>>,
}

impl JarFiles {
    fn open(path: &Path) -> Result<Self, String> {
        let file = fs::File::open(path).map_err(|error| format!("{}: {error}", path.display()))?;
        let mut archive =
            zip::ZipArchive::new(file).map_err(|error| format!("{}: {error}", path.display()))?;
        let mut files = BTreeMap::new();
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index).map_err(|error| error.to_string())?;
            if entry.is_dir() {
                continue;
            }
            let name = entry.name().map_err(|error| error.to_string())?.to_string();
            let mut data = Vec::new();
            use std::io::Read;
            entry
                .read_to_end(&mut data)
                .map_err(|error| format!("{name}: {error}"))?;
            files.insert(name, data);
        }
        Ok(Self {
            files,
            classes: RefCell::new(BTreeMap::new()),
        })
    }

    fn class(&self, internal: &str) -> Option<Rc<ClassFile>> {
        if let Some(hit) = self.classes.borrow().get(internal) {
            return hit.clone();
        }
        let parsed = self
            .files
            .get(&format!("{internal}.class"))
            .and_then(|bytes| classfile::parse(bytes).ok())
            .map(Rc::new);
        self.classes
            .borrow_mut()
            .insert(internal.to_string(), parsed.clone());
        parsed
    }

    fn text(&self, name: &str) -> Option<&[u8]> {
        self.files.get(name).map(Vec::as_slice)
    }
}

// ---------------------------------------------------------------------------
// bytecode decoder
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
enum Kind {
    Plain,
    Branch,
    Iinc,
    Table,
    Lookup,
}

#[derive(Clone, Debug)]
struct Ins {
    op: u8,
    operand: i32,
    iinc: i32,
    kind: Kind,
    target: usize,
    low: i32,
    high: i32,
    pairs: Vec<(i32, usize)>,
    default: usize,
}

fn op_length(op: u8) -> usize {
    match op {
        0x10 | 0x12 | 0x15 | 0x16 | 0x17 | 0x18 | 0x19 | 0x36 | 0x37 | 0x38 | 0x39 | 0x3a
        | 0xa9 | 0xbc => 2,
        0x11 | 0x13 | 0x14 | 0x84 | 0xb2 | 0xb3 | 0xb4 | 0xb5 | 0xb6 | 0xb7 | 0xb8 | 0xbb
        | 0xbd | 0xc0 | 0xc1 | 0xc6 | 0xc7 => 3,
        0x99..=0xa8 => 3,
        0xb9 | 0xba | 0xc8 | 0xc9 => 5,
        0xc5 => 4,
        _ => 1,
    }
}

fn u2(bytes: &[u8], at: usize) -> usize {
    u16::from_be_bytes([bytes[at], bytes[at + 1]]) as usize
}

fn decode(code: &[u8]) -> Result<Vec<Ins>, String> {
    let mut raw: Vec<Ins> = Vec::new();
    let mut starts: Vec<usize> = Vec::new();
    let mut at = 0usize;
    while at < code.len() {
        let start = at;
        let op = code[at];
        let mut operand = 0i32;
        let mut iinc = 0i32;
        let mut kind = Kind::Plain;
        let mut low = 0;
        let mut high = 0;
        let mut pairs = Vec::new();
        let mut default_offset = 0i32;
        let length = match op {
            0xaa => {
                let mut p = at + 1;
                while p % 4 != 0 {
                    p += 1;
                }
                default_offset =
                    i32::from_be_bytes([code[p], code[p + 1], code[p + 2], code[p + 3]]);
                low = i32::from_be_bytes([code[p + 4], code[p + 5], code[p + 6], code[p + 7]]);
                high = i32::from_be_bytes([code[p + 8], code[p + 9], code[p + 10], code[p + 11]]);
                kind = Kind::Table;
                let count = (high - low + 1) as usize;
                let end = p + 12 + count * 4;
                operand = p as i32;
                end - start
            }
            0xab => {
                let mut p = at + 1;
                while p % 4 != 0 {
                    p += 1;
                }
                default_offset =
                    i32::from_be_bytes([code[p], code[p + 1], code[p + 2], code[p + 3]]);
                let count =
                    i32::from_be_bytes([code[p + 4], code[p + 5], code[p + 6], code[p + 7]]);
                kind = Kind::Lookup;
                let mut q = p + 8;
                for _ in 0..count {
                    let key = i32::from_be_bytes([code[q], code[q + 1], code[q + 2], code[q + 3]]);
                    let offset =
                        i32::from_be_bytes([code[q + 4], code[q + 5], code[q + 6], code[q + 7]]);
                    pairs.push((key, offset as usize));
                    q += 8;
                }
                operand = p as i32;
                q - start
            }
            0xc4 => {
                let wrapped = code[at + 1];
                let index = u2(code, at + 2) as i32;
                let mut len = 4;
                if wrapped == 0x84 {
                    iinc = i16::from_be_bytes([code[at + 4], code[at + 5]]) as i32;
                    len = 6;
                }
                raw.push(Ins {
                    op: wrapped,
                    operand: index,
                    iinc,
                    kind: Kind::Iinc,
                    target: 0,
                    low: 0,
                    high: 0,
                    pairs: Vec::new(),
                    default: 0,
                });
                starts.push(start);
                at = start + len;
                continue;
            }
            _ => op_length(op),
        };
        match op {
            0x12 | 0x15..=0x19 | 0x36..=0x3a | 0xbc => operand = code[at + 1] as i32,
            0x10 => operand = code[at + 1] as i8 as i32,
            0x11 => operand = i16::from_be_bytes([code[at + 1], code[at + 2]]) as i32,
            0x99..=0xa8 | 0xc6 | 0xc7 => {
                kind = Kind::Branch;
                operand = i16::from_be_bytes([code[at + 1], code[at + 2]]) as i32;
                operand = start as i32 + operand;
            }
            0x84 => {
                kind = Kind::Iinc;
                operand = code[at + 1] as i32;
                iinc = code[at + 2] as i8 as i32;
            }
            0xb2..=0xb8 | 0xbb | 0xbd | 0xc0 | 0xc1 => operand = u2(code, at + 1) as i32,
            0xb9 | 0xba => operand = u2(code, at + 1) as i32,
            0xc5 => operand = u2(code, at + 1) as i32,
            0x13 | 0x14 => operand = u2(code, at + 1) as i32,
            _ => {}
        }
        raw.push(Ins {
            op,
            operand,
            iinc,
            kind,
            target: 0,
            low,
            high,
            pairs,
            default: 0,
        });
        starts.push(start);
        at = start + length;
    }
    // Resolve branch/switch targets to instruction indices.
    let index_of = |byte_at: i32| -> usize {
        starts
            .iter()
            .position(|start| *start == byte_at as usize)
            .unwrap_or(starts.len())
    };
    for ins in raw.iter_mut() {
        match ins.kind {
            Kind::Branch => ins.target = index_of(ins.operand),
            Kind::Table => {
                let p = ins.operand as usize;
                ins.default = index_of(p as i32 + default_offset_of(code, p));
                let count = (ins.high - ins.low + 1) as usize;
                let mut targets = Vec::with_capacity(count);
                for k in 0..count {
                    let offset = i32::from_be_bytes([
                        code[p + 12 + k * 4],
                        code[p + 13 + k * 4],
                        code[p + 14 + k * 4],
                        code[p + 15 + k * 4],
                    ]);
                    targets.push(index_of(p as i32 + offset));
                }
                ins.pairs = targets.into_iter().map(|target| (0, target)).collect();
            }
            Kind::Lookup => {
                let p = ins.operand as usize;
                ins.default = index_of(p as i32 + default_offset_of(code, p));
                for pair in ins.pairs.iter_mut() {
                    pair.1 = index_of(p as i32 + pair.1 as i32);
                }
            }
            _ => {}
        }
    }
    Ok(raw)
}

fn default_offset_of(code: &[u8], p: usize) -> i32 {
    i32::from_be_bytes([code[p], code[p + 1], code[p + 2], code[p + 3]])
}

// ---------------------------------------------------------------------------
// modeled objects
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct PartPose {
    x: f32,
    y: f32,
    z: f32,
    xr: f32,
    yr: f32,
    zr: f32,
    xs: f32,
    ys: f32,
    zs: f32,
}

impl PartPose {
    fn zero() -> Self {
        PartPose {
            xs: 1.0,
            ys: 1.0,
            zs: 1.0,
            ..PartPose::default()
        }
    }
    fn offset(x: f32, y: f32, z: f32) -> Self {
        PartPose {
            x,
            y,
            z,
            ..PartPose::zero()
        }
    }
    fn offset_rotation(x: f32, y: f32, z: f32, xr: f32, yr: f32, zr: f32) -> Self {
        PartPose {
            x,
            y,
            z,
            xr,
            yr,
            zr,
            ..PartPose::zero()
        }
    }
    fn translated(&self, dx: f32, dy: f32, dz: f32) -> Self {
        PartPose {
            x: self.x + dx,
            y: self.y + dy,
            z: self.z + dz,
            ..*self
        }
    }
    fn scaled(&self, f: f32) -> Self {
        if f == 1.0 {
            return *self;
        }
        self.scaled_xyz(f, f, f)
    }
    fn scaled_xyz(&self, x: f32, y: f32, z: f32) -> Self {
        PartPose {
            x: self.x * x,
            y: self.y * y,
            z: self.z * z,
            xs: self.xs * x,
            ys: self.ys * y,
            zs: self.zs * z,
            ..*self
        }
    }
    fn with_scale(&self, f: f32) -> Self {
        PartPose {
            xs: f,
            ys: f,
            zs: f,
            ..*self
        }
    }
}

#[derive(Clone, Debug)]
struct CubeDef {
    tex: (f32, f32),
    origin: [f32; 3],
    dims: [f32; 3],
    grow: [f32; 3],
    mirror: bool,
    scale: [f32; 2],
    visible: Vec<&'static str>,
}

#[derive(Default)]
struct PartState {
    cubes: Vec<CubeDef>,
    pose: PartPose,
    children: Vec<(String, Rc<RefCell<PartState>>)>,
}

struct MeshState {
    root: Rc<RefCell<PartState>>,
}

struct CubeListState {
    cubes: Vec<CubeDef>,
    tex_u: i32,
    tex_v: i32,
    mirror: bool,
}

struct LayerState {
    mesh: Rc<RefCell<MeshState>>,
    tex: (i32, i32),
}

#[derive(Debug)]
struct LegacyRandom {
    seed: u64,
}

impl LegacyRandom {
    const MASK: u64 = (1 << 48) - 1;
    fn new(seed: i64) -> Self {
        LegacyRandom {
            seed: (seed as u64 ^ 0x5DEECE66D) & Self::MASK,
        }
    }
    fn next(&mut self, bits: u32) -> i32 {
        self.seed = (self.seed.wrapping_mul(0x5DEECE66D).wrapping_add(0xB)) & Self::MASK;
        (self.seed >> (48 - bits)) as i32
    }
    fn next_float(&mut self) -> f32 {
        self.next(24) as f32 / (1 << 24) as f32
    }
    fn next_double(&mut self) -> f64 {
        (((self.next(26) as i64) << 27) + self.next(27) as i64) as f64 / (1i64 << 53) as f64
    }
    fn next_int(&mut self) -> i32 {
        self.next(32)
    }
    fn next_int_bound(&mut self, bound: i32) -> i32 {
        if bound <= 0 {
            return 0;
        }
        if bound & (bound - 1) == 0 {
            return ((bound as i64).wrapping_mul(self.next(31) as i64) >> 31) as i32;
        }
        loop {
            let bits = self.next(31);
            let value = bits % bound;
            if bits - value + (bound - 1) >= 0 {
                return value;
            }
        }
    }
    fn next_long(&mut self) -> i64 {
        ((self.next(32) as i64) << 32) + self.next(32) as i64
    }
}

#[derive(Clone)]
struct BabyTransform {
    scale_head: bool,
    y_head: f32,
    z_head: f32,
    head_scale: f32,
    body_scale: f32,
    body_y: f32,
    head_parts: Val,
}

#[derive(Clone, Debug)]
struct ModelLayerLocation {
    model: String,
    layer: String,
}

impl ModelLayerLocation {
    fn key(&self) -> String {
        format!("{}#{}", self.model, self.layer)
    }
}

const DIRECTIONS: [&str; 6] = ["DOWN", "UP", "NORTH", "SOUTH", "WEST", "EAST"];

#[derive(Clone)]
enum Val {
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    Null,
    Unknown,
    Uninit(String),
    Str(String),
    Ident(String),
    Mesh(Rc<RefCell<MeshState>>),
    Part(Rc<RefCell<PartState>>),
    Cubes(Rc<RefCell<CubeListState>>),
    Deform(Rc<[f32; 3]>),
    Pose(Rc<PartPose>),
    Vec3([f32; 3]),
    Layer(Rc<LayerState>),
    Material(Rc<(i32, i32)>),
    MapBuilder(Rc<RefCell<Vec<(Val, Val)>>>),
    Map(Rc<RefCell<Vec<(Val, Val)>>>),
    ArmorSet(Rc<Vec<Val>>),
    Lambda {
        class: Rc<ClassFile>,
        method: String,
        desc: String,
        captured: Vec<Val>,
        is_static: bool,
    },
    ModelLayer(Rc<ModelLayerLocation>),
    BabyTransform(Rc<BabyTransform>),
    Random(Rc<RefCell<LegacyRandom>>),
    Direction(&'static str),
    DirSet(Rc<Vec<&'static str>>),
    Set(Rc<Vec<Val>>),
    MSet(Rc<RefCell<Vec<Val>>>),
    MList(Rc<RefCell<Vec<Val>>>),
    Array(Rc<RefCell<Vec<Val>>>),
    Transformer(f32),
    Vertex([f32; 5]),
}

fn val_name(value: &Val) -> &'static str {
    match value {
        Val::Int(_) => "int",
        Val::Long(_) => "long",
        Val::Float(_) => "float",
        Val::Double(_) => "double",
        Val::Null => "null",
        Val::Uninit(_) => "uninitialised object",
        Val::Unknown => "unknown",
        Val::Str(_) => "string",
        Val::Ident(_) => "Identifier",
        Val::Mesh(_) => "MeshDefinition",
        Val::Part(_) => "PartDefinition",
        Val::Cubes(_) => "CubeListBuilder",
        Val::Deform(_) => "CubeDeformation",
        Val::Pose(_) => "PartPose",
        Val::Vec3(_) => "Vector3f",
        Val::Layer(_) => "LayerDefinition",
        Val::Material(_) => "MaterialDefinition",
        Val::MapBuilder(_) => "ImmutableMap.Builder",
        Val::Map(_) => "Map",
        Val::ArmorSet(_) => "ArmorModelSet",
        Val::Lambda { .. } => "lambda",
        Val::ModelLayer(_) => "ModelLayerLocation",
        Val::BabyTransform(_) => "BabyModelTransform",
        Val::Random(_) => "RandomSource",
        Val::Direction(_) => "Direction",
        Val::DirSet(_) => "Set<Direction>",
        Val::Set(_) => "Set",
        Val::MSet(_) => "HashSet",
        Val::MList(_) => "List",
        Val::Array(_) => "array",
        Val::Transformer(_) => "MeshTransformer",
        Val::Vertex(_) => "Vertex",
    }
}

fn val_eq(left: &Val, right: &Val) -> bool {
    match (left, right) {
        (Val::Str(a), Val::Str(b)) => a == b,
        (Val::Ident(a), Val::Ident(b)) => a == b,
        (Val::Int(a), Val::Int(b)) => a == b,
        (Val::ModelLayer(a), Val::ModelLayer(b)) => a.key() == b.key(),
        (Val::Direction(a), Val::Direction(b)) => a == b,
        (Val::Null, Val::Null) => true,
        _ => false,
    }
}

fn pop(stack: &mut Vec<Val>) -> Result<Val, String> {
    stack.pop().ok_or_else(|| "stack underflow".to_string())
}

fn poke(stack: &mut Vec<Val>, value: Val) {
    stack.push(value);
}

fn take_f32(value: &Val) -> Result<f32, String> {
    match value {
        Val::Float(f) => Ok(*f),
        Val::Int(i) => Ok(*i as f32),
        Val::Double(d) => Ok(*d as f32),
        other => Err(format!("expected float, got {}", val_name(other))),
    }
}

fn take_f64(value: &Val) -> Result<f64, String> {
    match value {
        Val::Double(d) => Ok(*d),
        Val::Float(f) => Ok(*f as f64),
        Val::Int(i) => Ok(*i as f64),
        Val::Long(l) => Ok(*l as f64),
        other => Err(format!("expected double, got {}", val_name(other))),
    }
}

fn take_int(value: &Val) -> Result<i32, String> {
    match value {
        Val::Int(i) => Ok(*i),
        Val::Long(l) => Ok(*l as i32),
        other => Err(format!("expected int, got {}", val_name(other))),
    }
}

fn take_long(value: &Val) -> Result<i64, String> {
    match value {
        Val::Long(l) => Ok(*l),
        Val::Int(i) => Ok(*i as i64),
        other => Err(format!("expected long, got {}", val_name(other))),
    }
}

fn take_str(value: &Val) -> Result<String, String> {
    match value {
        Val::Str(s) => Ok(s.clone()),
        other => Err(format!("expected string, got {}", val_name(other))),
    }
}

// ---------------------------------------------------------------------------
// the interpreter
// ---------------------------------------------------------------------------

struct Vm {
    jar: JarFiles,
    statics: BTreeMap<String, BTreeMap<String, Val>>,
    inited: BTreeSet<String>,
    depth: usize,
    steps: u64,
    notes: Vec<String>,
}

const MAX_DEPTH: usize = 96;
const MAX_STEPS: u64 = 200_000_000;

impl Vm {
    fn new(jar: JarFiles) -> Self {
        Vm {
            jar,
            statics: BTreeMap::new(),
            inited: BTreeSet::new(),
            depth: 0,
            steps: 0,
            notes: Vec::new(),
        }
    }

    fn class(&self, internal: &str) -> Result<Rc<ClassFile>, String> {
        self.jar
            .class(internal)
            .ok_or_else(|| format!("class {internal} is not in the jar"))
    }

    fn member(class: &ClassFile, name: &str, desc: &str) -> Option<classfile::Member> {
        class
            .methods
            .iter()
            .find(|member| member.name == name && (desc.is_empty() || member.descriptor == desc))
            .cloned()
    }

    fn init(&mut self, internal: &str) -> Result<(), String> {
        if self.inited.contains(internal) {
            return Ok(());
        }
        let Some(class) = self.jar.class(internal) else {
            return Ok(());
        };
        self.inited.insert(internal.to_string());
        if !class.super_name.is_empty() && class.super_name != "java/lang/Object" {
            self.init(&class.super_name)?;
        }
        if let Some(initialiser) = Self::member(&class, "<clinit>", "()V") {
            if let Err(error) = self.call(class.clone(), &initialiser, Vec::new(), None) {
                // Some static initialisers build objects this evaluator does not
                // model (Transformation, SpriteMapper, loggers). They are not
                // needed by the model code; keep the fields that were set.
                self.notes.push(format!("init {internal}: {error}"));
            }
        }
        Ok(())
    }

    fn get_static(&mut self, owner: &str, name: &str) -> Result<Val, String> {
        if owner == "net/minecraft/core/Direction" {
            if let Some(direction) = DIRECTIONS.iter().find(|candidate| **candidate == name) {
                return Ok(Val::Direction(direction));
            }
        }
        if owner == "java/lang/Math" {
            match name {
                "PI" => return Ok(Val::Double(std::f64::consts::PI)),
                "E" => return Ok(Val::Double(std::f64::consts::E)),
                _ => {}
            }
        }
        if owner == "net/minecraft/world/entity/EquipmentSlot" {
            if ["HEAD", "CHEST", "LEGS", "FEET"].contains(&name) {
                return Ok(Val::Direction(Box::leak(name.to_string().into_boxed_str())));
            }
        }
        self.init(owner)?;
        let mut current = owner.to_string();
        for _ in 0..64 {
            if let Some(fields) = self.statics.get(&current) {
                if let Some(value) = fields.get(name) {
                    return Ok(value.clone());
                }
            }
            let Some(class) = self.jar.class(&current) else {
                break;
            };
            if class.super_name.is_empty() || class.super_name == "java/lang/Object" {
                break;
            }
            let parent = class.super_name.clone();
            self.init(&parent)?;
            current = parent;
        }
        Err(format!(
            "static field {owner}.{name} is not known; init notes: {:?}",
            &self.notes[self.notes.len().saturating_sub(4)..]
        ))
    }

    fn set_static(&mut self, owner: &str, name: &str, value: Val) {
        self.statics
            .entry(owner.to_string())
            .or_default()
            .insert(name.to_string(), value);
    }

    fn call(
        &mut self,
        class: Rc<ClassFile>,
        method: &classfile::Member,
        args: Vec<Val>,
        receiver: Option<Val>,
    ) -> Result<Val, String> {
        if self.depth >= MAX_DEPTH {
            return Err(format!(
                "call depth exceeded at {}.{}",
                class.name, method.name
            ));
        }
        let Some(code) = method.code.as_ref().map(|code| code.bytes.clone()) else {
            return Ok(Val::Null);
        };
        self.depth += 1;
        let result = self.exec(&class, method, &code, args, receiver);
        self.depth -= 1;
        result
    }

    fn exec(
        &mut self,
        class: &Rc<ClassFile>,
        method: &classfile::Member,
        code: &[u8],
        args: Vec<Val>,
        receiver: Option<Val>,
    ) -> Result<Val, String> {
        let ins =
            decode(code).map_err(|error| format!("{}.{}: {error}", class.name, method.name))?;
        let max_locals = method
            .code
            .as_ref()
            .map(|code| code.max_locals as usize)
            .unwrap_or(args.len() + 8);
        let mut locals: Vec<Val> = vec![Val::Null; max_locals.max(args.len() + 1)];
        let mut slot = 0usize;
        if let Some(receiver) = receiver {
            locals[0] = receiver;
            slot = 1;
        }
        for argument in args {
            if slot < locals.len() {
                locals[slot] = argument;
            }
            slot += 1;
        }
        let mut stack: Vec<Val> = Vec::new();
        let mut at = 0usize;
        let context = |ins: &Ins, at: usize| {
            format!(
                "{}.{}{} pc {} op {:#04x}",
                class.name, method.name, method.descriptor, at, ins.op
            )
        };
        loop {
            if at >= ins.len() {
                return Err(format!("{}.{}: fell off the end", class.name, method.name));
            }
            self.steps += 1;
            if self.steps > MAX_STEPS {
                return Err("instruction budget exhausted".to_string());
            }
            let current = &ins[at];
            let op = current.op;
            match op {
                0x00 => at += 1,
                0x01 => {
                    poke(&mut stack, Val::Null);
                    at += 1;
                }
                0x02..=0x08 => {
                    poke(&mut stack, Val::Int(op as i32 - 3));
                    at += 1;
                }
                0x09 => {
                    poke(&mut stack, Val::Long(0));
                    at += 1;
                }
                0x0a => {
                    poke(&mut stack, Val::Long(1));
                    at += 1;
                }
                0x0b => {
                    poke(&mut stack, Val::Float(0.0));
                    at += 1;
                }
                0x0c => {
                    poke(&mut stack, Val::Float(1.0));
                    at += 1;
                }
                0x0d => {
                    poke(&mut stack, Val::Float(2.0));
                    at += 1;
                }
                0x0e => {
                    poke(&mut stack, Val::Double(0.0));
                    at += 1;
                }
                0x0f => {
                    poke(&mut stack, Val::Double(1.0));
                    at += 1;
                }
                0x10 | 0x11 => {
                    poke(&mut stack, Val::Int(current.operand));
                    at += 1;
                }
                0x12 | 0x13 => {
                    let value = self.ldc(class, current.operand as usize)?;
                    poke(&mut stack, value);
                    at += 1;
                }
                0x14 => {
                    let value = self.ldc2(class, current.operand as usize)?;
                    poke(&mut stack, value);
                    at += 1;
                }
                0x15..=0x19 => {
                    poke(&mut stack, locals[current.operand as usize].clone());
                    at += 1;
                }
                0x1a..=0x2d => {
                    let slot = (op - 0x1a) as usize % 4;
                    poke(&mut stack, locals[slot].clone());
                    at += 1;
                }
                0x2e..=0x35 => {
                    let index = take_int(&pop(&mut stack)?)?;
                    let array = pop(&mut stack)?;
                    let Val::Array(items) = array else {
                        return Err(format!(
                            "{}: array load on {}",
                            context(current, at),
                            val_name(&array)
                        ));
                    };
                    let items = items.borrow();
                    let value = items
                        .get(index as usize)
                        .cloned()
                        .ok_or_else(|| format!("array index {index} out of bounds"))?;
                    poke(&mut stack, value);
                    at += 1;
                }
                0x36..=0x3a => {
                    locals[current.operand as usize] = pop(&mut stack)?;
                    at += 1;
                }
                0x3b..=0x4e => {
                    locals[(op - 0x3b) as usize % 4] = pop(&mut stack)?;
                    at += 1;
                }
                0x4f..=0x56 => {
                    let value = pop(&mut stack)?;
                    let index = take_int(&pop(&mut stack)?)?;
                    let array = pop(&mut stack)?;
                    let Val::Array(items) = array else {
                        return Err(format!(
                            "{}: array store on {}",
                            context(current, at),
                            val_name(&array)
                        ));
                    };
                    let mut items = items.borrow_mut();
                    if (index as usize) < items.len() {
                        items[index as usize] = value;
                    }
                    at += 1;
                }
                0x57 => {
                    pop(&mut stack)?;
                    at += 1;
                }
                0x58 => {
                    pop(&mut stack)?;
                    pop(&mut stack)?;
                    at += 1;
                }
                0x59 => {
                    let value = stack.last().cloned().unwrap_or(Val::Null);
                    poke(&mut stack, value);
                    at += 1;
                }
                0x5a => {
                    let a = pop(&mut stack)?;
                    let b = pop(&mut stack)?;
                    poke(&mut stack, a.clone());
                    poke(&mut stack, b);
                    poke(&mut stack, a);
                    at += 1;
                }
                0x5c => {
                    let a = pop(&mut stack)?;
                    let b = pop(&mut stack)?;
                    poke(&mut stack, b.clone());
                    poke(&mut stack, a.clone());
                    poke(&mut stack, b);
                    poke(&mut stack, a);
                    at += 1;
                }
                0x5f => {
                    let a = pop(&mut stack)?;
                    let b = pop(&mut stack)?;
                    poke(&mut stack, a);
                    poke(&mut stack, b);
                    at += 1;
                }
                0x60..=0x83 => {
                    self.arith(op, &mut stack)?;
                    at += 1;
                }
                0x84 => {
                    let slot = current.operand as usize;
                    let value = take_int(&locals[slot])? + current.iinc;
                    locals[slot] = Val::Int(value);
                    at += 1;
                }
                0x85..=0x98 => {
                    self.convert(op, &mut stack)?;
                    at += 1;
                }
                0x99..=0xa6 => {
                    let taken = match op {
                        0x99..=0x9e => {
                            let value = pop(&mut stack)?;
                            let value = take_int(&value)?;
                            match op {
                                0x99 => value == 0,
                                0x9a => value != 0,
                                0x9b => value < 0,
                                0x9c => value >= 0,
                                0x9d => value > 0,
                                _ => value <= 0,
                            }
                        }
                        0x9f..=0xa4 => {
                            let right = pop(&mut stack)?;
                            let left = pop(&mut stack)?;
                            let right = take_int(&right)?;
                            let left = take_int(&left)?;
                            match op {
                                0x9f => left == right,
                                0xa0 => left != right,
                                0xa1 => left < right,
                                0xa2 => left >= right,
                                0xa3 => left > right,
                                _ => left <= right,
                            }
                        }
                        0xa5 => {
                            let right = pop(&mut stack)?;
                            let left = pop(&mut stack)?;
                            val_eq(&left, &right)
                        }
                        _ => {
                            let right = pop(&mut stack)?;
                            let left = pop(&mut stack)?;
                            !val_eq(&left, &right)
                        }
                    };
                    if taken {
                        at = current.target;
                    } else {
                        at += 1;
                    }
                }
                0xa7 => at = current.target,
                0xaa => {
                    let key = take_int(&pop(&mut stack)?)?;
                    let index = if key >= current.low && key <= current.high {
                        (key - current.low) as usize
                    } else {
                        usize::MAX
                    };
                    at = if index < current.pairs.len() {
                        current.pairs[index].1
                    } else {
                        current.default
                    };
                }
                0xab => {
                    let key = take_int(&pop(&mut stack)?)?;
                    let mut next = current.default;
                    for (pair_key, target) in &current.pairs {
                        if *pair_key == key {
                            next = *target;
                            break;
                        }
                    }
                    at = next;
                }
                0xac..=0xb0 => {
                    let value = pop(&mut stack)?;
                    return Ok(match op {
                        0xac | 0xad | 0xae => value,
                        0xaf => value,
                        _ => value,
                    });
                }
                0xb1 => return Ok(Val::Null),
                0xb2 => {
                    let (owner, name, _) =
                        classfile::field_ref(&class.constants, current.operand as usize)
                            .ok_or_else(|| format!("{}: bad getstatic", context(current, at)))?;
                    let value = self.get_static(&owner, &name)?;
                    poke(&mut stack, value);
                    at += 1;
                }
                0xb3 => {
                    let (owner, name, _) =
                        classfile::field_ref(&class.constants, current.operand as usize)
                            .ok_or_else(|| format!("{}: bad putstatic", context(current, at)))?;
                    let value = pop(&mut stack)?;
                    self.set_static(&owner, &name, value);
                    at += 1;
                }
                0xb4 => {
                    let (_owner, name, _) =
                        classfile::field_ref(&class.constants, current.operand as usize)
                            .ok_or_else(|| format!("{}: bad getfield", context(current, at)))?;
                    let receiver = pop(&mut stack)?;
                    let value = self.get_field(&receiver, &name)?;
                    poke(&mut stack, value);
                    at += 1;
                }
                0xb5 => {
                    let (owner, name, _) =
                        classfile::field_ref(&class.constants, current.operand as usize)
                            .ok_or_else(|| format!("{}: bad putfield", context(current, at)))?;
                    let value = pop(&mut stack)?;
                    let receiver = pop(&mut stack)?;
                    self.set_field(&receiver, &owner, &name, value)?;
                    at += 1;
                }
                0xb6 | 0xb7 | 0xb8 | 0xb9 => {
                    let (owner, name, desc) =
                        classfile::method_ref(&class.constants, current.operand as usize)
                            .ok_or_else(|| format!("{}: bad method ref", context(current, at)))?;
                    let value = self
                        .dispatch(op, class, &owner, &name, &desc, &mut stack)
                        .map_err(|error| format!("{}: {error}", context(current, at)))?;
                    if let Some(value) = value {
                        if op == 0xb7 && name == "<init>" {
                            // `new X; dup; invokespecial X.<init>` turns the
                            // duplicate into the initialised object.
                            if let Some(slot) = stack.last_mut() {
                                if matches!(slot, Val::Uninit(_)) {
                                    *slot = value;
                                }
                            }
                        } else {
                            poke(&mut stack, value);
                        }
                    }
                    at += 1;
                }
                0xba => {
                    let value = self
                        .invoke_dynamic(class, current.operand as usize, &mut stack)
                        .map_err(|error| format!("{}: {error}", context(current, at)))?;
                    if let Some(value) = value {
                        poke(&mut stack, value);
                    }
                    at += 1;
                }
                0xbb => {
                    let name = class_name_at(&class.constants, current.operand as usize)
                        .ok_or_else(|| format!("{}: bad new", context(current, at)))?;
                    poke(&mut stack, Val::Uninit(name));
                    at += 1;
                }
                0xbc | 0xbd => {
                    let length = take_int(&pop(&mut stack)?)?;
                    let items = vec![Val::Null; length.max(0) as usize];
                    poke(&mut stack, Val::Array(Rc::new(RefCell::new(items))));
                    at += 1;
                }
                0xbe => {
                    let array = pop(&mut stack)?;
                    let Val::Array(items) = array else {
                        return Err(format!(
                            "{}: arraylength on {}",
                            context(current, at),
                            val_name(&array)
                        ));
                    };
                    poke(&mut stack, Val::Int(items.borrow().len() as i32));
                    at += 1;
                }
                0xbf => {
                    let value = pop(&mut stack)?;
                    return Err(format!(
                        "{}: athrow {}",
                        context(current, at),
                        stringify(&value)
                    ));
                }
                0xc0 => {
                    // checkcast keeps identity
                    at += 1;
                }
                0xc1 => {
                    // instanceof: we only know modeled values; keep it permissive.
                    pop(&mut stack)?;
                    poke(&mut stack, Val::Int(1));
                    at += 1;
                }
                0xc2 | 0xc3 => at += 1,
                0xc6 => {
                    let value = pop(&mut stack)?;
                    if matches!(value, Val::Null) {
                        at = current.target;
                    } else {
                        at += 1;
                    }
                }
                0xc7 => {
                    let value = pop(&mut stack)?;
                    if !matches!(value, Val::Null) {
                        at = current.target;
                    } else {
                        at += 1;
                    }
                }
                other => {
                    return Err(format!(
                        "{}: unsupported opcode {other:#04x} at {at}",
                        context(current, at)
                    ))
                }
            }
        }
    }

    fn ldc(&mut self, class: &Rc<ClassFile>, index: usize) -> Result<Val, String> {
        match class.constants.get(index) {
            Some(Constant::Integer(value)) => Ok(Val::Int(*value)),
            Some(Constant::Float(value)) => Ok(Val::Float(*value)),
            Some(Constant::String(_)) => Ok(Val::Str(
                classfile::string_constant(&class.constants, index)
                    .ok_or_else(|| format!("{}: bad string constant #{index}", class.name))?,
            )),
            Some(Constant::Class(_)) => Ok(Val::Str(
                class_name_at(&class.constants, index).unwrap_or_default(),
            )),
            other => Err(format!(
                "{}: unsupported ldc #{index}: {other:?}",
                class.name
            )),
        }
    }

    fn ldc2(&mut self, class: &Rc<ClassFile>, index: usize) -> Result<Val, String> {
        match class.constants.get(index) {
            Some(Constant::Long(value)) => Ok(Val::Long(*value)),
            Some(Constant::Double(value)) => Ok(Val::Double(*value)),
            // `hzr-bytecode`'s reader stores a wide constant one slot past the
            // index the instruction names (see its `classfile.rs`); read what it
            // wrote.
            _ => match class.constants.get(index + 1) {
                Some(Constant::Long(value)) => Ok(Val::Long(*value)),
                Some(Constant::Double(value)) => Ok(Val::Double(*value)),
                other => Err(format!(
                    "{}: unsupported ldc2 #{index}: {other:?}",
                    class.name
                )),
            },
        }
    }

    fn arith(&mut self, op: u8, stack: &mut Vec<Val>) -> Result<(), String> {
        let b = pop(stack)?;
        if matches!(op, 0x74 | 0x75 | 0x76 | 0x77) {
            let value = match (op, &b) {
                (0x74, Val::Int(v)) => Val::Int(v.wrapping_neg()),
                (0x75, Val::Long(v)) => Val::Long(v.wrapping_neg()),
                (0x76, Val::Float(v)) => Val::Float(-v),
                (0x77, Val::Double(v)) => Val::Double(-v),
                _ => return Err(format!("bad negate operand {}", val_name(&b))),
            };
            poke(stack, value);
            return Ok(());
        }
        let a = pop(stack)?;
        let value = match op {
            0x60 => Val::Int(take_int(&a)? + take_int(&b)?),
            0x61 => Val::Long(take_long(&a)? + take_long(&b)?),
            0x62 => Val::Float(take_f32(&a)? + take_f32(&b)?),
            0x63 => Val::Double(take_f64(&a)? + take_f64(&b)?),
            0x64 => Val::Int(take_int(&a)? - take_int(&b)?),
            0x65 => Val::Long(take_long(&a)? - take_long(&b)?),
            0x66 => Val::Float(take_f32(&a)? - take_f32(&b)?),
            0x67 => Val::Double(take_f64(&a)? - take_f64(&b)?),
            0x68 => Val::Int(take_int(&a)? * take_int(&b)?),
            0x69 => Val::Long(take_long(&a)? * take_long(&b)?),
            0x6a => Val::Float(take_f32(&a)? * take_f32(&b)?),
            0x6b => Val::Double(take_f64(&a)? * take_f64(&b)?),
            0x6c => Val::Int(take_int(&a)? / take_int(&b)?),
            0x6d => Val::Long(take_long(&a)? / take_long(&b)?),
            0x6e => Val::Float(take_f32(&a)? / take_f32(&b)?),
            0x6f => Val::Double(take_f64(&a)? / take_f64(&b)?),
            0x70 => Val::Int(take_int(&a)? % take_int(&b)?),
            0x71 => Val::Long(take_long(&a)? % take_long(&b)?),
            0x72 => Val::Float(take_f32(&a)? % take_f32(&b)?),
            0x73 => Val::Double(take_f64(&a)? % take_f64(&b)?),
            0x78 => Val::Int(take_int(&a)? << take_int(&b)?),
            0x79 => Val::Long(take_long(&a)? << take_int(&b)?),
            0x7a => Val::Int(take_int(&a)? >> take_int(&b)?),
            0x7b => Val::Long(take_long(&a)? >> take_int(&b)?),
            0x7c => Val::Int(((take_int(&a)? as u32) >> take_int(&b)?) as i32),
            0x7d => Val::Long(((take_long(&a)? as u64) >> take_int(&b)?) as i64),
            0x7e => Val::Int(take_int(&a)? & take_int(&b)?),
            0x7f => Val::Long(take_long(&a)? & take_long(&b)?),
            0x80 => Val::Int(take_int(&a)? | take_int(&b)?),
            0x81 => Val::Long(take_long(&a)? | take_long(&b)?),
            0x82 => Val::Int(take_int(&a)? ^ take_int(&b)?),
            0x83 => Val::Long(take_long(&a)? ^ take_long(&b)?),
            other => return Err(format!("unsupported arithmetic {other:#04x}")),
        };
        poke(stack, value);
        Ok(())
    }

    fn convert(&mut self, op: u8, stack: &mut Vec<Val>) -> Result<(), String> {
        let value = pop(stack)?;
        let converted = match op {
            0x85 => Val::Long(take_int(&value)? as i64),
            0x86 => Val::Float(take_int(&value)? as f32),
            0x87 => Val::Double(take_int(&value)? as f64),
            0x88 => Val::Int(take_long(&value)? as i32),
            0x89 => Val::Float(take_long(&value)? as f32),
            0x8a => Val::Double(take_long(&value)? as f64),
            0x8b => Val::Int(take_f32(&value)? as i32),
            0x8c => Val::Long(take_f32(&value)? as i64),
            0x8d => Val::Double(take_f32(&value)? as f64),
            0x8e => Val::Int(take_f64(&value)? as i32),
            0x8f => Val::Long(take_f64(&value)? as i64),
            0x90 => Val::Float(take_f64(&value)? as f32),
            0x91 => Val::Int(take_int(&value)? as i8 as i32),
            0x92 => Val::Int(take_int(&value)? as u16 as i32),
            0x93 => Val::Int(take_int(&value)? as i16 as i32),
            0x94 => {
                // lcmp
                let other = pop(stack)?;
                let left = take_long(&other)?;
                let right = take_long(&value)?;
                Val::Int(if left < right {
                    -1
                } else if left > right {
                    1
                } else {
                    0
                })
            }
            0x95 | 0x96 => {
                let other = pop(stack)?;
                let left = take_f32(&other)?;
                let right = take_f32(&value)?;
                Val::Int(if left < right {
                    -1
                } else if left > right {
                    1
                } else {
                    0
                })
            }
            0x97 | 0x98 => {
                let other = pop(stack)?;
                let left = take_f64(&other)?;
                let right = take_f64(&value)?;
                Val::Int(if left < right {
                    -1
                } else if left > right {
                    1
                } else {
                    0
                })
            }
            other => return Err(format!("unsupported conversion {other:#04x}")),
        };
        poke(stack, converted);
        Ok(())
    }

    fn branch(&mut self, op: u8, value: &Val, _other: Option<&Val>) -> Result<bool, String> {
        Ok(match op {
            0x99 => take_int(value)? == 0,
            0x9a => take_int(value)? != 0,
            0x9b => take_int(value)? < 0,
            0x9c => take_int(value)? >= 0,
            0x9d => take_int(value)? > 0,
            0x9e => take_int(value)? <= 0,
            _ => return Err(format!("unsupported branch {op:#04x}")),
        })
    }

    fn get_field(&mut self, receiver: &Val, name: &str) -> Result<Val, String> {
        match receiver {
            Val::Pose(pose) => Ok(Val::Float(match name {
                "x" => pose.x,
                "y" => pose.y,
                "z" => pose.z,
                "xRot" => pose.xr,
                "yRot" => pose.yr,
                "zRot" => pose.zr,
                "xScale" => pose.xs,
                "yScale" => pose.ys,
                "zScale" => pose.zs,
                _ => return Err(format!("PartPose.{name}")),
            })),
            other => Err(format!("getfield {name} on {}", val_name(other))),
        }
    }

    fn set_field(
        &mut self,
        receiver: &Val,
        _owner: &str,
        name: &str,
        _value: Val,
    ) -> Result<(), String> {
        Err(format!("putfield {name} on {}", val_name(receiver)))
    }

    fn dispatch(
        &mut self,
        op: u8,
        class: &Rc<ClassFile>,
        owner: &str,
        name: &str,
        desc: &str,
        stack: &mut Vec<Val>,
    ) -> Result<Option<Val>, String> {
        let mut arguments: Vec<Val> = Vec::new();
        let kinds = classfile::descriptor_types(desc);
        let count = kinds.len().saturating_sub(1);
        for _ in 0..count {
            arguments.push(pop(stack)?);
        }
        arguments.reverse();
        let receiver = if op == 0xb8 { None } else { Some(pop(stack)?) };
        self.invoke(op, class, owner, name, desc, receiver, arguments)
    }

    fn invoke(
        &mut self,
        op: u8,
        _class: &Rc<ClassFile>,
        owner: &str,
        name: &str,
        desc: &str,
        receiver: Option<Val>,
        arguments: Vec<Val>,
    ) -> Result<Option<Val>, String> {
        if op == 0xb7 && name == "<init>" {
            let value = self.construct(owner, arguments.clone())?;
            return Ok(Some(value));
        }
        if let Some(result) = self.native(op, owner, name, desc, receiver.as_ref(), &arguments)? {
            return Ok(Some(result));
        }
        // A lambda application: run its implementation method.
        if let Some(Val::Lambda {
            class: lambda_class,
            method,
            desc,
            captured,
            is_static,
        }) = receiver.as_ref()
        {
            if matches!(name, "apply" | "accept" | "test" | "get") {
                let lambda_class = lambda_class.clone();
                let method = method.clone();
                let lambda_desc = desc.clone();
                let mut all = captured.clone();
                all.extend(arguments);
                let Some(member) = Self::member(&lambda_class, &method, &lambda_desc) else {
                    return Err(format!(
                        "lambda implementation {}.{} {} is missing",
                        lambda_class.name, method, lambda_desc
                    ));
                };
                let (receiver, args) = if *is_static {
                    (None, all)
                } else {
                    let mut all = all;
                    let receiver = all.remove(0);
                    (Some(receiver), all)
                };
                let value = self.call(lambda_class, &member, args, receiver)?;
                return Ok(Some(value));
            }
        }
        // Unknown receiver kinds cannot be dispatched.
        if let Some(receiver) = receiver.as_ref() {
            if matches!(receiver, Val::Unknown) {
                return Ok(Some(Val::Unknown));
            }
        }
        // Fall back to interpreting the method from the jar.
        let mut current = owner.to_string();
        for _ in 0..64 {
            if let Some(target) = self.jar.class(&current) {
                if let Some(member) = Self::member(&target, name, desc) {
                    if op == 0xb8 {
                        self.init(&target.name)?;
                        let value = self.call(target, &member, arguments, None)?;
                        return Ok(Some(value));
                    }
                    if let Some(receiver) = receiver.clone() {
                        let value = self.call(target, &member, arguments, Some(receiver))?;
                        return Ok(Some(value));
                    }
                }
                if target.super_name.is_empty() || target.super_name == "java/lang/Object" {
                    break;
                }
                current = target.super_name.clone();
            } else {
                break;
            }
        }
        Err(format!(
            "unhandled {} call {}.{}{}",
            if op == 0xb8 { "static" } else { "virtual" },
            owner,
            name,
            desc
        ))
    }
    fn invoke_dynamic(
        &mut self,
        class: &Rc<ClassFile>,
        index: usize,
        stack: &mut Vec<Val>,
    ) -> Result<Option<Val>, String> {
        let Some(Constant::InvokeDynamic { bootstrap, .. }) = class.constants.get(index) else {
            return Err(format!("bad invokedynamic #{index}"));
        };
        let Some(entry) = class.bootstraps.get(*bootstrap).cloned() else {
            return Err("invokedynamic without bootstrap".to_string());
        };
        let Some(Constant::InvokeDynamic { name, .. }) = class.constants.get(index) else {
            return Err("bad invokedynamic".to_string());
        };
        let _call_site_name = classfile::utf8(
            &class.constants,
            name.trim_start_matches('#').parse::<usize>().unwrap_or(0),
        );
        // The call site descriptor says how many stack arguments the captures are.
        let descriptor = {
            let nat = name.trim_start_matches('#').parse::<usize>().unwrap_or(0);
            match class.constants.get(nat) {
                Some(Constant::NameAndType { kind, .. }) => {
                    let at: usize = kind.trim_start_matches('#').parse().unwrap_or(0);
                    classfile::utf8(&class.constants, at)
                }
                _ => String::new(),
            }
        };
        let types = classfile::descriptor_types(&descriptor);
        let mut captured: Vec<Val> = Vec::new();
        for _ in 0..types.len().saturating_sub(1) {
            captured.push(pop(stack)?);
        }
        captured.reverse();
        // Resolve the bootstrap method name.
        let bootstrap_name = match class.constants.get(entry.method) {
            Some(Constant::MethodHandle { reference }) => match class.constants.get(*reference) {
                Some(Constant::Methodref { name, .. }) => {
                    let at: usize = name.trim_start_matches('#').parse().unwrap_or(0);
                    match class.constants.get(at) {
                        Some(Constant::NameAndType { name, .. }) => {
                            let at: usize = name.trim_start_matches('#').parse().unwrap_or(0);
                            classfile::utf8(&class.constants, at)
                        }
                        _ => String::new(),
                    }
                }
                _ => String::new(),
            },
            _ => String::new(),
        };
        if bootstrap_name == "makeConcatWithConstants" || bootstrap_name == "makeConcat" {
            let recipe = entry
                .arguments
                .first()
                .and_then(|argument| classfile::string_constant(&class.constants, *argument))
                .unwrap_or_default();
            let mut out = String::new();
            let mut constants = entry.arguments.iter().skip(1);
            let mut values = captured.iter();
            for piece in recipe.split('\u{1}') {
                out.push_str(piece);
                if let Some(value) = values.next() {
                    out.push_str(&stringify(value));
                }
                let _ = &mut constants;
            }
            return Ok(Some(Val::Str(out)));
        }
        // Otherwise it is a lambda: the implementation method is the MethodHandle
        // among the bootstrap arguments.
        for argument in &entry.arguments {
            let Some(Constant::MethodHandle { reference }) = class.constants.get(*argument) else {
                continue;
            };
            let (target_owner, target_name) = match class.constants.get(*reference) {
                Some(Constant::Methodref {
                    class: owner, name, ..
                })
                | Some(Constant::InterfaceMethodref {
                    class: owner, name, ..
                }) => (owner.clone(), name.clone()),
                _ => continue,
            };
            let kind = match class.constants.get(*reference) {
                Some(Constant::Methodref { kind, .. }) => kind.clone(),
                _ => "M".to_string(),
            };
            let target_owner_at: usize = target_owner.trim_start_matches('#').parse().unwrap_or(0);
            let target_name_at: usize = target_name.trim_start_matches('#').parse().unwrap_or(0);
            let target_internal = match class.constants.get(target_owner_at) {
                Some(Constant::Class(reference)) => {
                    let inner: usize = reference.trim_start_matches('#').parse().unwrap_or(0);
                    classfile::utf8(&class.constants, inner)
                }
                _ => continue,
            };
            let (method_name, method_desc) = match class.constants.get(target_name_at) {
                Some(Constant::NameAndType { name, kind }) => {
                    let n: usize = name.trim_start_matches('#').parse().unwrap_or(0);
                    let k: usize = kind.trim_start_matches('#').parse().unwrap_or(0);
                    (
                        classfile::utf8(&class.constants, n),
                        classfile::utf8(&class.constants, k),
                    )
                }
                _ => continue,
            };
            let Some(target_class) = self.jar.class(&target_internal) else {
                continue;
            };
            let is_static = Self::member(&target_class, &method_name, &method_desc)
                .map(|member| member.access & 0x0008 != 0)
                .unwrap_or(true);
            let _ = kind;
            return Ok(Some(Val::Lambda {
                class: target_class,
                method: method_name,
                desc: method_desc,
                captured,
                is_static,
            }));
        }
        Err("invokedynamic target is not a lambda or string concat".to_string())
    }
}

fn stringify(value: &Val) -> String {
    match value {
        Val::Str(text) => text.clone(),
        Val::Ident(text) => text.clone(),
        Val::Int(value) => value.to_string(),
        Val::Long(value) => value.to_string(),
        Val::Float(value) => format_f32(*value),
        Val::Double(value) => format!("{value}"),
        Val::ModelLayer(layer) => layer.key(),
        Val::Null => "null".to_string(),
        Val::Direction(name) => (*name).to_string(),
        other => val_name(other).to_string(),
    }
}

fn format_f32(value: f32) -> String {
    format!("{value}")
}

const MTH_SIN_SCALE: f64 = 10430.378350470453;

fn mth_sin_table() -> &'static Vec<f32> {
    static TABLE: std::sync::OnceLock<Vec<f32>> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| {
        (0..65536)
            .map(|index| (((index as f64) / MTH_SIN_SCALE).sin()) as f32)
            .collect()
    })
}

fn mth_sin(value: f64) -> f32 {
    let index = (value * MTH_SIN_SCALE) as i64 & 65535;
    mth_sin_table()[index as usize]
}

fn mth_cos(value: f64) -> f32 {
    let index = (value * MTH_SIN_SCALE + 16384.0) as i64 & 65535;
    mth_sin_table()[index as usize]
}

fn class_name_at(constants: &[Constant], index: usize) -> Option<String> {
    match constants.get(index) {
        Some(Constant::Class(reference)) => {
            let inner: usize = reference.trim_start_matches('#').parse().ok()?;
            Some(classfile::utf8(constants, inner))
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// native semantics of the model builder library
// ---------------------------------------------------------------------------

impl Vm {
    fn native(
        &mut self,
        op: u8,
        owner: &str,
        name: &str,
        desc: &str,
        receiver: Option<&Val>,
        arguments: &[Val],
    ) -> Result<Option<Val>, String> {
        let short = owner.rsplit('/').next().unwrap_or(owner);
        // static factories
        if op == 0xb8 {
            let value = match (short, name) {
                ("CubeListBuilder", "create") => {
                    Some(Val::Cubes(Rc::new(RefCell::new(CubeListState {
                        cubes: Vec::new(),
                        tex_u: 0,
                        tex_v: 0,
                        mirror: false,
                    }))))
                }
                ("PartPose", "offset") => Some(Val::Pose(Rc::new(PartPose::offset(
                    take_f32(&arguments[0])?,
                    take_f32(&arguments[1])?,
                    take_f32(&arguments[2])?,
                )))),
                ("PartPose", "offsetAndRotation") => {
                    Some(Val::Pose(Rc::new(PartPose::offset_rotation(
                        take_f32(&arguments[0])?,
                        take_f32(&arguments[1])?,
                        take_f32(&arguments[2])?,
                        take_f32(&arguments[3])?,
                        take_f32(&arguments[4])?,
                        take_f32(&arguments[5])?,
                    ))))
                }
                ("PartPose", "rotation") => Some(Val::Pose(Rc::new(PartPose {
                    xr: take_f32(&arguments[0])?,
                    yr: take_f32(&arguments[1])?,
                    zr: take_f32(&arguments[2])?,
                    ..PartPose::zero()
                }))),
                ("LayerDefinition", "create") => {
                    let Val::Mesh(mesh) = &arguments[0] else {
                        return Err(format!(
                            "LayerDefinition.create: mesh is {}",
                            val_name(&arguments[0])
                        ));
                    };
                    Some(Val::Layer(Rc::new(LayerState {
                        mesh: mesh.clone(),
                        tex: (take_int(&arguments[1])?, take_int(&arguments[2])?),
                    })))
                }
                ("MeshTransformer", "scaling") => Some(Val::Transformer(take_f32(&arguments[0])?)),
                ("ImmutableMap", "builder") => {
                    Some(Val::MapBuilder(Rc::new(RefCell::new(Vec::new()))))
                }
                ("Maps", "newEnumMap") => match &arguments[0] {
                    Val::Map(map) => Some(Val::Map(map.clone())),
                    other => Some(Val::Map(Rc::new(RefCell::new(vec![(
                        Val::Null,
                        other.clone(),
                    )])))),
                },
                ("Sets", "newHashSet") => Some(Val::MSet(Rc::new(RefCell::new(Vec::new())))),
                ("EnumSet", "allOf") => {
                    Some(Val::DirSet(Rc::new(DIRECTIONS.iter().copied().collect())))
                }
                ("EnumSet", "of") => {
                    let mut directions = Vec::new();
                    for argument in arguments {
                        match argument {
                            Val::Direction(name) => directions.push(*name),
                            Val::Array(items) => {
                                for item in items.borrow().iter() {
                                    if let Val::Direction(name) = item {
                                        directions.push(*name);
                                    }
                                }
                            }
                            other => {
                                return Err(format!("EnumSet.of({})", val_name(other)));
                            }
                        }
                    }
                    Some(Val::DirSet(Rc::new(directions)))
                }
                ("Objects", "requireNonNull") => Some(arguments[0].clone()),
                ("ModelLayers", "getKnownLocations") => Some(Val::Unknown),
                ("RandomSource", "create") | ("RandomSource", "createThreadLocalInstance") => {
                    let seed = if let Some(argument) = arguments.first() {
                        take_long(argument)?
                    } else {
                        0
                    };
                    Some(Val::Random(Rc::new(RefCell::new(LegacyRandom::new(seed)))))
                }
                ("Mth", name) => match name {
                    "sin" => Some(Val::Float(mth_sin(take_f64(&arguments[0])?))),
                    "cos" => Some(Val::Float(mth_cos(take_f64(&arguments[0])?))),
                    "sqrt" => Some(Val::Float(take_f32(&arguments[0])?.sqrt())),
                    "floor" => Some(Val::Int(take_f32(&arguments[0])?.floor() as i32)),
                    "ceil" => Some(Val::Int(take_f32(&arguments[0])?.ceil() as i32)),
                    "abs" => Some(match &arguments[0] {
                        Val::Int(value) => Val::Int(value.abs()),
                        Val::Float(value) => Val::Float(value.abs()),
                        other => return Err(format!("Mth.abs on {}", val_name(other))),
                    }),
                    "clamp" => match (&arguments[0], &arguments[1], &arguments[2]) {
                        (Val::Int(value), Val::Int(min), Val::Int(max)) => {
                            Some(Val::Int((*value).clamp(*min, *max)))
                        }
                        (a, b, c) => {
                            Some(Val::Float(take_f32(a)?.clamp(take_f32(b)?, take_f32(c)?)))
                        }
                    },
                    _ => None,
                },
                ("Util", "allOfEnumExcept") => {
                    let excluded = match &arguments[0] {
                        Val::Direction(name) => *name,
                        _ => "",
                    };
                    Some(Val::DirSet(Rc::new(
                        DIRECTIONS
                            .iter()
                            .copied()
                            .filter(|direction| *direction != excluded)
                            .collect(),
                    )))
                }
                ("Math", name) => match name {
                    "cos" => Some(Val::Double(take_f64(&arguments[0])?.cos())),
                    "sin" => Some(Val::Double(take_f64(&arguments[0])?.sin())),
                    "tan" => Some(Val::Double(take_f64(&arguments[0])?.tan())),
                    "sqrt" => Some(Val::Double(take_f64(&arguments[0])?.sqrt())),
                    "cbrt" => Some(Val::Double(take_f64(&arguments[0])?.cbrt())),
                    "abs" => Some(match &arguments[0] {
                        Val::Int(value) => Val::Int(value.abs()),
                        Val::Long(value) => Val::Long(value.abs()),
                        Val::Float(value) => Val::Float(value.abs()),
                        Val::Double(value) => Val::Double(value.abs()),
                        other => return Err(format!("Math.abs on {}", val_name(other))),
                    }),
                    "floor" => Some(Val::Double(take_f64(&arguments[0])?.floor())),
                    "ceil" => Some(Val::Double(take_f64(&arguments[0])?.ceil())),
                    "round" => Some(Val::Long(take_f64(&arguments[0])?.round() as i64)),
                    "pow" => Some(Val::Double(
                        take_f64(&arguments[0])?.powf(take_f64(&arguments[1])?),
                    )),
                    "atan2" => Some(Val::Double(
                        take_f64(&arguments[0])?.atan2(take_f64(&arguments[1])?),
                    )),
                    "toRadians" => Some(Val::Double(take_f64(&arguments[0])?.to_radians())),
                    "toDegrees" => Some(Val::Double(take_f64(&arguments[0])?.to_degrees())),
                    "min" => Some(match (&arguments[0], &arguments[1]) {
                        (Val::Int(a), Val::Int(b)) => Val::Int(*a.min(b)),
                        (Val::Float(a), Val::Float(b)) => Val::Float(a.min(*b)),
                        (a, b) => Val::Double(take_f64(a)?.min(take_f64(b)?)),
                    }),
                    "max" => Some(match (&arguments[0], &arguments[1]) {
                        (Val::Int(a), Val::Int(b)) => Val::Int(*a.max(b)),
                        (Val::Float(a), Val::Float(b)) => Val::Float(a.max(*b)),
                        (a, b) => Val::Double(take_f64(a)?.max(take_f64(b)?)),
                    }),
                    _ => None,
                },
                ("String", "valueOf") => Some(Val::Str(stringify(&arguments[0]))),
                ("Identifier", "withDefaultNamespace") => Some(Val::Ident(format!(
                    "minecraft:{}",
                    take_str(&arguments[0])?
                ))),
                ("Identifier", "parse") => {
                    let text = take_str(&arguments[0])?;
                    Some(Val::Ident(if text.contains(':') {
                        text
                    } else {
                        format!("minecraft:{text}")
                    }))
                }
                ("Map", "of") => {
                    let mut entries = Vec::new();
                    let mut index = 0;
                    while index + 1 < arguments.len() {
                        entries.push((arguments[index].clone(), arguments[index + 1].clone()));
                        index += 2;
                    }
                    Some(Val::Map(Rc::new(RefCell::new(entries))))
                }
                ("Set", "of") => Some(Val::Set(Rc::new(arguments.to_vec()))),
                _ => None,
            };
            if let Some(value) = value {
                return Ok(Some(value));
            }
            return Ok(None);
        }
        // constructors
        if op == 0xb7 {
            let value = self.construct(short, arguments.to_vec())?;
            return Ok(Some(value));
        }
        // instance methods
        let Some(receiver) = receiver else {
            return Ok(None);
        };
        let value = match (short, name) {
            ("RandomSource", "nextFloat") | ("SingleThreadedRandomSource", "nextFloat") => {
                let Val::Random(random) = receiver else {
                    return Err("nextFloat receiver".to_string());
                };
                Some(Val::Float(random.borrow_mut().next_float()))
            }
            ("RandomSource", "nextDouble") | ("SingleThreadedRandomSource", "nextDouble") => {
                let Val::Random(random) = receiver else {
                    return Err("nextDouble receiver".to_string());
                };
                Some(Val::Double(random.borrow_mut().next_double()))
            }
            ("RandomSource", "nextInt") | ("SingleThreadedRandomSource", "nextInt") => {
                let Val::Random(random) = receiver else {
                    return Err("nextInt receiver".to_string());
                };
                let value = if arguments.is_empty() {
                    random.borrow_mut().next_int()
                } else {
                    random.borrow_mut().next_int_bound(take_int(&arguments[0])?)
                };
                Some(Val::Int(value))
            }
            ("RandomSource", "nextLong") | ("SingleThreadedRandomSource", "nextLong") => {
                let Val::Random(random) = receiver else {
                    return Err("nextLong receiver".to_string());
                };
                Some(Val::Long(random.borrow_mut().next_long()))
            }
            ("RandomSource", "nextBoolean") | ("SingleThreadedRandomSource", "nextBoolean") => {
                let Val::Random(random) = receiver else {
                    return Err("nextBoolean receiver".to_string());
                };
                Some(Val::Int((random.borrow_mut().next(1) != 0) as i32))
            }
            ("CubeListBuilder", "texOffs") => {
                if let Val::Cubes(builder) = receiver {
                    let mut builder = builder.borrow_mut();
                    builder.tex_u = take_int(&arguments[0])?;
                    builder.tex_v = take_int(&arguments[1])?;
                }
                Some(receiver.clone())
            }
            ("CubeListBuilder", "mirror") => {
                if let Val::Cubes(builder) = receiver {
                    let flag = if arguments.is_empty() {
                        true
                    } else {
                        take_int(&arguments[0])? != 0
                    };
                    builder.borrow_mut().mirror = flag;
                }
                Some(receiver.clone())
            }
            ("CubeListBuilder", "addBox") => {
                self.add_box(receiver, desc, arguments)?;
                Some(receiver.clone())
            }
            ("PartDefinition", "addOrReplaceChild") => {
                let Val::Part(parent) = receiver else {
                    return Err("addOrReplaceChild receiver".to_string());
                };
                let name = take_str(&arguments[0])?;
                let child = if arguments.len() == 2 {
                    let Val::Part(child) = &arguments[1] else {
                        return Err("addOrReplaceChild: child is not a PartDefinition".to_string());
                    };
                    child.clone()
                } else {
                    let Val::Cubes(cubes) = &arguments[1] else {
                        return Err("addOrReplaceChild: cube builder".to_string());
                    };
                    let Val::Pose(pose) = &arguments[2] else {
                        return Err("addOrReplaceChild: pose".to_string());
                    };
                    Rc::new(RefCell::new(PartState {
                        cubes: cubes.borrow().cubes.clone(),
                        pose: **pose,
                        children: Vec::new(),
                    }))
                };
                let replaced = {
                    let mut parent = parent.borrow_mut();
                    let mut previous = None;
                    if let Some(slot) = parent.children.iter_mut().find(|(key, _)| *key == name) {
                        previous = Some(slot.1.clone());
                        slot.1 = child.clone();
                    } else {
                        parent.children.push((name, child.clone()));
                    }
                    previous
                };
                if let Some(previous) = replaced {
                    let mut previous = previous.borrow_mut();
                    let mut child = child.borrow_mut();
                    let inherited: Vec<_> = previous.children.drain(..).collect();
                    for entry in inherited {
                        if !child.children.iter().any(|(key, _)| *key == entry.0) {
                            child.children.push(entry);
                        }
                    }
                }
                Some(Val::Part(child))
            }
            ("PartDefinition", "getChild") => {
                let Val::Part(parent) = receiver else {
                    return Err("getChild receiver".to_string());
                };
                let name = take_str(&arguments[0])?;
                let child = parent
                    .borrow()
                    .children
                    .iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, child)| child.clone())
                    .ok_or_else(|| format!("PartDefinition.getChild({name}): not found"))?;
                Some(Val::Part(child))
            }
            ("PartDefinition", "clearChild") => {
                let Val::Part(parent) = receiver else {
                    return Err("clearChild receiver".to_string());
                };
                let name = take_str(&arguments[0])?;
                Some(Val::Part(clear_child(parent, &name)))
            }
            ("PartDefinition", "clearRecursively") => {
                let Val::Part(part) = receiver else {
                    return Err("clearRecursively receiver".to_string());
                };
                clear_recursively(part);
                Some(Val::Part(part.clone()))
            }
            ("PartDefinition", "retainPartsAndChildren") => {
                let Val::Part(part) = receiver else {
                    return Err("retain receiver".to_string());
                };
                let keep = take_set(&arguments[0])?;
                retain_parts_and_children(part, &keep);
                Some(Val::Null)
            }
            ("PartDefinition", "retainExactParts") => {
                let Val::Part(part) = receiver else {
                    return Err("retain receiver".to_string());
                };
                let keep = take_set(&arguments[0])?;
                retain_exact_parts(part, &keep);
                Some(Val::Null)
            }
            ("PartDefinition", "transformed") => {
                let Val::Part(part) = receiver else {
                    return Err("transformed receiver".to_string());
                };
                let operator = arguments[0].clone();
                let pose = self.apply_operator(&operator, part.borrow().pose)?;
                let (cubes, children) = {
                    let part = part.borrow();
                    (part.cubes.clone(), part.children.clone())
                };
                Some(Val::Part(Rc::new(RefCell::new(PartState {
                    cubes,
                    pose,
                    children,
                }))))
            }
            ("PartDefinition", "bake") => {
                let Val::Part(part) = receiver else {
                    return Err("bake receiver".to_string());
                };
                // Baked lazily by the exporter; hand back the builder part.
                Some(Val::Part(part.clone()))
            }
            ("MeshDefinition", "getRoot") => {
                let Val::Mesh(mesh) = receiver else {
                    return Err("getRoot receiver".to_string());
                };
                Some(Val::Part(mesh.borrow().root.clone()))
            }
            ("MeshDefinition", "transformed") => {
                let Val::Mesh(mesh) = receiver else {
                    return Err("transformed receiver".to_string());
                };
                let operator = arguments[0].clone();
                let root = mesh.borrow().root.clone();
                let pose = self.apply_operator(&operator, root.borrow().pose)?;
                let (cubes, children) = {
                    let root = root.borrow();
                    (root.cubes.clone(), root.children.clone())
                };
                let new_root = Rc::new(RefCell::new(PartState {
                    cubes,
                    pose,
                    children,
                }));
                Some(Val::Mesh(Rc::new(RefCell::new(MeshState {
                    root: new_root,
                }))))
            }
            ("MeshTransformer", "apply") => match receiver {
                Val::BabyTransform(baby) => Some(baby_transform(
                    baby,
                    match &arguments[0] {
                        Val::Mesh(mesh) => mesh.clone(),
                        other => return Err(format!("apply argument is {}", val_name(other))),
                    },
                )),
                Val::Transformer(factor) => {
                    let mesh = match &arguments[0] {
                        Val::Mesh(mesh) => mesh.clone(),
                        other => return Err(format!("apply argument is {}", val_name(other))),
                    };
                    let offset = SCALING_BASE * (1.0 - factor);
                    let root = mesh.borrow().root.clone();
                    let (cubes, children) = {
                        let root = root.borrow();
                        (root.cubes.clone(), root.children.clone())
                    };
                    let pose = root
                        .borrow()
                        .pose
                        .scaled(*factor)
                        .translated(0.0, offset, 0.0);
                    Some(Val::Mesh(Rc::new(RefCell::new(MeshState {
                        root: Rc::new(RefCell::new(PartState {
                            cubes,
                            pose,
                            children,
                        })),
                    }))))
                }
                _other => Some(Val::Unknown),
            },
            ("MeshDefinition", "apply") => {
                let Val::Mesh(mesh) = receiver else {
                    return Err("apply receiver".to_string());
                };
                let result = self.apply_transformer(&arguments[0], mesh.clone())?;
                Some(result)
            }
            ("LayerDefinition", "apply") => {
                let Val::Layer(layer) = receiver else {
                    return Err("LayerDefinition.apply receiver".to_string());
                };
                let mesh = self.apply_transformer(&arguments[0], layer.mesh.clone())?;
                let Val::Mesh(mesh) = mesh else {
                    return Err(
                        "LayerDefinition.apply: transformer did not return a mesh".to_string()
                    );
                };
                Some(Val::Layer(Rc::new(LayerState {
                    mesh,
                    tex: layer.tex,
                })))
            }
            ("PartPose", "translated") => {
                let Val::Pose(pose) = receiver else {
                    return Err("translated receiver".to_string());
                };
                Some(Val::Pose(Rc::new(pose.translated(
                    take_f32(&arguments[0])?,
                    take_f32(&arguments[1])?,
                    take_f32(&arguments[2])?,
                ))))
            }
            ("PartPose", "scaled") => {
                let Val::Pose(pose) = receiver else {
                    return Err("scaled receiver".to_string());
                };
                let value = if arguments.len() == 1 {
                    let f = take_f32(&arguments[0])?;
                    // `scaled(f)` returns `this` when f == 1 (checked in bytecode).
                    if f == 1.0 {
                        Val::Pose(pose.clone())
                    } else {
                        Val::Pose(Rc::new(pose.scaled(f)))
                    }
                } else {
                    Val::Pose(Rc::new(pose.scaled_xyz(
                        take_f32(&arguments[0])?,
                        take_f32(&arguments[1])?,
                        take_f32(&arguments[2])?,
                    )))
                };
                Some(value)
            }
            ("PartPose", "withScale") => {
                let Val::Pose(pose) = receiver else {
                    return Err("withScale receiver".to_string());
                };
                Some(Val::Pose(Rc::new(
                    pose.with_scale(take_f32(&arguments[0])?),
                )))
            }
            ("PartPose", "x") | ("PartPose", "y") | ("PartPose", "z") => {
                let Val::Pose(pose) = receiver else {
                    return Err("pose accessor receiver".to_string());
                };
                Some(Val::Float(match name {
                    "x" => pose.x,
                    "y" => pose.y,
                    _ => pose.z,
                }))
            }
            ("CubeDeformation", "extend") => {
                let Val::Deform(grow) = receiver else {
                    return Err("extend receiver".to_string());
                };
                let (dx, dy, dz) = if arguments.len() == 1 {
                    let f = take_f32(&arguments[0])?;
                    (f, f, f)
                } else {
                    (
                        take_f32(&arguments[0])?,
                        take_f32(&arguments[1])?,
                        take_f32(&arguments[2])?,
                    )
                };
                Some(Val::Deform(Rc::new([
                    grow[0] + dx,
                    grow[1] + dy,
                    grow[2] + dz,
                ])))
            }
            ("ArmorModelSet", "map") => {
                let Val::ArmorSet(items) = receiver else {
                    return Err("ArmorModelSet.map receiver".to_string());
                };
                let function = arguments[0].clone();
                let mut mapped = Vec::with_capacity(items.len());
                for item in items.iter() {
                    mapped.push(self.apply_value(&function, item.clone())?);
                }
                Some(Val::ArmorSet(Rc::new(mapped)))
            }
            ("ArmorModelSet", "putFrom") => {
                let Val::ArmorSet(items) = receiver else {
                    return Err("putFrom receiver".to_string());
                };
                let Val::ArmorSet(other) = &arguments[0] else {
                    return Err("putFrom argument".to_string());
                };
                let Val::MapBuilder(builder) = &arguments[1] else {
                    return Err("putFrom builder".to_string());
                };
                let mut builder = builder.borrow_mut();
                for index in 0..items.len().min(other.len()) {
                    builder.push((items[index].clone(), other[index].clone()));
                }
                Some(Val::Null)
            }
            ("ArmorModelSet", "head")
            | ("ArmorModelSet", "chest")
            | ("ArmorModelSet", "legs")
            | ("ArmorModelSet", "feet") => {
                let Val::ArmorSet(items) = receiver else {
                    return Err("ArmorModelSet accessor receiver".to_string());
                };
                let slot = match name {
                    "head" => 0,
                    "chest" => 1,
                    "legs" => 2,
                    _ => 3,
                };
                Some(items.get(slot).cloned().unwrap_or(Val::Null))
            }
            ("ArmorModelSet", "get") => {
                let Val::ArmorSet(items) = receiver else {
                    return Err("ArmorModelSet.get receiver".to_string());
                };
                let slot = match &arguments[0] {
                    Val::Direction(name) => match *name {
                        "HEAD" => 0,
                        "CHEST" => 1,
                        "LEGS" => 2,
                        _ => 3,
                    },
                    _ => 0,
                };
                Some(items.get(slot).cloned().unwrap_or(Val::Null))
            }
            ("ImmutableMap$Builder", "put") => {
                let Val::MapBuilder(builder) = receiver else {
                    return Err("put receiver".to_string());
                };
                builder
                    .borrow_mut()
                    .push((arguments[0].clone(), arguments[1].clone()));
                Some(receiver.clone())
            }
            ("ImmutableMap$Builder", "build") => {
                let Val::MapBuilder(builder) = receiver else {
                    return Err("build receiver".to_string());
                };
                Some(Val::Map(Rc::new(RefCell::new(builder.borrow().clone()))))
            }
            ("ImmutableMap", "containsKey") | ("Map", "containsKey") => {
                let entries = match receiver {
                    Val::Map(map) => map.borrow().clone(),
                    _ => return Err("containsKey receiver".to_string()),
                };
                let found = entries.iter().any(|(key, _)| val_eq(key, &arguments[0]));
                Some(Val::Int(found as i32))
            }
            ("Map", "get") => {
                let entries = match receiver {
                    Val::Map(map) => map.borrow().clone(),
                    _ => return Err("get receiver".to_string()),
                };
                let found = entries
                    .iter()
                    .find(|(key, _)| val_eq(key, &arguments[0]))
                    .map(|(_, value)| value.clone())
                    .unwrap_or(Val::Null);
                Some(found)
            }
            ("Map", "put") => {
                let Val::Map(map) = receiver else {
                    return Err("Map.put receiver".to_string());
                };
                let mut map = map.borrow_mut();
                if let Some(slot) = map.iter_mut().find(|(key, _)| val_eq(key, &arguments[0])) {
                    slot.1 = arguments[1].clone();
                } else {
                    map.push((arguments[0].clone(), arguments[1].clone()));
                }
                Some(Val::Null)
            }
            ("List", "add") => {
                let Val::MList(list) = receiver else {
                    return Err("List.add receiver".to_string());
                };
                list.borrow_mut().push(arguments[0].clone());
                Some(Val::Int(1))
            }
            ("List", "isEmpty") => {
                let empty = match receiver {
                    Val::MList(list) => list.borrow().is_empty(),
                    Val::Array(items) => items.borrow().is_empty(),
                    Val::Set(items) => items.is_empty(),
                    // The validation tail's stream is not modelled; report it as
                    // empty so the IllegalStateException branch is not taken.
                    _ => true,
                };
                Some(Val::Int(empty as i32))
            }
            ("Set", "add") => {
                let Val::MSet(set) = receiver else {
                    return Err("Set.add receiver".to_string());
                };
                let mut set = set.borrow_mut();
                if set.iter().any(|item| val_eq(item, &arguments[0])) {
                    Some(Val::Int(0))
                } else {
                    set.push(arguments[0].clone());
                    Some(Val::Int(1))
                }
            }
            ("Set", "contains") => {
                let found = match receiver {
                    Val::Set(items) => items.iter().any(|item| val_eq(item, &arguments[0])),
                    Val::MSet(items) => items
                        .borrow()
                        .iter()
                        .any(|item| val_eq(item, &arguments[0])),
                    Val::DirSet(items) => match &arguments[0] {
                        Val::Direction(name) => items.iter().any(|item| item == name),
                        _ => false,
                    },
                    _ => false,
                };
                Some(Val::Int(found as i32))
            }
            ("Set", "size") | ("Set<Direction>", "size") => {
                let size = match receiver {
                    Val::Set(items) => items.len(),
                    Val::MSet(items) => items.borrow().len(),
                    Val::DirSet(items) => items.len(),
                    _ => 0,
                };
                Some(Val::Int(size as i32))
            }
            ("ModelLayerLocation", "toString") => match receiver {
                Val::ModelLayer(layer) => Some(Val::Str(layer.key())),
                _ => None,
            },
            ("ModelLayerLocation", "model") => match receiver {
                Val::ModelLayer(layer) => Some(Val::Ident(layer.model.clone())),
                _ => None,
            },
            ("ModelLayerLocation", "layer") => match receiver {
                Val::ModelLayer(layer) => Some(Val::Str(layer.layer.clone())),
                _ => None,
            },
            ("Identifier", "toString") => match receiver {
                Val::Ident(text) => Some(Val::Str(text.clone())),
                _ => None,
            },
            ("Vector3f", "x") | ("Vector3f", "y") | ("Vector3f", "z") => {
                let Val::Vec3(values) = receiver else {
                    return Err("Vector3f accessor receiver".to_string());
                };
                Some(Val::Float(match name {
                    "x" => values[0],
                    "y" => values[1],
                    _ => values[2],
                }))
            }
            _ => None,
        };
        Ok(value)
    }

    fn construct(&mut self, owner: &str, arguments: Vec<Val>) -> Result<Val, String> {
        let short = owner.rsplit('/').next().unwrap_or(owner);
        match short {
            "CubeDeformation" => match arguments.len() {
                1 => {
                    let f = take_f32(&arguments[0])?;
                    Ok(Val::Deform(Rc::new([f, f, f])))
                }
                3 => Ok(Val::Deform(Rc::new([
                    take_f32(&arguments[0])?,
                    take_f32(&arguments[1])?,
                    take_f32(&arguments[2])?,
                ]))),
                _ => Err(format!("CubeDeformation({} args)", arguments.len())),
            },
            "PartPose" => {
                if arguments.len() != 9 {
                    return Err(format!("PartPose({} args)", arguments.len()));
                }
                Ok(Val::Pose(Rc::new(PartPose {
                    x: take_f32(&arguments[0])?,
                    y: take_f32(&arguments[1])?,
                    z: take_f32(&arguments[2])?,
                    xr: take_f32(&arguments[3])?,
                    yr: take_f32(&arguments[4])?,
                    zr: take_f32(&arguments[5])?,
                    xs: take_f32(&arguments[6])?,
                    ys: take_f32(&arguments[7])?,
                    zs: take_f32(&arguments[8])?,
                })))
            }
            "MeshDefinition" => Ok(Val::Mesh(Rc::new(RefCell::new(MeshState {
                root: Rc::new(RefCell::new(PartState {
                    cubes: Vec::new(),
                    pose: PartPose::zero(),
                    children: Vec::new(),
                })),
            })))),
            "CubeListBuilder" => Ok(Val::Cubes(Rc::new(RefCell::new(CubeListState {
                cubes: Vec::new(),
                tex_u: 0,
                tex_v: 0,
                mirror: false,
            })))),
            "MaterialDefinition" => Ok(Val::Material(Rc::new((
                take_int(&arguments[0])?,
                take_int(&arguments[1])?,
            )))),
            "LayerDefinition" => {
                let Val::Mesh(mesh) = &arguments[0] else {
                    return Err("LayerDefinition(<init>): mesh".to_string());
                };
                let Val::Material(material) = &arguments[1] else {
                    return Err("LayerDefinition(<init>): material".to_string());
                };
                Ok(Val::Layer(Rc::new(LayerState {
                    mesh: mesh.clone(),
                    tex: **material,
                })))
            }
            "ModelLayerLocation" => {
                let model = match &arguments[0] {
                    Val::Ident(text) => text.clone(),
                    other => {
                        return Err(format!("ModelLayerLocation model is {}", val_name(other)))
                    }
                };
                Ok(Val::ModelLayer(Rc::new(ModelLayerLocation {
                    model,
                    layer: take_str(&arguments[1])?,
                })))
            }
            "ArmorModelSet" => {
                if arguments.len() != 4 {
                    return Err(format!("ArmorModelSet({} args)", arguments.len()));
                }
                Ok(Val::ArmorSet(Rc::new(arguments)))
            }
            "BabyModelTransform" => {
                let (scale_head, y_head, z_head, head_scale, body_scale, body_y, set) =
                    match arguments.len() {
                        1 => (false, 5.0, 2.0, 2.0, 2.0, 24.0, arguments[0].clone()),
                        4 => (
                            take_int(&arguments[0])? != 0,
                            take_f32(&arguments[1])?,
                            take_f32(&arguments[2])?,
                            2.0,
                            2.0,
                            24.0,
                            arguments[3].clone(),
                        ),
                        7 => (
                            take_int(&arguments[0])? != 0,
                            take_f32(&arguments[1])?,
                            take_f32(&arguments[2])?,
                            take_f32(&arguments[3])?,
                            take_f32(&arguments[4])?,
                            take_f32(&arguments[5])?,
                            arguments[6].clone(),
                        ),
                        count => {
                            return Err(format!("BabyModelTransform({count} args)"));
                        }
                    };
                Ok(Val::BabyTransform(Rc::new(BabyTransform {
                    scale_head,
                    y_head,
                    z_head,
                    head_scale,
                    body_scale,
                    body_y,
                    head_parts: set,
                })))
            }
            "Vector3f" => {
                if arguments.is_empty() {
                    return Ok(Val::Vec3([0.0, 0.0, 0.0]));
                }
                Ok(Val::Vec3([
                    take_f32(&arguments[0])?,
                    take_f32(&arguments[1])?,
                    take_f32(&arguments[2])?,
                ]))
            }
            "UVPair" => Ok(Val::Vec3([
                take_f32(&arguments[0])?,
                take_f32(&arguments[1])?,
                0.0,
            ])),
            "EntityModelSet" => Ok(Val::Unknown),
            "PartDefinition" => {
                let Val::Pose(pose) = &arguments[1] else {
                    return Err("PartDefinition(<init>): pose".to_string());
                };
                Ok(Val::Part(Rc::new(RefCell::new(PartState {
                    cubes: Vec::new(),
                    pose: **pose,
                    children: Vec::new(),
                }))))
            }
            other => Err(format!("unhandled constructor {other}")),
        }
    }

    fn add_box(&mut self, receiver: &Val, desc: &str, arguments: &[Val]) -> Result<(), String> {
        let types = classfile::descriptor_types(desc);
        let params = &types[..types.len().saturating_sub(1)];
        let mut index = 0usize;
        let mut coords = [0f32; 6];
        let mut seen = 0usize;
        let mut grow = None;
        let mut mirror_override = None;
        let mut visible = None;
        let mut scale = None;
        let mut tex_override = None;
        let mut position = 0usize;
        while position < params.len() && index < arguments.len() {
            match params[position].as_str() {
                "Ljava/lang/String;" => {
                    // addBox comment
                }
                "F" | "I" if seen < 6 => {
                    coords[seen] = take_f32(&arguments[index])?;
                    seen += 1;
                }
                kind if kind.ends_with("CubeDeformation;") => {
                    let Val::Deform(values) = &arguments[index] else {
                        return Err(format!(
                            "addBox deformation argument: desc {} index {} args {:?}",
                            desc,
                            index,
                            arguments.iter().map(val_name).collect::<Vec<_>>()
                        ));
                    };
                    grow = Some(**values);
                }
                "Z" => {
                    mirror_override = Some(take_int(&arguments[index])? != 0);
                }
                "Ljava/util/Set;" => {
                    visible = Some(take_dir_set(&arguments[index])?);
                }
                "I" => {
                    let u = take_int(&arguments[index])?;
                    let v = if index + 1 < arguments.len() {
                        take_int(&arguments[index + 1])?
                    } else {
                        0
                    };
                    tex_override = Some((u, v));
                    index += 1;
                    position += 1;
                }
                "F" => {
                    let u = take_f32(&arguments[index])?;
                    let v = if index + 1 < arguments.len() {
                        take_f32(&arguments[index + 1])?
                    } else {
                        1.0
                    };
                    scale = Some([u, v]);
                    index += 1;
                    position += 1;
                }
                _ => {}
            }
            index += 1;
            position += 1;
        }
        let Val::Cubes(builder) = receiver else {
            return Err("addBox receiver".to_string());
        };
        let mut builder = builder.borrow_mut();
        let tex = tex_override.unwrap_or((builder.tex_u, builder.tex_v));
        let cube = CubeDef {
            tex: (tex.0 as f32, tex.1 as f32),
            origin: [coords[0], coords[1], coords[2]],
            dims: [coords[3], coords[4], coords[5]],
            grow: grow.unwrap_or([0.0; 3]),
            mirror: mirror_override.unwrap_or(builder.mirror),
            scale: scale.unwrap_or([1.0, 1.0]),
            visible: visible.unwrap_or_else(|| DIRECTIONS.to_vec()),
        };
        builder.cubes.push(cube);
        Ok(())
    }
    fn apply_operator(&mut self, operator: &Val, input: PartPose) -> Result<PartPose, String> {
        let value = self.apply_value(operator, Val::Pose(Rc::new(input)))?;
        match value {
            Val::Pose(pose) => Ok(*pose),
            other => Err(format!("operator returned {}", val_name(&other))),
        }
    }

    fn apply_value(&mut self, function: &Val, argument: Val) -> Result<Val, String> {
        let Val::Lambda {
            class,
            method,
            desc,
            captured,
            is_static,
        } = function
        else {
            return Err(format!("cannot apply {}", val_name(function)));
        };
        let mut all = captured.clone();
        all.push(argument);
        let member = Self::member(class, method, desc)
            .ok_or_else(|| format!("lambda {}.{} is missing", class.name, method))?;
        let (receiver, args) = if *is_static {
            (None, all)
        } else {
            let receiver = all.remove(0);
            (Some(receiver), all)
        };
        let class = class.clone();
        self.call(class, &member, args, receiver)
    }

    fn apply_transformer(
        &mut self,
        transformer: &Val,
        mesh: Rc<RefCell<MeshState>>,
    ) -> Result<Val, String> {
        match transformer {
            Val::Transformer(factor) => {
                let offset = SCALING_BASE * (1.0 - factor);
                let root = mesh.borrow().root.clone();
                let (cubes, children) = {
                    let root = root.borrow();
                    (root.cubes.clone(), root.children.clone())
                };
                let pose = root
                    .borrow()
                    .pose
                    .scaled(*factor)
                    .translated(0.0, offset, 0.0);
                let new_root = Rc::new(RefCell::new(PartState {
                    cubes,
                    pose,
                    children,
                }));
                Ok(Val::Mesh(Rc::new(RefCell::new(MeshState {
                    root: new_root,
                }))))
            }
            Val::BabyTransform(baby) => Ok(baby_transform(baby, mesh)),
            Val::Lambda { .. } => self.apply_value(transformer, Val::Mesh(mesh)),
            other => Err(format!("cannot apply transformer {}", val_name(other))),
        }
    }
}

fn baby_transform(baby: &BabyTransform, mesh: Rc<RefCell<MeshState>>) -> Val {
    let head_factor = if baby.scale_head {
        1.5 / baby.head_scale
    } else {
        1.0
    };
    let body_factor = 1.0 / baby.body_scale;
    let root = mesh.borrow().root.clone();
    let children = root.borrow().children.clone();
    let mut new_root = PartState {
        cubes: Vec::new(),
        pose: PartPose::zero(),
        children: Vec::new(),
    };
    for (name, child) in children {
        let is_head = match &baby.head_parts {
            Val::Set(items) => items.iter().any(|item| match item {
                Val::Str(text) => *text == name,
                _ => false,
            }),
            Val::MSet(items) => items.borrow().iter().any(|item| match item {
                Val::Str(text) => *text == name,
                _ => false,
            }),
            _ => false,
        };
        let pose = child.borrow().pose;
        let transformed = if is_head {
            pose.translated(0.0, baby.y_head, baby.z_head)
                .scaled(head_factor)
        } else {
            pose.translated(0.0, baby.body_y, 0.0).scaled(body_factor)
        };
        let (cubes, grandchildren) = {
            let child = child.borrow();
            (child.cubes.clone(), child.children.clone())
        };
        new_root.children.push((
            name,
            Rc::new(RefCell::new(PartState {
                cubes,
                pose: transformed,
                children: grandchildren,
            })),
        ));
    }
    Val::Mesh(Rc::new(RefCell::new(MeshState {
        root: Rc::new(RefCell::new(new_root)),
    })))
}

/// `PartDefinition.clearRecursively`: `clearChild` every child (which replaces
/// the child with an empty-cube part that inherits its children) and recurse.
/// The node's own cubes are kept; every descendant's cubes are removed.
fn clear_recursively(part: &Rc<RefCell<PartState>>) {
    let names: Vec<String> = part
        .borrow()
        .children
        .iter()
        .map(|(name, _)| name.clone())
        .collect();
    for name in names {
        let replacement = clear_child(part, &name);
        clear_recursively(&replacement);
    }
}

/// `PartDefinition.clearChild`: replace the child with an empty-cube part that
/// keeps the child's pose and children.
fn clear_child(part: &Rc<RefCell<PartState>>, name: &str) -> Rc<RefCell<PartState>> {
    let (pose, grandchildren) = {
        let part = part.borrow();
        let Some(child) = part.children.iter().find(|(key, _)| key == name) else {
            return Rc::new(RefCell::new(PartState::default()));
        };
        let child = child.1.borrow();
        (child.pose, child.children.clone())
    };
    let replacement = Rc::new(RefCell::new(PartState {
        cubes: Vec::new(),
        pose,
        children: grandchildren,
    }));
    {
        let mut part = part.borrow_mut();
        if let Some(slot) = part.children.iter_mut().find(|(key, _)| key == name) {
            slot.1 = replacement.clone();
        }
    }
    replacement
}

fn take_set(value: &Val) -> Result<Vec<String>, String> {
    match value {
        Val::Set(items) => Ok(items
            .iter()
            .map(|item| match item {
                Val::Str(text) => Ok(text.clone()),
                other => Err(format!("set member is {}", val_name(other))),
            })
            .collect::<Result<Vec<_>, String>>()?),
        other => Err(format!("expected Set, got {}", val_name(other))),
    }
}

fn take_dir_set(value: &Val) -> Result<Vec<&'static str>, String> {
    match value {
        Val::DirSet(items) => Ok(items.as_ref().clone()),
        Val::Set(items) => Ok(items
            .iter()
            .filter_map(|item| match item {
                Val::Direction(name) => Some(*name),
                _ => None,
            })
            .collect()),
        other => Err(format!("expected Set<Direction>, got {}", val_name(other))),
    }
}

fn retain_parts_and_children(part: &Rc<RefCell<PartState>>, keep: &[String]) {
    let children: Vec<(String, Rc<RefCell<PartState>>)> = part.borrow().children.clone();
    for (name, child) in children {
        if keep.contains(&name) {
            continue;
        }
        let (pose, grandchildren) = {
            let child = child.borrow();
            (child.pose, child.children.clone())
        };
        let replacement = Rc::new(RefCell::new(PartState {
            cubes: Vec::new(),
            pose,
            children: grandchildren,
        }));
        {
            let mut part = part.borrow_mut();
            if let Some(slot) = part.children.iter_mut().find(|(key, _)| *key == name) {
                slot.1 = replacement.clone();
            }
        }
        retain_parts_and_children(&replacement, keep);
    }
}

fn retain_exact_parts(part: &Rc<RefCell<PartState>>, keep: &[String]) {
    let children: Vec<(String, Rc<RefCell<PartState>>)> = part.borrow().children.clone();
    for (name, child) in children {
        if keep.contains(&name) {
            clear_recursively(&child);
            continue;
        }
        let (pose, grandchildren) = {
            let child = child.borrow();
            (child.pose, child.children.clone())
        };
        let replacement = Rc::new(RefCell::new(PartState {
            cubes: Vec::new(),
            pose,
            children: grandchildren,
        }));
        {
            let mut part = part.borrow_mut();
            if let Some(slot) = part.children.iter_mut().find(|(key, _)| *key == name) {
                slot.1 = replacement.clone();
            }
        }
        retain_exact_parts(&replacement, keep);
    }
}

// ---------------------------------------------------------------------------
// bake: ModelPart.Cube / Polygon, from the vanilla bytecode
// ---------------------------------------------------------------------------

fn unit_vector(direction: &str) -> [f32; 3] {
    match direction {
        "DOWN" => [0.0, -1.0, 0.0],
        "UP" => [0.0, 1.0, 0.0],
        "NORTH" => [0.0, 0.0, -1.0],
        "SOUTH" => [0.0, 0.0, 1.0],
        "WEST" => [-1.0, 0.0, 0.0],
        "EAST" => [1.0, 0.0, 0.0],
        _ => [0.0, 0.0, 0.0],
    }
}

fn mirror_facing(direction: &str) -> &'static str {
    match direction {
        "WEST" => "EAST",
        "EAST" => "WEST",
        other => match other {
            "DOWN" => "DOWN",
            "UP" => "UP",
            "NORTH" => "NORTH",
            "SOUTH" => "SOUTH",
            _ => "DOWN",
        },
    }
}

struct BakedPolygon {
    vertices: Vec<[f32; 5]>,
    normal: [f32; 3],
}

fn bake_cube(cube: &CubeDef, tex_size: (i32, i32)) -> Vec<BakedPolygon> {
    let mut x = cube.origin[0];
    let mut y = cube.origin[1];
    let mut z = cube.origin[2];
    let mut x2 = x + cube.dims[0];
    let mut y2 = y + cube.dims[1];
    let mut z2 = z + cube.dims[2];
    x -= cube.grow[0];
    y -= cube.grow[1];
    z -= cube.grow[2];
    x2 += cube.grow[0];
    y2 += cube.grow[1];
    z2 += cube.grow[2];
    if cube.mirror {
        std::mem::swap(&mut x, &mut x2);
    }
    let v0 = [x, y, z];
    let v1 = [x2, y, z];
    let v2 = [x2, y2, z];
    let v3 = [x, y2, z];
    let v4 = [x, y, z2];
    let v5 = [x2, y, z2];
    let v6 = [x2, y2, z2];
    let v7 = [x, y2, z2];
    let tex_u = cube.tex.0 as i32 as f32;
    let tex_v = cube.tex.1 as i32 as f32;
    let dx = cube.dims[0];
    let dy = cube.dims[1];
    let dz = cube.dims[2];
    let u0 = tex_u;
    let u1 = tex_u + dz;
    let u2 = tex_u + dz + dx;
    let u3 = tex_u + dz + dx + dx;
    let u4 = tex_u + dz + dx + dz;
    let u5 = u4 + dx;
    let w0 = tex_v;
    let w1 = tex_v + dz;
    let w2 = tex_v + dz + dy;
    let scale_u = tex_size.0 as f32 * cube.scale[0];
    let scale_v = tex_size.1 as f32 * cube.scale[1];
    // The face order the Cube constructor appends polygons in.
    let faces: [(&str, [&[f32; 3]; 4], f32, f32, f32, f32); 6] = [
        ("DOWN", [&v5, &v4, &v0, &v1], u1, w0, u2, w1),
        ("UP", [&v2, &v3, &v7, &v6], u2, w1, u3, w0),
        ("WEST", [&v0, &v4, &v7, &v3], u0, w1, u1, w2),
        ("NORTH", [&v1, &v0, &v3, &v2], u1, w1, u2, w2),
        ("EAST", [&v5, &v1, &v2, &v6], u2, w1, u4, w2),
        ("SOUTH", [&v4, &v5, &v6, &v7], u4, w1, u5, w2),
    ];
    let mut polygons = Vec::new();
    for (direction, corners, a0, b0, a1, b1) in faces {
        if !cube.visible.iter().any(|visible| *visible == direction) {
            continue;
        }
        let mut vertices: Vec<[f32; 5]> = corners
            .iter()
            .map(|corner| [corner[0], corner[1], corner[2], 0.0, 0.0])
            .collect();
        let u_left = if scale_u == 0.0 { 0.0 } else { a1 / scale_u };
        let v_top = if scale_v == 0.0 { 0.0 } else { b0 / scale_v };
        let u_right = if scale_u == 0.0 { 0.0 } else { a0 / scale_u };
        let v_bottom = if scale_v == 0.0 { 0.0 } else { b1 / scale_v };
        vertices[0][3] = u_left;
        vertices[0][4] = v_top;
        vertices[1][3] = u_right;
        vertices[1][4] = v_top;
        vertices[2][3] = u_right;
        vertices[2][4] = v_bottom;
        vertices[3][3] = u_left;
        vertices[3][4] = v_bottom;
        if cube.mirror {
            vertices.swap(0, 3);
            vertices.swap(1, 2);
        }
        let facing = if cube.mirror {
            mirror_facing(direction)
        } else {
            direction
        };
        let normal = unit_vector(facing);
        polygons.push(BakedPolygon { vertices, normal });
    }
    polygons
}

fn f32_value(value: f32) -> Value {
    if !value.is_finite() {
        return Value::Number(serde_json::Number::from_f64(0.0).unwrap());
    }
    let text = format!("{value}");
    let number: f64 = text.parse().unwrap_or(0.0);
    Value::Number(serde_json::Number::from_f64(number).expect("finite"))
}

fn vector_values(component: f32) -> Value {
    f32_value(component)
}

fn part_json(part: &Rc<RefCell<PartState>>, tex_size: (i32, i32)) -> Value {
    let part = part.borrow();
    let mut object = Map::new();
    object.insert(
        "pose".into(),
        Value::Array(vec![
            f32_value(part.pose.x),
            f32_value(part.pose.y),
            f32_value(part.pose.z),
            f32_value(part.pose.xr),
            f32_value(part.pose.yr),
            f32_value(part.pose.zr),
            f32_value(part.pose.xs),
            f32_value(part.pose.ys),
            f32_value(part.pose.zs),
        ]),
    );
    let mut polygons = Vec::new();
    let mut normals = Vec::new();
    for cube in &part.cubes {
        for polygon in bake_cube(cube, tex_size) {
            let vertices = polygon
                .vertices
                .iter()
                .map(|vertex| {
                    Value::Array(
                        vertex
                            .iter()
                            .map(|component| f32_value(*component))
                            .collect(),
                    )
                })
                .collect();
            polygons.push(Value::Array(vertices));
            normals.push(Value::Array(
                polygon
                    .normal
                    .iter()
                    .map(|component| vector_values(*component))
                    .collect(),
            ));
        }
    }
    object.insert("polygons".into(), Value::Array(polygons));
    object.insert("normals".into(), Value::Array(normals));
    let mut children = Map::new();
    for (name, child) in &part.children {
        children.insert(name.clone(), part_json(child, tex_size));
    }
    object.insert("children".into(), Value::Object(children));
    Value::Object(object)
}

// ---------------------------------------------------------------------------
// public API
// ---------------------------------------------------------------------------

/// The rebuilt table, plus what could not be evaluated.
#[derive(Debug, Clone, Default)]
pub struct EntityModels {
    /// `minecraft:<model>#<layer>` -> baked part tree, in `createRoots` put order.
    pub layers: Vec<(String, Value)>,
    pub gaps: Vec<String>,
    pub notes: Vec<String>,
}

impl EntityModels {
    pub fn to_value(&self) -> Value {
        let mut object = Map::new();
        for (key, value) in &self.layers {
            object.insert(key.clone(), value.clone());
        }
        Value::Object(object)
    }

    pub fn keys(&self) -> Vec<&str> {
        self.layers.iter().map(|(key, _)| key.as_str()).collect()
    }
}

/// Evaluate `LayerDefinitions.createRoots()` and bake every layer.
pub fn entity_models(jar_path: &Path) -> Result<EntityModels, String> {
    let jar = JarFiles::open(jar_path)?;
    let mut vm = Vm::new(jar);
    let roots = vm.create_roots()?;
    let mut result = EntityModels {
        notes: std::mem::take(&mut vm.notes),
        ..EntityModels::default()
    };
    for (key, layer) in roots {
        let Val::Layer(layer) = layer else {
            result
                .gaps
                .push(format!("{key}: value is not a LayerDefinition"));
            continue;
        };
        let key = key;
        let _ = &key;
        result
            .layers
            .push((key, part_json(&layer.mesh.borrow().root.clone(), layer.tex)));
    }
    Ok(result)
}

impl Vm {
    fn create_roots(&mut self) -> Result<Vec<(String, Val)>, String> {
        let class = self.class("net/minecraft/client/model/geom/LayerDefinitions")?;
        self.init(&class.name)?;
        let member = Self::member(&class, "createRoots", "")
            .ok_or_else(|| "LayerDefinitions.createRoots is missing".to_string())?;
        let value = self.call(class, &member, Vec::new(), None)?;
        let Val::Map(entries) = value else {
            return Err(format!(
                "createRoots returned {} instead of a map",
                val_name(&value)
            ));
        };
        let entries = entries.borrow().clone();
        let mut out = Vec::with_capacity(entries.len());
        for (key, value) in entries {
            let Val::ModelLayer(layer) = key else {
                return Err("createRoots key is not a ModelLayerLocation".to_string());
            };
            out.push((layer.key(), value));
        }
        Ok(out)
    }
}

/// Write `entity-models.json` (compact, in `createRoots` order).
pub fn write_entity_models(jar_path: &Path, out: impl AsRef<Path>) -> Result<EntityModels, String> {
    let table = entity_models(jar_path)?;
    let mut text = String::from("{");
    for (slot, (key, value)) in table.layers.iter().enumerate() {
        if slot > 0 {
            text.push(',');
        }
        let key = serde_json::to_string(key).map_err(|error| error.to_string())?;
        text.push_str(&key);
        text.push(':');
        text.push_str(&serde_json::to_string(value).map_err(|error| error.to_string())?);
    }
    text.push('}');
    let out = out.as_ref();
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("{}: {error}", parent.display()))?;
    }
    std::fs::write(out, text).map_err(|error| format!("{}: {error}", out.display()))?;
    Ok(table)
}
