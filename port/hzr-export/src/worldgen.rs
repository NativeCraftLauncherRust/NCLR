//! Horizon's `biome-parameters` tables, rebuilt from the player's own `client.jar`.
//!
//! `ship.hzr3` ships four files with two tables in them:
//!
//! ```text
//! biome-parameters/overworld.json
//! biome-parameters/nether.json
//! worldgen/biome_parameters/overworld.json
//! worldgen/biome_parameters/nether.json
//! ```
//!
//! They are vanilla's data-generator report `reports/biome_parameters/minecraft/*.json`
//! (`net.minecraft.data.info.BiomeParametersDumpReport`), which `horizon-prepare`
//! copies out of the data generator's output. This module rebuilds them from the
//! player's own jar, so the launcher can stop shipping the bundle.
//!
//! # Where the values live
//!
//! The report is *not* a data file. `data/minecraft/worldgen/multi_noise_biome_source_parameter_list/`
//! holds two one-line documents that name a built-in preset:
//!
//! ```json
//! { "preset": "minecraft:overworld" }
//! ```
//!
//! and `MultiNoiseBiomeSourceParameterList.Preset` builds the parameter lists in code:
//! `Preset.OVERWORLD` runs `OverworldBiomeBuilder.addBiomes`, `Preset.NETHER` is an
//! anonymous source provider (`MultiNoiseBiomeSourceParameterList$Preset$1`). Evidence:
//!
//! * `net/minecraft/world/level/biome/OverworldBiomeBuilder.class` — 7594 points built by
//!   `addBiomes`/`addOffCoastBiomes`/`addInlandBiomes`/`addPeaks`/`addHighSlice`/
//!   `addMidSlice`/`addLowSlice`/`addValleys`/`addUndergroundBiomes`.
//! * `net/minecraft/world/level/biome/MultiNoiseBiomeSourceParameterList$Preset$1.class` —
//!   five `Climate.parameters` calls (the whole nether table).
//!
//! So the coordinates are Java code constants, not pack data. Rather than copy those
//! constants into Rust (which would be transcribing decompiled source and would rot on
//! the next version), this module reads the two classes out of the jar and *runs* the
//! vanilla methods in a small interpreter for the bytecode subset they use. Every number
//! and every biome id therefore comes from the player's own jar. The only things stated
//! here are the parts of vanilla's own contract that the report format needs:
//!
//! * `Climate.quantizeCoord(f) = (long) (f * 10000.0f)`, `Climate.unquantizeCoord(j) = (float) j / 10000.0f`.
//! * `Climate.Parameter` is `(long min, long max)`; `Parameter.span(FF)`, `point(F)`,
//!   `span(Parameter, Parameter)` and `max()` behave as their bytecode says.
//! * `Climate.parameters(6 x Parameter, float offset)` builds a `ParameterPoint` whose
//!   offset is `quantizeCoord(offset)`.
//! * `Biomes.<FIELD>` is `minecraft:<lowercased field name>`; the id is checked against
//!   `data/minecraft/worldgen/biome/<id>.json` before it is written.
//! * The report writes a `Parameter` as one number when `min == max` and as
//!   `[min, max]` otherwise, both `unquantizeCoord`-ed.
//!
//! Nothing here reads `ship.hzr3`.
//!
//! # Why an interpreter
//!
//! The overworld builder is control flow, not data: 7594 points from nested loops over
//! `temperatures[5]`, `humidities[5]`, `erosions[7]` and the `ResourceKey[][]` tables its
//! constructor builds. The executed method set uses no arithmetic opcodes at all — only
//! loads/stores, field and array access, branches, comparisons and calls — so the subset
//! the VM below implements is small and closed. Unsupported opcodes and calls are hard
//! errors rather than silent guesses, so a jar that changes shape fails loudly.
//!
//! # Wiring
//!
//! `src/main.rs` is out of scope for this change, so nothing calls in here yet; a
//! `biome-parameters` command would be
//!
//! ```text
//! hzr-export biome-parameters <client.jar> <resources-dir>
//! ```
//!
//! over [`export`], which writes all four paths below `resources-dir`.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::Path;

/// The presets vanilla's report writes, in the order the four files are listed.
pub const PRESETS: [&str; 2] = ["overworld", "nether"];

const OVERWORLD_BUILDER: &str = "net/minecraft/world/level/biome/OverworldBiomeBuilder.class";
const NETHER_PROVIDER: &str =
    "net/minecraft/world/level/biome/MultiNoiseBiomeSourceParameterList$Preset$1.class";
const BIOME_DIR: &str = "data/minecraft/worldgen/biome/";

/// One `Climate.Parameter`: the quantized `[min, max]` interval vanilla stores.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Parameter {
    pub min: i64,
    pub max: i64,
}

impl Parameter {
    fn point(value: f32) -> Parameter {
        let quantized = quantize(value);
        Parameter {
            min: quantized,
            max: quantized,
        }
    }

    fn span(min: f32, max: f32) -> Parameter {
        Parameter {
            min: quantize(min),
            max: quantize(max),
        }
    }

    /// `Parameter.span(Parameter, Parameter)`: from the first's start to the second's end.
    fn covering(start: Parameter, end: Parameter) -> Parameter {
        Parameter {
            min: start.min,
            max: end.max,
        }
    }
}

/// One `Climate.ParameterPoint`, in vanilla's field order, plus its offset.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParameterPoint {
    pub temperature: Parameter,
    pub humidity: Parameter,
    pub continentalness: Parameter,
    pub erosion: Parameter,
    pub depth: Parameter,
    pub weirdness: Parameter,
    /// `ParameterPoint.offset` is a quantized `long` in vanilla.
    pub offset: i64,
}

/// One entry of the report's `biomes` array.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BiomeRow {
    /// A full id, `minecraft:` included.
    pub biome: String,
    pub point: ParameterPoint,
}

/// `Climate.quantizeCoord`: truncating conversion of the 1/10000 grid coordinate.
fn quantize(value: f32) -> i64 {
    // `f * 10000.0f` then Java's `f2l` (truncate toward zero) — Rust `as` does the same.
    (value * 10_000.0f32) as i64
}

/// `Climate.unquantizeCoord`.
fn unquantize(quantized: i64) -> f32 {
    // Java `l2f` then `fdiv`; both are IEEE round-to-nearest.
    quantized as f32 / 10_000.0f32
}

// ---------------------------------------------------------------------------
// The jar
// ---------------------------------------------------------------------------

/// The handful of `client.jar` entries this table needs, plus the biome ids it ships.
pub struct Jar {
    entries: BTreeMap<String, Vec<u8>>,
    biomes: BTreeSet<String>,
}

impl Jar {
    /// Opens `client.jar` and reads the classes the parameter tables live in.
    ///
    /// The jar is never read whole: only the classes below and the names under
    /// `data/minecraft/worldgen/biome/` (used to check the biome ids the report names).
    pub fn open(path: &str) -> Result<Jar, String> {
        let file = fs::File::open(path).map_err(|e| format!("{path}: {e}"))?;
        let mut zip = zip::ZipArchive::new(file).map_err(|e| format!("{path}: {e}"))?;
        let mut entries = BTreeMap::new();
        let mut biomes = BTreeSet::new();
        for index in 0..zip.len() {
            let mut entry = zip.by_index(index).map_err(|e| format!("{path}: {e}"))?;
            let name = entry
                .name()
                .map_err(|e| format!("{path}: entry {index}: {e}"))?
                .to_string();
            if name == OVERWORLD_BUILDER || name == NETHER_PROVIDER {
                let mut data = Vec::new();
                entry
                    .read_to_end(&mut data)
                    .map_err(|e| format!("{path}: {name}: {e}"))?;
                entries.insert(name, data);
            } else if let Some(rest) = name.strip_prefix(BIOME_DIR) {
                if let Some(id) = rest.strip_suffix(".json") {
                    if !id.contains('/') {
                        biomes.insert(id.to_string());
                    }
                }
            }
        }
        if !entries.contains_key(OVERWORLD_BUILDER) || !entries.contains_key(NETHER_PROVIDER) {
            return Err(format!(
                "{path}: no {OVERWORLD_BUILDER} or no {NETHER_PROVIDER}; not a Minecraft jar"
            ));
        }
        Ok(Jar { entries, biomes })
    }

    fn class(&self, name: &str) -> Result<ClassFile, String> {
        let data = self
            .entries
            .get(name)
            .ok_or_else(|| format!("{name} is not in the jar"))?;
        ClassFile::parse(data).map_err(|e| format!("{name}: {e}"))
    }

    /// Every biome id under `data/minecraft/worldgen/biome/`, without the namespace.
    pub fn biome_ids(&self) -> &BTreeSet<String> {
        &self.biomes
    }
}

// ---------------------------------------------------------------------------
// Public entry points
// ---------------------------------------------------------------------------

/// Builds one preset's table from an open jar.
///
/// `preset` is `"overworld"` or `"nether"`, vanilla's `MultiNoiseBiomeSourceParameterList.Preset`
/// names.
pub fn table(jar: &Jar, preset: &str) -> Result<Vec<BiomeRow>, String> {
    let rows = match preset {
        "overworld" => overworld_rows(jar)?,
        "nether" => nether_rows(jar)?,
        other => {
            return Err(format!(
                "unknown preset {other:?}; known presets are {}",
                PRESETS.join(", ")
            ))
        }
    };
    if rows.is_empty() {
        return Err(format!("{preset}: vanilla produced no parameter points"));
    }
    Ok(rows)
}

/// Builds and renders one preset's report, the exact shape the data generator writes.
pub fn render_table(jar: &Jar, preset: &str) -> Result<String, String> {
    Ok(render(&table(jar, preset)?))
}

/// Writes all four files `ship.hzr3` carries, below `root`:
///
/// ```text
/// biome-parameters/<preset>.json
/// worldgen/biome_parameters/<preset>.json
/// ```
pub fn export(jar_path: &str, root: &Path) -> Result<(), String> {
    let jar = Jar::open(jar_path)?;
    for preset in PRESETS {
        let text = render_table(&jar, preset)?;
        for relative in [
            format!("biome-parameters/{preset}.json"),
            format!("worldgen/biome_parameters/{preset}.json"),
        ] {
            let path = root.join(&relative);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
            }
            fs::write(&path, &text).map_err(|e| format!("{}: {e}", path.display()))?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// The two presets
// ---------------------------------------------------------------------------

/// `MultiNoiseBiomeSourceParameterList.Preset.OVERWORLD`: `new OverworldBiomeBuilder().addBiomes`.
fn overworld_rows(jar: &Jar) -> Result<Vec<BiomeRow>, String> {
    let class = jar.class(OVERWORLD_BUILDER)?;
    let mut vm = Vm::new(&class, jar.biome_ids());
    let this = vm.new_instance(OVERWORLD_BUILDER);
    vm.call_named(Value::Ref(this), "<init>", Vec::new())?;
    let consumer = vm.new_instance("java/util/function/Consumer");
    vm.call_named(Value::Ref(this), "addBiomes", vec![Value::Ref(consumer)])?;
    Ok(vm.rows)
}

/// `MultiNoiseBiomeSourceParameterList.Preset.NETHER`: the anonymous provider's `apply`.
fn nether_rows(jar: &Jar) -> Result<Vec<BiomeRow>, String> {
    let class = jar.class(NETHER_PROVIDER)?;
    let mut vm = Vm::new(&class, jar.biome_ids());
    let this = vm.new_instance(
        "net/minecraft/world/level/biome/MultiNoiseBiomeSourceParameterList$Preset$1",
    );
    // `Function<ResourceKey<Biome>, T>`: the report asks for the keys themselves.
    let identity = vm.new_instance("java/util/function/Function");
    let list = vm.call_named(Value::Ref(this), "apply", vec![Value::Ref(identity)])?;
    // `apply` returns a `Climate.ParameterList`, whose `<init>` kept the `List.of(...)`.
    let values = vm.field(&list, "values")?;
    let elements = vm.elements_of(&values)?;
    let mut rows = Vec::with_capacity(elements.len());
    for element in elements {
        rows.push(vm.row_of(&element)?);
    }
    Ok(rows)
}

// ---------------------------------------------------------------------------
// The report writer
// ---------------------------------------------------------------------------

/// Renders a table as the data generator's report.
///
/// Formatting matches vanilla's (Gson pretty printing, two-space indent, keys in the
/// order the record declares them once sorted), so the output can also be compared byte
/// for byte; only the values matter to a reader.
pub fn render(rows: &[BiomeRow]) -> String {
    let mut out = String::with_capacity(rows.len() * 460 + 32);
    out.push_str("{\n  \"biomes\": [\n");
    for (index, row) in rows.iter().enumerate() {
        if index > 0 {
            out.push_str(",\n");
        }
        out.push_str("    {\n");
        out.push_str("      \"biome\": ");
        write_string(&mut out, &row.biome);
        out.push_str(",\n");
        out.push_str("      \"parameters\": {\n");
        let point = &row.point;
        // The record's seven components, alphabetized the way the report writes them:
        // `offset` sorts between `humidity` and `temperature`.
        for (name, parameter) in [
            ("continentalness", point.continentalness),
            ("depth", point.depth),
            ("erosion", point.erosion),
            ("humidity", point.humidity),
        ] {
            write_parameter(&mut out, name, parameter, false);
        }
        write_offset(&mut out, point.offset);
        for (name, parameter) in [
            ("temperature", point.temperature),
            ("weirdness", point.weirdness),
        ] {
            write_parameter(&mut out, name, parameter, name == "weirdness");
        }
        out.push_str("      }\n    }");
    }
    out.push_str("\n  ]\n}");
    out
}

fn write_parameter(out: &mut String, name: &str, parameter: Parameter, last: bool) {
    let comma = if last { "" } else { "," };
    if parameter.min == parameter.max {
        out.push_str("        ");
        write_string(out, name);
        out.push_str(": ");
        write_number(out, unquantize(parameter.min));
        out.push_str(comma);
        out.push('\n');
    } else {
        out.push_str("        ");
        write_string(out, name);
        out.push_str(": [\n          ");
        write_number(out, unquantize(parameter.min));
        out.push_str(",\n          ");
        write_number(out, unquantize(parameter.max));
        out.push_str("\n        ]");
        out.push_str(comma);
        out.push('\n');
    }
}

/// `ParameterPoint.offset` is a quantized `long`, written as `unquantizeCoord` of it.
fn write_offset(out: &mut String, offset: i64) {
    out.push_str("        \"offset\": ");
    write_number(out, unquantize(offset));
    out.push_str(",\n");
}

fn write_string(out: &mut String, value: &str) {
    out.push('"');
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// A quantized coordinate as the report writes it: `Float.toString(unquantizeCoord(x))`.
fn write_number(out: &mut String, value: f32) {
    if !value.is_finite() {
        out.push_str("null");
        return;
    }
    let text = format!("{value}");
    out.push_str(&text);
    if !text.contains('.') && !text.contains('e') && !text.contains('E') {
        out.push_str(".0");
    }
}

// ---------------------------------------------------------------------------
// Class files
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
enum Constant {
    Unusable,
    Utf8(String),
    Int(i32),
    Float(f32),
    Long(i64),
    /// Only ever rejected, so the value is not kept.
    Double,
    Class(u16),
    /// Only ever rejected, so the payload (another constant's index) is not kept.
    String,
    Field {
        class: u16,
        name_and_type: u16,
    },
    Method {
        class: u16,
        name_and_type: u16,
    },
    InterfaceMethod {
        class: u16,
        name_and_type: u16,
    },
    NameAndType {
        name: u16,
        descriptor: u16,
    },
    Other,
}

struct MethodCode {
    name: String,
    descriptor: String,
    max_locals: u16,
    code: Vec<u8>,
}

struct ClassFile {
    pool: Vec<Constant>,
    methods: Vec<MethodCode>,
}

impl ClassFile {
    fn parse(data: &[u8]) -> Result<ClassFile, String> {
        let mut at = Cursor::new(data);
        if at.u4()? != 0xCAFE_BABE {
            return Err("not a class file".into());
        }
        at.u2()?; // minor
        at.u2()?; // major
        let count = at.u2()? as usize;
        let mut pool = vec![Constant::Unusable; count];
        let mut index = 1;
        while index < count {
            let tag = at.u1()?;
            let constant = match tag {
                1 => {
                    let length = at.u2()? as usize;
                    let bytes = at.take(length)?;
                    Constant::Utf8(String::from_utf8_lossy(bytes).into_owned())
                }
                3 => Constant::Int(at.u4()? as i32),
                4 => Constant::Float(f32::from_bits(at.u4()?)),
                5 => Constant::Long(at.u8()? as i64),
                6 => {
                    at.u8()?;
                    Constant::Double
                }
                7 => Constant::Class(at.u2()?),
                8 => {
                    at.u2()?;
                    Constant::String
                }
                9 => Constant::Field {
                    class: at.u2()?,
                    name_and_type: at.u2()?,
                },
                10 => Constant::Method {
                    class: at.u2()?,
                    name_and_type: at.u2()?,
                },
                11 => Constant::InterfaceMethod {
                    class: at.u2()?,
                    name_and_type: at.u2()?,
                },
                12 => Constant::NameAndType {
                    name: at.u2()?,
                    descriptor: at.u2()?,
                },
                15 => {
                    at.u1()?;
                    at.u2()?;
                    Constant::Other
                }
                16 => {
                    at.u2()?;
                    Constant::Other
                }
                17 | 18 => {
                    at.u2()?;
                    at.u2()?;
                    Constant::Other
                }
                19 | 20 => {
                    at.u2()?;
                    Constant::Other
                }
                other => return Err(format!("constant pool tag {other}")),
            };
            let takes_two = tag == 5 || tag == 6;
            pool[index] = constant;
            index += if takes_two { 2 } else { 1 };
        }
        at.u2()?; // access flags
        at.u2()?; // this class
        at.u2()?; // super class
        let interfaces = at.u2()?;
        for _ in 0..interfaces {
            at.u2()?;
        }
        let fields = at.u2()?;
        for _ in 0..fields {
            at.u2()?;
            at.u2()?;
            at.u2()?;
            let attributes = at.u2()?;
            for _ in 0..attributes {
                at.u2()?;
                let length = at.u4()? as usize;
                at.take(length)?;
            }
        }
        let class = ClassFile {
            pool,
            methods: Vec::new(),
        };
        let mut methods = Vec::new();
        let method_count = at.u2()?;
        for _ in 0..method_count {
            at.u2()?; // access flags
            let name = class.utf(at.u2()?)?.to_string();
            let descriptor = class.utf(at.u2()?)?.to_string();
            let attributes = at.u2()?;
            let mut max_locals = 0;
            let mut code = None;
            for _ in 0..attributes {
                let attribute = class.utf(at.u2()?)?;
                let length = at.u4()? as usize;
                if attribute == "Code" {
                    let body = at.take(length)?;
                    let mut body_at = Cursor::new(body);
                    body_at.u2()?; // max stack
                    max_locals = body_at.u2()?;
                    let code_length = body_at.u4()? as usize;
                    code = Some(body_at.take(code_length)?.to_vec());
                } else {
                    at.take(length)?;
                }
            }
            if let Some(code) = code {
                methods.push(MethodCode {
                    name,
                    descriptor,
                    max_locals,
                    code,
                });
            }
        }
        Ok(ClassFile {
            pool: class.pool,
            methods,
        })
    }

    fn utf(&self, index: u16) -> Result<&str, String> {
        match self.pool.get(index as usize) {
            Some(Constant::Utf8(value)) => Ok(value),
            _ => Err(format!("constant {index} is not a string")),
        }
    }

    fn class_name(&self, index: u16) -> Result<&str, String> {
        match self.pool.get(index as usize) {
            Some(Constant::Class(name)) => self.utf(*name),
            _ => Err(format!("constant {index} is not a class")),
        }
    }

    /// `(owner, name, descriptor)` for a field or method reference.
    fn member(&self, index: u16) -> Result<(&str, &str, &str), String> {
        let (class, name_and_type) = match self.pool.get(index as usize) {
            Some(Constant::Field {
                class,
                name_and_type,
            })
            | Some(Constant::Method {
                class,
                name_and_type,
            })
            | Some(Constant::InterfaceMethod {
                class,
                name_and_type,
            }) => (*class, *name_and_type),
            _ => return Err(format!("constant {index} is not a member reference")),
        };
        let (name, descriptor) = match self.pool.get(name_and_type as usize) {
            Some(Constant::NameAndType { name, descriptor }) => (*name, *descriptor),
            _ => return Err(format!("constant {name_and_type} is not a name and type")),
        };
        Ok((
            self.class_name(class)?,
            self.utf(name)?,
            self.utf(descriptor)?,
        ))
    }

    fn method(&self, name: &str) -> Result<&MethodCode, String> {
        self.methods
            .iter()
            .find(|method| method.name == name)
            .ok_or_else(|| format!("no method {name} in the class"))
    }
}

struct Cursor<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    fn new(data: &'a [u8]) -> Cursor<'a> {
        Cursor { data, at: 0 }
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8], String> {
        let end = self.at.checked_add(length).ok_or("class file is cut")?;
        let slice = self.data.get(self.at..end).ok_or("class file is cut")?;
        self.at = end;
        Ok(slice)
    }

    fn u1(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }

    fn u2(&mut self) -> Result<u16, String> {
        let bytes = self.take(2)?;
        Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
    }

    fn u4(&mut self) -> Result<u32, String> {
        let bytes = self.take(4)?;
        Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn u8(&mut self) -> Result<u64, String> {
        let high = self.u4()? as u64;
        let low = self.u4()? as u64;
        Ok((high << 32) | low)
    }
}

// ---------------------------------------------------------------------------
// The interpreter
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
enum Value {
    Int(i32),
    Long(i64),
    Float(f32),
    Ref(usize),
    Null,
}

#[derive(Clone, Debug)]
enum Object {
    Parameter(Parameter),
    Point(ParameterPoint),
    Biome(String),
    Pair(Value, Value),
    Array(Vec<Value>),
    Instance {
        class: String,
        fields: BTreeMap<String, Value>,
    },
}

struct Vm<'a> {
    class: &'a ClassFile,
    biome_ids: &'a BTreeSet<String>,
    heap: Vec<Object>,
    rows: Vec<BiomeRow>,
}

impl<'a> Vm<'a> {
    fn new(class: &'a ClassFile, biome_ids: &'a BTreeSet<String>) -> Vm<'a> {
        Vm {
            class,
            biome_ids,
            heap: Vec::new(),
            rows: Vec::new(),
        }
    }

    fn allocate(&mut self, object: Object) -> Value {
        self.heap.push(object);
        Value::Ref(self.heap.len() - 1)
    }

    fn new_instance(&mut self, class: &str) -> usize {
        let value = self.allocate(Object::Instance {
            class: class.to_string(),
            fields: BTreeMap::new(),
        });
        match value {
            Value::Ref(index) => index,
            _ => unreachable!(),
        }
    }

    fn method(&self, name: &str) -> Result<&'a MethodCode, String> {
        let class: &'a ClassFile = self.class;
        class.method(name)
    }

    /// Calls a method of the loaded class, with `receiver` as local 0 if given.
    fn call_named(
        &mut self,
        receiver: Value,
        name: &str,
        args: Vec<Value>,
    ) -> Result<Value, String> {
        let method = self.method(name)?;
        let mut locals = Vec::with_capacity(args.len() + 1);
        if !matches!(receiver, Value::Null) {
            locals.push(receiver);
        }
        locals.extend(args);
        self.run(method, locals)
    }

    fn run(&mut self, method: &MethodCode, locals: Vec<Value>) -> Result<Value, String> {
        let context = format!("{}{}", method.name, method.descriptor);
        let code = &method.code[..];
        let mut locals = locals;
        locals.resize((method.max_locals as usize).max(locals.len()), Value::Null);
        let mut stack: Vec<Value> = Vec::with_capacity(method.max_locals as usize + 8);
        let mut at = 0usize;
        loop {
            let opcode = *code
                .get(at)
                .ok_or_else(|| format!("{}: ran off the end", context))?;
            // JVM branch offsets are relative to the branch opcode's own address.
            let opcode_at = at;
            at += 1;
            match opcode {
                0x00 => {}
                0x01 => stack.push(Value::Null),
                0x02 => stack.push(Value::Int(-1)),
                0x03..=0x08 => stack.push(Value::Int((opcode - 0x03) as i32)),
                0x09 => stack.push(Value::Long(0)),
                0x0a => stack.push(Value::Long(1)),
                0x0b => stack.push(Value::Float(0.0)),
                0x0c => stack.push(Value::Float(1.0)),
                0x0d => stack.push(Value::Float(2.0)),
                0x10 => {
                    let value = *code.get(at).ok_or_else(|| format!("{}: bipush", context))?;
                    at += 1;
                    stack.push(Value::Int(value as i8 as i32));
                }
                0x11 => {
                    let value = read_i16(code, &mut at, &context)?;
                    stack.push(Value::Int(value as i32));
                }
                0x12 => {
                    let index = *code.get(at).ok_or_else(|| format!("{}: ldc", context))?;
                    at += 1;
                    let value = self.constant(index as u16, &context)?;
                    stack.push(value);
                }
                0x13 => {
                    let index = read_u16(code, &mut at, &context)?;
                    let value = self.constant(index, &context)?;
                    stack.push(value);
                }
                0x14 => {
                    let index = read_u16(code, &mut at, &context)?;
                    let value = match self.class.pool.get(index as usize) {
                        Some(Constant::Long(value)) => Value::Long(*value),
                        Some(Constant::Double) => {
                            return Err(format!("{}: double constant", context))
                        }
                        _ => return Err(format!("{}: bad ldc2_w {index}", context)),
                    };
                    stack.push(value);
                }
                0x15 => {
                    let index = self.local_index(code, &mut at, &context)?;
                    stack.push(local(&locals, index, &context)?);
                }
                0x17 => {
                    let index = self.local_index(code, &mut at, &context)?;
                    stack.push(local(&locals, index, &context)?);
                }
                0x19 => {
                    let index = self.local_index(code, &mut at, &context)?;
                    stack.push(local(&locals, index, &context)?);
                }
                0x1a..=0x1d => stack.push(local(&locals, (opcode - 0x1a) as usize, &context)?),
                0x22..=0x25 => stack.push(local(&locals, (opcode - 0x22) as usize, &context)?),
                0x2a..=0x2d => stack.push(local(&locals, (opcode - 0x2a) as usize, &context)?),
                0x32 => {
                    let index = pop_int(&mut stack, &context)?;
                    let array = pop(&mut stack, &context)?;
                    stack.push(self.element(&array, index, &context)?);
                }
                0x36 | 0x3a => {
                    let index = self.local_index(code, &mut at, &context)?;
                    let value = pop(&mut stack, &context)?;
                    store_local(&mut locals, index, value, &context)?;
                }
                0x3b..=0x3e => {
                    let index = (opcode - 0x3b) as usize;
                    let value = pop(&mut stack, &context)?;
                    store_local(&mut locals, index, value, &context)?;
                }
                0x4b..=0x4e => {
                    let index = (opcode - 0x4b) as usize;
                    let value = pop(&mut stack, &context)?;
                    store_local(&mut locals, index, value, &context)?;
                }
                0x53 => {
                    let value = pop(&mut stack, &context)?;
                    let index = pop_int(&mut stack, &context)?;
                    let array = pop(&mut stack, &context)?;
                    self.store_element(&array, index, value, &context)?;
                }
                0x59 => {
                    let value = stack
                        .last()
                        .cloned()
                        .ok_or_else(|| format!("{}: dup on nothing", context))?;
                    stack.push(value);
                }
                0x84 => {
                    let index = *code.get(at).ok_or_else(|| format!("{}: iinc", context))? as usize;
                    let delta = *code
                        .get(at + 1)
                        .ok_or_else(|| format!("{}: iinc", context))?
                        as i8 as i32;
                    at += 2;
                    let value = local(&locals, index, &context)?;
                    store_local(
                        &mut locals,
                        index,
                        Value::Int(int_of(&value, &context)? + delta),
                        &context,
                    )?;
                }
                0x94 => {
                    let right = pop_long(&mut stack, &context)?;
                    let left = pop_long(&mut stack, &context)?;
                    stack.push(Value::Int(match left.cmp(&right) {
                        std::cmp::Ordering::Less => -1,
                        std::cmp::Ordering::Equal => 0,
                        std::cmp::Ordering::Greater => 1,
                    }));
                }
                0x99..=0x9e => {
                    let offset = read_i16(code, &mut at, &context)?;
                    let value = pop_int(&mut stack, &context)?;
                    let taken = match opcode {
                        0x99 => value == 0,
                        0x9a => value != 0,
                        0x9b => value < 0,
                        0x9c => value >= 0,
                        0x9d => value > 0,
                        _ => value <= 0,
                    };
                    if taken {
                        branch(&mut at, opcode_at, offset, &context)?;
                    }
                }
                0x9f..=0xa4 => {
                    let offset = read_i16(code, &mut at, &context)?;
                    let right = pop_int(&mut stack, &context)?;
                    let left = pop_int(&mut stack, &context)?;
                    let taken = match opcode {
                        0x9f => left == right,
                        0xa0 => left != right,
                        0xa1 => left < right,
                        0xa2 => left >= right,
                        0xa3 => left > right,
                        _ => left <= right,
                    };
                    if taken {
                        branch(&mut at, opcode_at, offset, &context)?;
                    }
                }
                0xa5 | 0xa6 => {
                    let offset = read_i16(code, &mut at, &context)?;
                    let right = pop(&mut stack, &context)?;
                    let left = pop(&mut stack, &context)?;
                    let same = same_reference(&left, &right);
                    let taken = if opcode == 0xa5 { same } else { !same };
                    if taken {
                        branch(&mut at, opcode_at, offset, &context)?;
                    }
                }
                0xa7 => {
                    let offset = read_i16(code, &mut at, &context)?;
                    branch(&mut at, opcode_at, offset, &context)?;
                }
                0xb0 => return pop(&mut stack, &context),
                0xb1 => return Ok(Value::Null),
                0xb2 => {
                    let index = read_u16(code, &mut at, &context)?;
                    let value = self.static_field(index, &context)?;
                    stack.push(value);
                }
                0xb3 => {
                    let _index = read_u16(code, &mut at, &context)?;
                    let _value = pop(&mut stack, &context)?;
                }
                0xb4 => {
                    let index = read_u16(code, &mut at, &context)?;
                    let (_, name, _) = self.member_owned(index, &context)?;
                    let receiver = pop(&mut stack, &context)?;
                    let value = self.field(&receiver, &name)?;
                    stack.push(value);
                }
                0xb5 => {
                    let index = read_u16(code, &mut at, &context)?;
                    let (_, name, _) = self.member_owned(index, &context)?;
                    let value = pop(&mut stack, &context)?;
                    let receiver = pop(&mut stack, &context)?;
                    self.set_field(&receiver, &name, value, &context)?;
                }
                0xb6..=0xb9 => {
                    let index = read_u16(code, &mut at, &context)?;
                    if opcode == 0xb9 {
                        // invokeinterface: a count and a reserved zero follow the index.
                        at += 2;
                    }
                    let (owner, name, descriptor) = self.member_owned(index, &context)?;
                    let mut arguments = Vec::new();
                    for _ in 0..argument_count(&descriptor)? {
                        arguments.push(pop(&mut stack, &context)?);
                    }
                    arguments.reverse();
                    let receiver = if opcode == 0xb8 {
                        Value::Null
                    } else {
                        pop(&mut stack, &context)?
                    };
                    let value = self.invoke(&owner, &name, &descriptor, receiver, arguments)?;
                    if !matches!(descriptor.chars().last(), Some('V')) {
                        stack.push(value);
                    }
                }
                0xbb => {
                    let index = read_u16(code, &mut at, &context)?;
                    let class = self.class.class_name(index)?.to_string();
                    let value = self.allocate(Object::Instance {
                        class,
                        fields: BTreeMap::new(),
                    });
                    stack.push(value);
                }
                0xbd => {
                    let _index = read_u16(code, &mut at, &context)?;
                    let count = pop_int(&mut stack, &context)?;
                    if count < 0 {
                        return Err(format!("{}: negative array size", context));
                    }
                    let value = self.allocate(Object::Array(vec![Value::Null; count as usize]));
                    stack.push(value);
                }
                0xbe => {
                    let array = pop(&mut stack, &context)?;
                    let length = match self.object(&array, &context)? {
                        Object::Array(elements) => elements.len(),
                        _ => return Err(format!("{}: arraylength of a non-array", context)),
                    };
                    stack.push(Value::Int(length as i32));
                }
                0xc6 | 0xc7 => {
                    let offset = read_i16(code, &mut at, &context)?;
                    let value = pop(&mut stack, &context)?;
                    let is_null = matches!(value, Value::Null);
                    let taken = if opcode == 0xc6 { is_null } else { !is_null };
                    if taken {
                        branch(&mut at, opcode_at, offset, &context)?;
                    }
                }
                other => {
                    return Err(format!(
                        "{}: unsupported opcode 0x{other:02x} at {at}",
                        context
                    ))
                }
            }
        }
    }

    /// Resolves one `invoke*` instruction.
    fn invoke(
        &mut self,
        owner: &str,
        name: &str,
        descriptor: &str,
        receiver: Value,
        arguments: Vec<Value>,
    ) -> Result<Value, String> {
        // Vanilla's own methods, run from the jar.
        if owner == "net/minecraft/world/level/biome/OverworldBiomeBuilder"
            || owner
                == "net/minecraft/world/level/biome/MultiNoiseBiomeSourceParameterList$Preset$1"
        {
            let method = self
                .class
                .methods
                .iter()
                .find(|method| {
                    method.name == name && (method.descriptor == descriptor || name == "<init>")
                })
                .ok_or_else(|| format!("{owner}.{name}{descriptor} is not in the class"))?;
            let mut locals = Vec::with_capacity(arguments.len() + 1);
            locals.push(receiver);
            locals.extend(arguments);
            return self.run(method, locals);
        }

        // The small slice of the JDK and Mojang libraries the two classes call.
        match (owner, name) {
            ("java/lang/Object", "<init>") => Ok(Value::Null),
            ("net/minecraft/world/level/biome/Climate$Parameter", "span") => {
                if descriptor == "(FF)Lnet/minecraft/world/level/biome/Climate$Parameter;" {
                    let end = float_of(&arguments[1])?;
                    let start = float_of(&arguments[0])?;
                    Ok(self.allocate(Object::Parameter(Parameter::span(start, end))))
                } else {
                    let end = self.parameter(&arguments[1])?;
                    let start = self.parameter(&arguments[0])?;
                    Ok(self.allocate(Object::Parameter(Parameter::covering(
                        start, end,
                    ))))
                }
            }
            ("net/minecraft/world/level/biome/Climate$Parameter", "point") => {
                let value = float_of(&arguments[0])?;
                Ok(self.allocate(Object::Parameter(Parameter::point(value))))
            }
            ("net/minecraft/world/level/biome/Climate$Parameter", "min") => {
                let parameter = self.parameter(&receiver)?;
                Ok(Value::Long(parameter.min))
            }
            ("net/minecraft/world/level/biome/Climate$Parameter", "max") => {
                let parameter = self.parameter(&receiver)?;
                Ok(Value::Long(parameter.max))
            }
            ("net/minecraft/world/level/biome/Climate", "parameters") => {
                let count = arguments.len();
                if count != 7 {
                    return Err(format!(
                        "{owner}.{name}{descriptor}: expected 7 arguments, saw {count}"
                    ));
                }
                let offset = float_of(&arguments[6])?;
                let offset = quantize(offset);
                // The seven-float overload is the one the nether preset uses; the
                // overworld's helpers pass six `Parameter`s and an offset.
                let point = if descriptor.starts_with("(FFFFFFF)") {
                    // `Climate.parameters(FFFFFFF)`.
                    let mut values = [0.0f32; 6];
                    for (slot, argument) in values.iter_mut().zip(&arguments[..6]) {
                        *slot = float_of(argument)?;
                    }
                    ParameterPoint {
                        temperature: Parameter::point(values[0]),
                        humidity: Parameter::point(values[1]),
                        continentalness: Parameter::point(values[2]),
                        erosion: Parameter::point(values[3]),
                        depth: Parameter::point(values[4]),
                        weirdness: Parameter::point(values[5]),
                        offset,
                    }
                } else {
                    // `Climate.parameters(6 x Parameter, float)`.
                    ParameterPoint {
                        temperature: self.parameter(&arguments[0])?,
                        humidity: self.parameter(&arguments[1])?,
                        continentalness: self.parameter(&arguments[2])?,
                        erosion: self.parameter(&arguments[3])?,
                        depth: self.parameter(&arguments[4])?,
                        weirdness: self.parameter(&arguments[5])?,
                        offset,
                    }
                };
                Ok(self.allocate(Object::Point(point)))
            }
            ("com/mojang/datafixers/util/Pair", "of") => {
                Ok(self.allocate(Object::Pair(arguments[0].clone(), arguments[1].clone())))
            }
            ("java/util/function/Consumer", "accept") => {
                self.collect(&arguments[0])?;
                Ok(Value::Null)
            }
            ("java/util/function/Function", "apply") => Ok(arguments[0].clone()),
            ("java/util/List", "of") => Ok(self.allocate(Object::Array(arguments))),
            ("net/minecraft/world/level/biome/Climate$ParameterList", "<init>") => {
                self.set_field(
                    &receiver,
                    "values",
                    arguments[0].clone(),
                    "Climate.ParameterList.<init>",
                )?;
                Ok(Value::Null)
            }
            _ => Err(format!(
                "unsupported call {owner}.{name}{descriptor}; this jar's code is not the shape this exporter knows"
            )),
        }
    }

    fn constant(&self, index: u16, context: &str) -> Result<Value, String> {
        match self.class.pool.get(index as usize) {
            Some(Constant::Int(value)) => Ok(Value::Int(*value)),
            Some(Constant::Float(value)) => Ok(Value::Float(*value)),
            Some(Constant::Long(value)) => Ok(Value::Long(*value)),
            Some(Constant::String) => Err(format!("{context}: string constant")),
            _ => Err(format!("{context}: unsupported constant {index}")),
        }
    }

    /// `getstatic`: the biome constants and the one debug flag vanilla checks.
    fn static_field(&mut self, index: u16, context: &str) -> Result<Value, String> {
        let (owner, name, _) = self.member_owned(index, context)?;
        match owner.as_str() {
            "net/minecraft/world/level/biome/Biomes" => {
                Ok(self.allocate(Object::Biome(name.to_lowercase())))
            }
            "net/minecraft/SharedConstants" => {
                // `debugGenerateSquareTerrainWithoutNoise` is off in a normal run; the report
                // is the normal run's.
                Ok(Value::Int(0))
            }
            _ => Err(format!(
                "{context}: unsupported static field {owner}.{name}"
            )),
        }
    }

    fn member_owned(&self, index: u16, context: &str) -> Result<(String, String, String), String> {
        let (owner, name, descriptor) = self
            .class
            .member(index)
            .map_err(|error| format!("{context}: {error}"))?;
        Ok((owner.to_string(), name.to_string(), descriptor.to_string()))
    }

    fn local_index(&self, code: &[u8], at: &mut usize, context: &str) -> Result<usize, String> {
        let index = *code
            .get(*at)
            .ok_or_else(|| format!("{context}: local index"))?;
        *at += 1;
        Ok(index as usize)
    }

    fn object(&self, value: &Value, context: &str) -> Result<&Object, String> {
        let index = match value {
            Value::Ref(index) => *index,
            _ => return Err(format!("{context}: expected an object, saw {value:?}")),
        };
        self.heap
            .get(index)
            .ok_or_else(|| format!("{context}: object {index} is gone"))
    }

    fn parameter(&self, value: &Value) -> Result<Parameter, String> {
        match self.object(value, "parameter")? {
            Object::Parameter(parameter) => Ok(*parameter),
            other => Err(format!("expected a Parameter, saw {other:?}")),
        }
    }

    fn element(&self, array: &Value, index: i32, context: &str) -> Result<Value, String> {
        let elements = match self.object(array, context)? {
            Object::Array(elements) => elements,
            _ => return Err(format!("{context}: aaload of a non-array")),
        };
        elements
            .get(index as usize)
            .cloned()
            .ok_or_else(|| format!("{context}: array index {index} is out of bounds"))
    }

    fn store_element(
        &mut self,
        array: &Value,
        index: i32,
        value: Value,
        context: &str,
    ) -> Result<(), String> {
        let slot = index as usize;
        let array_index = match array {
            Value::Ref(index) => *index,
            _ => return Err(format!("{context}: aastore into a value")),
        };
        match self.heap.get_mut(array_index) {
            Some(Object::Array(elements)) => {
                let slot_ref = elements
                    .get_mut(slot)
                    .ok_or_else(|| format!("{context}: array index {slot} is out of bounds"))?;
                *slot_ref = value;
                Ok(())
            }
            _ => Err(format!("{context}: aastore into a non-array")),
        }
    }

    fn field(&self, value: &Value, name: &str) -> Result<Value, String> {
        match self.object(value, "getfield")? {
            Object::Instance { class, fields } => fields
                .get(name)
                .cloned()
                .ok_or_else(|| format!("getfield {name} on {class}: the field was never set")),
            other => Err(format!("getfield {name}: {other:?} has no fields")),
        }
    }

    fn set_field(
        &mut self,
        value: &Value,
        name: &str,
        field: Value,
        context: &str,
    ) -> Result<(), String> {
        let index = match value {
            Value::Ref(index) => *index,
            _ => return Err(format!("{context}: putfield on a value")),
        };
        match self.heap.get_mut(index) {
            Some(Object::Instance { fields, .. }) => {
                fields.insert(name.to_string(), field);
                Ok(())
            }
            other => Err(format!("{context}: putfield on {other:?}")),
        }
    }

    /// A `List` this table built: the `List.of(...)` array, or a `ParameterList` holding one.
    fn elements_of(&self, value: &Value) -> Result<Vec<Value>, String> {
        match self.object(value, "list")? {
            Object::Array(elements) => Ok(elements.clone()),
            other => Err(format!("expected a list, saw {other:?}")),
        }
    }

    fn collect(&mut self, value: &Value) -> Result<(), String> {
        let row = self.row_of(value)?;
        self.rows.push(row);
        Ok(())
    }

    fn row_of(&self, value: &Value) -> Result<BiomeRow, String> {
        let (point, biome) = match self.object(value, "row")? {
            Object::Pair(point, biome) => (point.clone(), biome.clone()),
            other => return Err(format!("expected a Pair, saw {other:?}")),
        };
        let point = match self.object(&point, "row")? {
            Object::Point(point) => point.clone(),
            other => return Err(format!("expected a ParameterPoint, saw {other:?}")),
        };
        let biome = match self.object(&biome, "row")? {
            Object::Biome(biome) => biome.clone(),
            other => return Err(format!("expected a biome key, saw {other:?}")),
        };
        if !self.biome_ids.contains(&biome) {
            return Err(format!("biome {biome:?} is not in {BIOME_DIR} in this jar"));
        }
        Ok(BiomeRow {
            biome: format!("minecraft:{biome}"),
            point,
        })
    }
}

fn pop(stack: &mut Vec<Value>, context: &str) -> Result<Value, String> {
    stack
        .pop()
        .ok_or_else(|| format!("{context}: stack underflow"))
}

fn pop_int(stack: &mut Vec<Value>, context: &str) -> Result<i32, String> {
    let value = pop(stack, context)?;
    int_of(&value, context)
}

fn pop_long(stack: &mut Vec<Value>, context: &str) -> Result<i64, String> {
    match pop(stack, context)? {
        Value::Long(value) => Ok(value),
        other => Err(format!("{context}: expected a long, saw {other:?}")),
    }
}

fn int_of(value: &Value, context: &str) -> Result<i32, String> {
    match value {
        Value::Int(value) => Ok(*value),
        other => Err(format!("{context}: expected an int, saw {other:?}")),
    }
}

fn float_of(value: &Value) -> Result<f32, String> {
    match value {
        Value::Float(value) => Ok(*value),
        other => Err(format!("expected a float, saw {other:?}")),
    }
}

fn local(locals: &[Value], index: usize, context: &str) -> Result<Value, String> {
    locals
        .get(index)
        .cloned()
        .ok_or_else(|| format!("{context}: local {index} is out of range"))
}

fn store_local(
    locals: &mut [Value],
    index: usize,
    value: Value,
    context: &str,
) -> Result<(), String> {
    match locals.get_mut(index) {
        Some(slot) => {
            *slot = value;
            Ok(())
        }
        None => Err(format!("{context}: local {index} is out of range")),
    }
}

fn same_reference(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Null, Value::Null) => true,
        (Value::Ref(left), Value::Ref(right)) => left == right,
        _ => false,
    }
}

fn read_u16(code: &[u8], at: &mut usize, context: &str) -> Result<u16, String> {
    let bytes = code
        .get(*at..*at + 2)
        .ok_or_else(|| format!("{context}: operand is cut"))?;
    *at += 2;
    Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
}

fn read_i16(code: &[u8], at: &mut usize, context: &str) -> Result<i16, String> {
    Ok(read_u16(code, at, context)? as i16)
}

fn branch(at: &mut usize, opcode_at: usize, offset: i16, context: &str) -> Result<(), String> {
    let target = opcode_at as isize + offset as isize;
    if target < 0 {
        return Err(format!("{context}: branch before the method"));
    }
    *at = target as usize;
    Ok(())
}

/// The slot count of a method descriptor's parameters, which is all the VM needs to pop
/// a call's operands.
fn argument_count(descriptor: &str) -> Result<usize, String> {
    let bytes = descriptor.as_bytes();
    if bytes.first() != Some(&b'(') {
        return Err(format!("{descriptor}: not a method descriptor"));
    }
    let mut at = 1;
    let mut count = 0;
    while at <= bytes.len() {
        match bytes.get(at) {
            None => return Err(format!("{descriptor}: the descriptor is cut")),
            Some(b')') => return Ok(count),
            Some(b'L') => {
                at += 1;
                while bytes.get(at) != Some(&b';') {
                    at += 1;
                    if at >= bytes.len() {
                        return Err(format!("{descriptor}: unterminated class name"));
                    }
                }
                at += 1;
                count += 1;
            }
            Some(b'[') => {
                while bytes.get(at) == Some(&b'[') {
                    at += 1;
                }
                match bytes.get(at) {
                    Some(b'L') => {
                        at += 1;
                        while bytes.get(at) != Some(&b';') {
                            at += 1;
                            if at >= bytes.len() {
                                return Err(format!("{descriptor}: unterminated class name"));
                            }
                        }
                        at += 1;
                    }
                    Some(_) => at += 1,
                    None => return Err(format!("{descriptor}: the descriptor is cut")),
                }
                count += 1;
            }
            Some(_) => {
                at += 1;
                count += 1;
            }
        }
    }
    Err(format!("{descriptor}: the descriptor is cut"))
}
