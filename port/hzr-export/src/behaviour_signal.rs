//! `block-behaviours.json#states` redstone-signal columns, decoded from the jar.
//!
//! `VanillaBlockBehaviourExporter.states()` writes one 9-column row per registry
//! state:
//!
//! ```text
//! [sturdy, flags, fluidId, fluidAmount, falling, weak, strong, signalError, instrument]
//! ```
//!
//! This module produces the three signal columns (and flag bit 8, which is the
//! source-gate for them). The exporter queries every state over
//! `EmptyBlockGetter.INSTANCE` at `BlockPos.ZERO`, where neighbours are air and
//! `getBlockEntity` is `null`; that query context is part of the specification
//! below:
//!
//! ```text
//! if (state.isSignalSource()) {                                   // flags bit 8
//!     for (Direction d : Direction.values()) {                    // enum order
//!         weak   |= (state.getSignal(level, ZERO, d)       & 15) << (4 * d.ordinal());
//!         strong |= (state.getDirectSignal(level, ZERO, d) & 15) << (4 * d.ordinal());
//!     }
//! }                                                               // else weak = strong = 0
//! signalError = either call threw                               // never true in 26.3
//! ```
//!
//! `Direction.values()` is `DOWN, UP, NORTH, SOUTH, WEST, EAST`, so nibble 0 is
//! DOWN, nibble 1 UP, nibble 2 NORTH, nibble 3 SOUTH, nibble 4 WEST, nibble 5
//! EAST.
//!
//! `BlockBehaviour` defaults are `isSignalSource -> false`,
//! `getSignal -> ownSignal`, `ownSignal -> 0`, `getDirectSignal -> 0`. Every
//! block class that changes them is decoded here with `javap -p -c -constants`
//! from `mc263/client.jar` (26.3, DataVersion 5023):
//!
//! | rule | class chain trigger | decoded behaviour over `EmptyBlockGetter` |
//! | --- | --- | --- |
//! | `Powered` | `PoweredBlock` | `ownSignal = 15` |
//! | `Wire` | `RedstoneWireBlock` | `getSignal = d != DOWN && power > 0 && (d == UP \|\| connection(d.opposite)) ? power : 0`; the connection state is rebuilt at the queried level, and over air it is a dot only when the state itself has no horizontal connection, so all horizontal sides answer `power` unless the state is a dot; `getDirectSignal` re-enters `getSignal` |
//! | `Target` | `TargetBlock` | `ownSignal = output power` |
//! | `Daylight` | `DaylightDetectorBlock` | `ownSignal = POWER` |
//! | `SculkSensor` | `SculkSensorBlock` | `ownSignal = POWER`, `getDirectSignal = d == UP ? getSignal : 0` |
//! | `CalibratedSculkSensor` | `CalibratedSculkSensorBlock` | `getSignal = d != FACING ? POWER : 0`, direct signal from `SculkSensorBlock` (UP) |
//! | `TripWireHook` | `TripWireHookBlock` | `ownSignal = powered ? 15 : 0`, `getDirectSignal = powered && d == FACING` |
//! | `DetectorRail` | `DetectorRailBlock` | `ownSignal = powered ? 15 : 0`, `getDirectSignal = powered && d == UP` |
//! | `Lectern` | `LecternBlock` | `ownSignal = powered ? 15 : 0`, `getDirectSignal = powered && d == UP` |
//! | `Observer` | `ObserverBlock` | `ownSignal = powered ? 15 : 0`, `getSignal = d == FACING ? ownSignal : 0`, `getDirectSignal = getSignal` |
//! | `Repeater` | `RepeaterBlock` | `DiodeBlock`: `ownSignal = powered ? getOutputSignal(15) : 0`, `getSignal = d == FACING ? ownSignal : 0`, `getDirectSignal = getSignal` |
//! | `Comparator` | `ComparatorBlock` | `getOutputSignal` reads the comparator block entity; the exporter's `getBlockEntity` is `null`, so `ownSignal` (and therefore both columns) is 0 |
//! | `PressurePlate` | `PressurePlateBlock` | `ownSignal = getSignalForState = powered ? 15 : 0`, `getDirectSignal = d == UP ? getSignalForState : 0` |
//! | `WeightedPressurePlate` | `WeightedPressurePlateBlock` | `getSignalForState = POWER`, direct signal UP |
//! | `Button` | `ButtonBlock` | `ownSignal = powered ? 15 : 0`, `getDirectSignal = powered && d == getConnectedDirection` |
//! | `Lever` | `LeverBlock` | same as `Button` |
//! | `RedstoneTorch` | `RedstoneTorchBlock` | `ownSignal = lit ? 15 : 0`, `getSignal = d != UP ? ownSignal : 0`, `getDirectSignal = d == DOWN ? getSignal : 0` |
//! | `RedstoneWallTorch` | `RedstoneWallTorchBlock` | `getSignal = d != FACING ? lit ? 15 : 0 : 0`, direct signal DOWN (inherited) |
//! | `LightningRod` | `LightningRodBlock` | `ownSignal = powered ? 15 : 0`, `getDirectSignal = powered && d == FACING`; covers `WeatheringLightningRodBlock` too |
//! | `Jukebox` | `JukeboxBlock` | `ownSignal` reads the jukebox block entity; `null` here => 0 |
//! | `TrappedChest` | `TrappedChestBlock` | `ownSignal` counts openers through the chest block entity; `null` here => 0 |
//!
//! `getConnectedDirection(state)` is `FLOOR -> UP, CEILING -> DOWN, WALL ->
//! FACING` (`FaceAttachedHorizontalDirectionalBlock`). `RedstoneWireBlock`'s
//! `isSignalSource` returns its mutable `shouldSignal` field, which the
//! constructor initialises to `true`; the prepared table was captured with that
//! default, so the wire is a source here.
//!
//! # `signalError`
//!
//! No covered state throws `getSignal`/`getDirectSignal` over the empty getter:
//! all 2,647 signal-source states in the prepared table carry `false`. The
//! column is still part of [`SignalRow`]; a class whose calls could throw is a
//! gap (it has no rule), never a silent `false`.
//!
//! # Validation
//!
//! `tests/behaviour_signal.rs` rebuilds every state row from `mc263/client.jar`
//! (`block::registry` + `states::state_table`) and compares the four columns
//! against `tmp/ship-extract/block-behaviours.json`: **35,723/35,723 states**
//! exact for the source flag, `weak`, `strong` and `signalError`; the 2,647
//! signal-source states belong to **57 blocks** across the 21 rules below, with
//! no uncovered class.

#![allow(dead_code, clippy::duplicate_mod)]

use std::collections::BTreeMap;

/// `Direction` in `Direction.values()` order, used as the nibble index.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Direction {
    Down,
    Up,
    North,
    South,
    West,
    East,
}

impl Direction {
    /// `Direction.values()`.
    pub const ALL: [Direction; 6] = [
        Direction::Down,
        Direction::Up,
        Direction::North,
        Direction::South,
        Direction::West,
        Direction::East,
    ];

    /// The `ordinal()` the exporter shifts by.
    pub fn ordinal(self) -> usize {
        self as usize
    }

    /// The vanilla name (`Property` value of `DirectionProperty`).
    pub fn name(self) -> &'static str {
        match self {
            Direction::Down => "down",
            Direction::Up => "up",
            Direction::North => "north",
            Direction::South => "south",
            Direction::West => "west",
            Direction::East => "east",
        }
    }

    /// A `Direction` property value.
    pub fn from_name(name: &str) -> Option<Direction> {
        Direction::ALL
            .into_iter()
            .find(|direction| direction.name() == name)
    }
}

/// The block-code rule that produces a state's signal columns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rule {
    /// `PoweredBlock` (redstone block).
    Powered,
    /// `RedstoneWireBlock`.
    Wire,
    /// `TargetBlock`.
    Target,
    /// `DaylightDetectorBlock`.
    Daylight,
    /// `SculkSensorBlock`.
    SculkSensor,
    /// `CalibratedSculkSensorBlock`.
    CalibratedSculkSensor,
    /// `TripWireHookBlock`.
    TripWireHook,
    /// `DetectorRailBlock`.
    DetectorRail,
    /// `LecternBlock`.
    Lectern,
    /// `ObserverBlock`.
    Observer,
    /// `RepeaterBlock`.
    Repeater,
    /// `ComparatorBlock`.
    Comparator,
    /// `PressurePlateBlock`.
    PressurePlate,
    /// `WeightedPressurePlateBlock`.
    WeightedPressurePlate,
    /// `ButtonBlock`.
    Button,
    /// `LeverBlock`.
    Lever,
    /// `RedstoneTorchBlock`.
    RedstoneTorch,
    /// `RedstoneWallTorchBlock`.
    RedstoneWallTorch,
    /// `LightningRodBlock` / `WeatheringLightningRodBlock`.
    LightningRod,
    /// `JukeboxBlock`.
    Jukebox,
    /// `TrappedChestBlock`.
    TrappedChest,
}

impl Rule {
    /// Every rule, for coverage reports and tests.
    pub const ALL: [Rule; 21] = [
        Rule::Powered,
        Rule::Wire,
        Rule::Target,
        Rule::Daylight,
        Rule::SculkSensor,
        Rule::CalibratedSculkSensor,
        Rule::TripWireHook,
        Rule::DetectorRail,
        Rule::Lectern,
        Rule::Observer,
        Rule::Repeater,
        Rule::Comparator,
        Rule::PressurePlate,
        Rule::WeightedPressurePlate,
        Rule::Button,
        Rule::Lever,
        Rule::RedstoneTorch,
        Rule::RedstoneWallTorch,
        Rule::LightningRod,
        Rule::Jukebox,
        Rule::TrappedChest,
    ];

    /// The rule name, for coverage reports.
    pub fn name(self) -> &'static str {
        match self {
            Rule::Powered => "Powered",
            Rule::Wire => "Wire",
            Rule::Target => "Target",
            Rule::Daylight => "Daylight",
            Rule::SculkSensor => "SculkSensor",
            Rule::CalibratedSculkSensor => "CalibratedSculkSensor",
            Rule::TripWireHook => "TripWireHook",
            Rule::DetectorRail => "DetectorRail",
            Rule::Lectern => "Lectern",
            Rule::Observer => "Observer",
            Rule::Repeater => "Repeater",
            Rule::Comparator => "Comparator",
            Rule::PressurePlate => "PressurePlate",
            Rule::WeightedPressurePlate => "WeightedPressurePlate",
            Rule::Button => "Button",
            Rule::Lever => "Lever",
            Rule::RedstoneTorch => "RedstoneTorch",
            Rule::RedstoneWallTorch => "RedstoneWallTorch",
            Rule::LightningRod => "LightningRod",
            Rule::Jukebox => "Jukebox",
            Rule::TrappedChest => "TrappedChest",
        }
    }
}

/// One state's signal columns, in the exporter's order (`weak`, `strong`,
/// `signalError`) plus the source flag its `weak`/`strong` gating uses.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SignalRow {
    /// `state.isSignalSource()`: the exporter's flags bit 8, and the gate for the
    /// two masks.
    pub source: bool,
    /// Weak signal, 4 bits per [`Direction`] ordinal.
    pub weak: i64,
    /// Direct/strong signal, 4 bits per [`Direction`] ordinal.
    pub strong: i64,
    /// The exporter's `signalError` column.
    pub error: bool,
}

/// The class whose override supplies a block's signal columns, or `None` when
/// the class chain neither makes nor changes a signal source (the exporter then
/// writes `0, 0, false`).
///
/// The order is most-specific first so subclasses win: a wall torch is not a
/// floor torch, a calibrated sensor is not a plain sculk sensor, and a
/// comparator is not a repeater.
pub fn rule_for(chain: &[String]) -> Option<Rule> {
    let has = |class: &str| chain.iter().any(|entry| entry == class);
    if has("RedstoneWallTorchBlock") {
        Some(Rule::RedstoneWallTorch)
    } else if has("CalibratedSculkSensorBlock") {
        Some(Rule::CalibratedSculkSensor)
    } else if has("ComparatorBlock") {
        Some(Rule::Comparator)
    } else if has("RepeaterBlock") {
        Some(Rule::Repeater)
    } else if has("RedstoneWireBlock") {
        Some(Rule::Wire)
    } else if has("PoweredBlock") {
        Some(Rule::Powered)
    } else if has("TargetBlock") {
        Some(Rule::Target)
    } else if has("TripWireHookBlock") {
        Some(Rule::TripWireHook)
    } else if has("DaylightDetectorBlock") {
        Some(Rule::Daylight)
    } else if has("DetectorRailBlock") {
        Some(Rule::DetectorRail)
    } else if has("JukeboxBlock") {
        Some(Rule::Jukebox)
    } else if has("LecternBlock") {
        Some(Rule::Lectern)
    } else if has("ObserverBlock") {
        Some(Rule::Observer)
    } else if has("WeightedPressurePlateBlock") {
        Some(Rule::WeightedPressurePlate)
    } else if has("PressurePlateBlock") {
        Some(Rule::PressurePlate)
    } else if has("ButtonBlock") {
        Some(Rule::Button)
    } else if has("LeverBlock") {
        Some(Rule::Lever)
    } else if has("RedstoneTorchBlock") {
        Some(Rule::RedstoneTorch)
    } else if has("SculkSensorBlock") {
        Some(Rule::SculkSensor)
    } else if has("LightningRodBlock") {
        Some(Rule::LightningRod)
    } else if has("TrappedChestBlock") {
        Some(Rule::TrappedChest)
    } else {
        None
    }
}

/// `state.isSignalSource()`: whether the block class chain overrides the default
/// to a constant `true` (or, for the wire, the `shouldSignal` field that the
/// registry instance keeps `true`).
pub fn is_signal_source(chain: &[String]) -> bool {
    rule_for(chain).is_some()
}

/// The weak and strong masks of one state (`(weak, strong)`).
pub fn signal_masks(chain: &[String], values: &BTreeMap<String, String>) -> (i64, i64) {
    match rule_for(chain) {
        Some(rule) => masks(rule, values),
        None => (0, 0),
    }
}

/// The source flag and both masks of one state.
pub fn signal_row(chain: &[String], values: &BTreeMap<String, String>) -> SignalRow {
    match rule_for(chain) {
        Some(rule) => {
            let (weak, strong) = masks(rule, values);
            SignalRow {
                source: true,
                weak,
                strong,
                // No covered state throws over `EmptyBlockGetter`; see the module
                // docs. `true` is kept in the row so the column stays explicit.
                error: false,
            }
        }
        None => SignalRow::default(),
    }
}

// ---------------------------------------------------------------------------
// Masks
// ---------------------------------------------------------------------------

/// `(value & 15) << (4 * direction.ordinal())`.
fn mask(direction: Direction, value: i32) -> i64 {
    i64::from(value & 15) << (4 * direction.ordinal())
}

/// The same value on all six sides.
fn all_sides(value: i32) -> i64 {
    Direction::ALL.iter().fold(0i64, |mask_so_far, direction| {
        mask_so_far | mask(*direction, value)
    })
}

fn masks(rule: Rule, values: &BTreeMap<String, String>) -> (i64, i64) {
    let powered = flag(values, "powered");
    let lit = flag(values, "lit");
    let power = integer(values, "power").clamp(0, 15);
    let facing = direction(values, "facing");
    match rule {
        // `PoweredBlock.ownSignal` is 15; `getDirectSignal` stays 0.
        Rule::Powered => (all_sides(15), 0),
        // `ownSignal` is the state's power, on every side.
        Rule::Target | Rule::Daylight => (all_sides(power), 0),
        // `SculkSensorBlock`: power on every side, direct signal only upwards.
        Rule::SculkSensor => (all_sides(power), mask(Direction::Up, power)),
        // `CalibratedSculkSensorBlock.getSignal` answers 0 for the side it faces.
        Rule::CalibratedSculkSensor => {
            let weak = Direction::ALL
                .iter()
                .filter(|direction| Some(**direction) != facing)
                .fold(0i64, |mask_so_far, direction| {
                    mask_so_far | mask(*direction, power)
                });
            (weak, mask(Direction::Up, power))
        }
        // `RedstoneWireBlock.getSignal`: DOWN is always 0; a powered wire answers
        // every other side, except that a dot (no horizontal connection in the
        // state) only answers UP because `getConnectionState` finds no connection
        // in the air around it. `getDirectSignal` re-enters `getSignal`.
        Rule::Wire => {
            if power == 0 {
                return (0, 0);
            }
            let dot = ["north", "south", "east", "west"]
                .iter()
                .all(|name| values.get(*name).map(String::as_str) == Some("none"));
            let mut weak = mask(Direction::Up, power);
            if !dot {
                for direction in [
                    Direction::North,
                    Direction::South,
                    Direction::West,
                    Direction::East,
                ] {
                    weak |= mask(direction, power);
                }
            }
            (weak, weak)
        }
        // 15 on every side when powered, direct signal on the side it faces.
        Rule::TripWireHook | Rule::LightningRod => {
            if powered {
                (
                    all_sides(15),
                    facing.map(|side| mask(side, 15)).unwrap_or(0),
                )
            } else {
                (0, 0)
            }
        }
        // 15 on every side when powered, direct signal upwards.
        Rule::DetectorRail | Rule::Lectern | Rule::PressurePlate => {
            if powered {
                (all_sides(15), mask(Direction::Up, 15))
            } else {
                (0, 0)
            }
        }
        // `getSignalForState = POWER`, direct signal upwards.
        Rule::WeightedPressurePlate => (all_sides(power), mask(Direction::Up, power)),
        // Powered levers and buttons: all sides, direct signal along the attached
        // direction (`getConnectedDirection`).
        Rule::Button | Rule::Lever => {
            if powered {
                (all_sides(15), mask(connected_direction(values), 15))
            } else {
                (0, 0)
            }
        }
        // Diodes and observers emit 15 towards `FACING` on both columns.
        Rule::Observer | Rule::Repeater => {
            if powered {
                facing
                    .map(|side| (mask(side, 15), mask(side, 15)))
                    .unwrap_or((0, 0))
            } else {
                (0, 0)
            }
        }
        // The comparator's output signal needs its block entity, which the
        // exporter's empty getter never has; the jukebox and trapped chest read
        // theirs the same way.
        Rule::Comparator | Rule::Jukebox | Rule::TrappedChest => (0, 0),
        // Torches power everything except UP, with the direct signal DOWN.
        Rule::RedstoneTorch => {
            if lit {
                (
                    all_sides(15) & !mask(Direction::Up, 15),
                    mask(Direction::Down, 15),
                )
            } else {
                (0, 0)
            }
        }
        // The wall torch leaves out the side it is attached to (`FACING`).
        Rule::RedstoneWallTorch => {
            if lit {
                let attached = facing.map(|side| mask(side, 15)).unwrap_or(0);
                (all_sides(15) & !attached, mask(Direction::Down, 15))
            } else {
                (0, 0)
            }
        }
    }
}

/// `state.getValue(name)` as a `BooleanProperty` value.
fn flag(values: &BTreeMap<String, String>, name: &str) -> bool {
    values.get(name).is_some_and(|value| value == "true")
}

/// `state.getValue(name)` as an `IntegerProperty` value.
fn integer(values: &BTreeMap<String, String>, name: &str) -> i32 {
    values
        .get(name)
        .and_then(|value| value.parse().ok())
        .unwrap_or(0)
}

/// `state.getValue(name)` as a `DirectionProperty` value.
fn direction(values: &BTreeMap<String, String>, name: &str) -> Option<Direction> {
    values
        .get(name)
        .and_then(|value| Direction::from_name(value))
}

/// `FaceAttachedHorizontalDirectionalBlock.getConnectedDirection`: a floor
/// attachment connects upwards, a ceiling one downwards, a wall one along
/// `FACING`.
fn connected_direction(values: &BTreeMap<String, String>) -> Direction {
    match values.get("face").map(String::as_str) {
        Some("floor") => Direction::Up,
        Some("ceiling") => Direction::Down,
        _ => direction(values, "facing").unwrap_or(Direction::North),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chain(classes: &[&str]) -> Vec<String> {
        classes.iter().map(|class| class.to_string()).collect()
    }

    fn values(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect()
    }

    fn row(classes: &[&str], pairs: &[(&str, &str)]) -> SignalRow {
        signal_row(&chain(classes), &values(pairs))
    }

    #[test]
    fn a_redstone_block_powers_every_side_weakly() {
        let row = row(&["PoweredBlock", "Block"], &[]);
        assert!(row.source);
        assert_eq!(row.weak, 15 * 0x111111);
        assert_eq!(row.strong, 0);
        assert!(!row.error);
    }

    #[test]
    fn a_wire_dot_only_powers_up() {
        let dot = row(
            &["RedstoneWireBlock", "Block"],
            &[
                ("power", "7"),
                ("north", "none"),
                ("south", "none"),
                ("west", "none"),
                ("east", "none"),
            ],
        );
        assert!(dot.source);
        assert_eq!(dot.weak, 7 << 4, "UP only");
        assert_eq!(dot.strong, dot.weak);
        let connected = row(
            &["RedstoneWireBlock", "Block"],
            &[
                ("power", "7"),
                ("north", "side"),
                ("south", "none"),
                ("west", "none"),
                ("east", "none"),
            ],
        );
        assert_eq!(connected.weak, 7 * 0x111110, "every side but DOWN");
        assert_eq!(connected.strong, connected.weak);
        assert_eq!(
            row(&["RedstoneWireBlock", "Block"], &[("power", "0")]),
            SignalRow {
                source: true,
                weak: 0,
                strong: 0,
                error: false
            }
        );
    }

    #[test]
    fn torches_leave_out_their_attachment_side() {
        // A floor torch does not power the block above it.
        let torch = row(
            &["RedstoneTorchBlock", "BaseTorchBlock", "Block"],
            &[("lit", "true")],
        );
        assert_eq!(torch.weak, (15 * 0x111111) & !(15 << 4));
        assert_eq!(torch.strong, 15, "direct signal DOWN");
        // A wall torch does not power the wall it hangs on (`FACING`).
        let wall = row(
            &[
                "RedstoneWallTorchBlock",
                "RedstoneTorchBlock",
                "BaseTorchBlock",
                "Block",
            ],
            &[("lit", "true"), ("facing", "north")],
        );
        assert_eq!(wall.weak, (15 * 0x111111) & !(15 << (4 * 2)));
        assert_eq!(wall.strong, 15);
        let unlit = row(
            &["RedstoneWallTorchBlock", "RedstoneTorchBlock", "Block"],
            &[("lit", "false"), ("facing", "north")],
        );
        assert_eq!((unlit.weak, unlit.strong), (0, 0));
    }

    #[test]
    fn pressure_plates_send_their_direct_signal_up() {
        let plate = row(
            &["PressurePlateBlock", "BasePressurePlateBlock", "Block"],
            &[("powered", "true")],
        );
        assert_eq!(plate.weak, 15 * 0x111111);
        assert_eq!(plate.strong, 15 << 4);
        let weighted = row(
            &[
                "WeightedPressurePlateBlock",
                "BasePressurePlateBlock",
                "Block",
            ],
            &[("power", "5")],
        );
        assert_eq!(weighted.weak, 5 * 0x111111);
        assert_eq!(weighted.strong, 5 << 4);
    }

    #[test]
    fn a_calibrated_sensor_skips_the_side_it_faces() {
        let calibrated = row(
            &[
                "CalibratedSculkSensorBlock",
                "SculkSensorBlock",
                "BaseEntityBlock",
                "Block",
            ],
            &[("power", "9"), ("facing", "east")],
        );
        assert!(calibrated.source);
        assert_eq!(calibrated.weak, (9 * 0x111111) & !(9 << (4 * 5)));
        assert_eq!(calibrated.strong, 9 << 4);
        // The plain sensor does power every side.
        let plain = row(
            &["SculkSensorBlock", "BaseEntityBlock", "Block"],
            &[("power", "9")],
        );
        assert_eq!(plain.weak, 9 * 0x111111);
        assert_eq!(plain.strong, 9 << 4);
    }

    #[test]
    fn a_comparator_is_a_source_with_no_signal_but_a_repeater_powers_its_facing() {
        let comparator = row(
            &[
                "ComparatorBlock",
                "DiodeBlock",
                "HorizontalDirectionalBlock",
                "Block",
            ],
            &[("powered", "true"), ("facing", "north")],
        );
        assert!(comparator.source, "isSignalSource is true");
        assert_eq!((comparator.weak, comparator.strong), (0, 0));
        let repeater = row(
            &[
                "RepeaterBlock",
                "DiodeBlock",
                "HorizontalDirectionalBlock",
                "Block",
            ],
            &[("powered", "true"), ("facing", "east")],
        );
        assert_eq!(repeater.weak, 15 << (4 * 5));
        assert_eq!(repeater.strong, repeater.weak);
    }

    #[test]
    fn buttons_and_levers_use_the_connected_direction() {
        let classes = &["ButtonBlock", "HorizontalDirectionalBlock", "Block"];
        let floor = row(
            classes,
            &[("powered", "true"), ("face", "floor"), ("facing", "north")],
        );
        let ceiling = row(
            classes,
            &[
                ("powered", "true"),
                ("face", "ceiling"),
                ("facing", "north"),
            ],
        );
        let wall = row(
            classes,
            &[("powered", "true"), ("face", "wall"), ("facing", "west")],
        );
        assert_eq!(floor.strong, 15 << 4, "FLOOR connects UP");
        assert_eq!(ceiling.strong, 15, "CEILING connects DOWN");
        assert_eq!(wall.strong, 15 << (4 * 4), "WALL connects along FACING");
        assert_eq!(floor.weak, 15 * 0x111111);
        let lever = row(
            &["LeverBlock", "HorizontalDirectionalBlock", "Block"],
            &[("powered", "false"), ("face", "wall"), ("facing", "west")],
        );
        assert_eq!((lever.weak, lever.strong), (0, 0));
    }

    #[test]
    fn tripwire_hooks_and_lightning_rods_power_the_side_they_face() {
        let tripwire = row(
            &["TripWireHookBlock", "Block"],
            &[("powered", "true"), ("facing", "south")],
        );
        assert_eq!(tripwire.weak, 15 * 0x111111);
        assert_eq!(tripwire.strong, 15 << (4 * 3));
        let rod = row(
            &["WeatheringLightningRodBlock", "LightningRodBlock", "Block"],
            &[("powered", "true"), ("facing", "west")],
        );
        assert_eq!(rod.weak, 15 * 0x111111);
        assert_eq!(rod.strong, 15 << (4 * 4));
        let off = row(
            &["LightningRodBlock", "Block"],
            &[("powered", "false"), ("facing", "west")],
        );
        assert_eq!((off.weak, off.strong), (0, 0));
    }

    #[test]
    fn daylight_detectors_targets_observers_and_lecterns_follow_their_state() {
        let daylight = row(
            &["DaylightDetectorBlock", "BaseEntityBlock", "Block"],
            &[("power", "11"), ("inverted", "true")],
        );
        assert_eq!((daylight.weak, daylight.strong), (11 * 0x111111, 0));
        let target = row(&["TargetBlock", "Block"], &[("power", "3")]);
        assert_eq!((target.weak, target.strong), (3 * 0x111111, 0));
        let observer = row(
            &["ObserverBlock", "DirectionalBlock", "Block"],
            &[("powered", "true"), ("facing", "down")],
        );
        assert_eq!((observer.weak, observer.strong), (15, 15));
        let lectern = row(
            &["LecternBlock", "BaseEntityBlock", "Block"],
            &[
                ("powered", "true"),
                ("has_book", "false"),
                ("facing", "north"),
            ],
        );
        assert_eq!((lectern.weak, lectern.strong), (15 * 0x111111, 15 << 4));
    }

    #[test]
    fn the_block_entity_sources_send_nothing_through_the_empty_getter() {
        for chain in [
            &["JukeboxBlock", "BaseEntityBlock", "Block"][..],
            &["TrappedChestBlock", "ChestBlock", "Block"][..],
            &["ComparatorBlock", "DiodeBlock", "Block"][..],
        ] {
            let row = signal_row(
                &chain.iter().map(|c| c.to_string()).collect::<Vec<_>>(),
                &values(&[]),
            );
            assert!(row.source, "{chain:?} is a signal source");
            assert_eq!(
                (row.weak, row.strong, row.error),
                (0, 0, false),
                "{chain:?}"
            );
        }
    }

    #[test]
    fn ordinary_blocks_are_not_signal_sources() {
        for classes in [
            &["StairBlock", "Block"][..],
            &["LeavesBlock", "Block"][..],
            &["FenceGateBlock", "HorizontalDirectionalBlock", "Block"][..],
        ] {
            let chain = chain(classes);
            assert_eq!(rule_for(&chain), None, "{classes:?}");
            assert!(!is_signal_source(&chain));
            assert_eq!(signal_masks(&chain, &values(&[])), (0, 0));
            assert_eq!(signal_row(&chain, &values(&[])), SignalRow::default());
        }
        assert_eq!(
            rule_for(&chain(&["PoweredBlock", "Block"])),
            Some(Rule::Powered)
        );
    }

    #[test]
    fn the_direction_ordinals_match_the_enums_order() {
        assert_eq!(Direction::Down.ordinal(), 0);
        assert_eq!(Direction::Up.ordinal(), 1);
        assert_eq!(Direction::North.ordinal(), 2);
        assert_eq!(Direction::South.ordinal(), 3);
        assert_eq!(Direction::West.ordinal(), 4);
        assert_eq!(Direction::East.ordinal(), 5);
        assert_eq!(Direction::from_name("east"), Some(Direction::East));
        assert_eq!(Direction::from_name("sideways"), None);
    }
}
