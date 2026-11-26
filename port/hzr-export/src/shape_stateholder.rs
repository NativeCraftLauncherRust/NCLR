//! A property-key-backed synthetic block state for `StateHolder.getValues()` /
//! `propertyKeys`, and the `MultifaceBlock` shape family it unblocks.
//!
//! The step-2b bytecode interpreter (`shape_static.rs`) reports a gap for
//! `GlowLichenBlock`, `MultifaceBlock` and `SculkVeinBlock`:
//!
//! ```text
//! arraylength on Opaque(state.propertyKeys) in StateHolder.valueIndex(Property)
//! ```
//!
//! Vanilla's `StateHolder` (26.3) is a class with two parallel arrays:
//!
//! ```text
//! private final Property<?>[]      propertyKeys;    // createBlockStateDefinition order
//! private final Comparable<?>[]    propertyValues;  // the value per key
//! ```
//!
//! `getValue(Property)` -> `getNullableValue` -> `valueIndex(Property)` scans
//! `propertyKeys` by **identity** (`if_acmpne`) and returns the slot whose
//! `propertyValues` entry is the state's value; `getValues()` maps `i ->
//! Property.Value(propertyKeys[i], propertyValues[i])`; `getProperties()` lists
//! the keys; `isSingletonState()` is `propertyKeys.length == 0`. A hand-made VM
//! state has no such arrays, so the shape function of the multiface family -
//! which asks `state.getValue(PROPERTY_BY_DIRECTION.get(direction))` for six
//! boolean properties - cannot run.
//!
//! This module builds the two arrays from the jar-derived state table
//! (`states::BlockStates`: property insertion order + per-property value lists)
//! and implements the `StateHolder` accessors on top, so any consumer can ask
//! the same questions vanilla's code asks:
//!
//! * [`SyntheticState::property_keys`] / [`SyntheticState::property_values`] -
//!   the two arrays;
//! * [`SyntheticState::value_index`] - the identity lookup (`-1` equivalent is
//!   `None`);
//! * [`SyntheticState::get_value`] / `has_property` / `get_optional_value` /
//!   `get_value_or_else`;
//! * [`SyntheticState::get_values`] - the `(key, value)` pairs `getValues()`
//!   streams;
//! * [`SyntheticState::get_properties`] / `is_singleton_state`.
//!
//! Property identity is modelled as the property's slot in the block's
//! `createBlockStateDefinition` order (with its owner class name for
//! diagnostics), which is exactly what an array identity scan can observe: a
//! block has at most one property per name, and no two slots are equal.
//!
//! # The unblocked family
//!
//! `MultifaceBlock.getShape` (inherited by `GlowLichenBlock` through
//! `MultifaceSpreadeableBlock`, and by `SculkVeinBlock`) is
//! `shapes.apply(state)` where `shapes` maps a state to:
//!
//! ```text
//! VoxelShape result = Shapes.empty();
//! for (Direction direction : DIRECTIONS)          // DOWN, UP, NORTH, SOUTH, WEST, EAST
//!     if (hasFace(state, direction))
//!         result = Shapes.or(result, shapes.get(direction));
//! return result.isEmpty() ? Shapes.block() : result;
//! ```
//!
//! `hasFace` is `state.getValue(PROPERTY_BY_DIRECTION.get(direction))`; the base
//! plate is `Block.boxZ(16, 0, 1)` (`box(0, 0, 0, 16, 16, 1)`) rotated by
//! `Shapes.rotateAll`: north = `z 0..1`, south = `z 15..16`, west = `x 0..1`,
//! east = `x 15..16`, up = `y 15..16`, down = `y 0..1` (1/16 units).
//!
//! [`multiface_occlusion_table`] derives the occlusion shape of every state of
//! those three classes through the synthetic state. The prepared
//! `occlusion.nbt` carries **384** such states (3 classes x 128 states: six face
//! booleans + waterlogged) and the test compares all of them exactly.

#![allow(dead_code, clippy::duplicate_mod)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

#[path = "shape_families.rs"]
mod shape_families;

#[path = "geometry.rs"]
pub mod geometry;

#[path = "states.rs"]
pub mod states;

#[path = "block.rs"]
pub mod block;

use shape_families::VoxelShape;

/// The classes whose shape function needs the property-key arrays.
pub const MULTIFACE_CLASSES: [&str; 3] = ["GlowLichenBlock", "MultifaceBlock", "SculkVeinBlock"];

/// Whether one of the three shape functions can use this module's state arrays.
pub fn is_multiface_class(class: &str) -> bool {
    MULTIFACE_CLASSES.contains(&class)
}

// ---------------------------------------------------------------------------
// The synthetic state
// ---------------------------------------------------------------------------

/// The type of a property, with the value list in `Property.getPossibleValues()`
/// order (which is the ordinal order the state ids enumerate).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PropertyKind {
    /// `BooleanProperty`; `Property.values` lists `true`, `false`.
    Bool,
    /// `IntegerProperty` over `min..=max`, in increasing order.
    Int,
    /// `EnumProperty` with `values` serialized names in ordinal order.
    Enum { values: Vec<String> },
}

/// One entry of `StateHolder.propertyKeys`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PropertyKey {
    /// Slot in `createBlockStateDefinition` order (the array index).
    pub ordinal: usize,
    /// The property's name (`north`, `waterlogged`, `age`, 閳?.
    pub name: String,
    pub kind: PropertyKind,
}

/// One entry of `StateHolder.propertyValues`, in `Comparable` terms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StateValue {
    Bool(bool),
    Int(i32),
    Enum {
        /// Ordinal in the property's value list.
        index: usize,
        /// Serialized name.
        name: String,
    },
}

impl StateValue {
    /// The serialized value the prepared tables use.
    pub fn serialized(&self) -> String {
        match self {
            StateValue::Bool(value) => value.to_string(),
            StateValue::Int(value) => value.to_string(),
            StateValue::Enum { name, .. } => name.clone(),
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            StateValue::Bool(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_int(&self) -> Option<i32> {
        match self {
            StateValue::Int(value) => Some(*value),
            _ => None,
        }
    }

    /// The value's ordinal in `Property.getPossibleValues()`.
    pub fn ordinal(&self) -> usize {
        match self {
            StateValue::Bool(value) => {
                // `BooleanProperty.create` lists `true` then `false`.
                if *value {
                    0
                } else {
                    1
                }
            }
            StateValue::Int(value) => *value as usize,
            StateValue::Enum { index, .. } => *index,
        }
    }
}

/// A synthetic `BlockState`: the property-key array plus the matching values.
///
/// This is the data `StateHolder` keeps in its two private arrays; the accessors
/// below mirror its methods (including the identity scan in `valueIndex`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntheticState {
    keys: Vec<PropertyKey>,
    values: Vec<StateValue>,
}

impl SyntheticState {
    /// Build the synthetic state for one state index of a block.
    ///
    /// `state_index` is the state id offset inside the block, enumerated the way
    /// `StateDefinition` does it (sorted property names, last varies fastest).
    /// The returned arrays are in `createBlockStateDefinition` order
    /// (`BlockStates::properties`), which is what `StateHolder.propertyKeys` is.
    pub fn from_index(
        block: &states::BlockStates,
        state_index: usize,
    ) -> Result<SyntheticState, String> {
        if state_index >= block.state_count {
            return Err(format!(
                "{}: state index {state_index} out of range ({} states)",
                block.id, block.state_count
            ));
        }
        let sorted = block.sorted();
        let mut remainder = state_index;
        let mut value_ordinals: BTreeMap<String, usize> = BTreeMap::new();
        for property in sorted.iter().rev() {
            let count = property.values.len().max(1);
            value_ordinals.insert(property.name.clone(), remainder % count);
            remainder /= count;
        }
        let mut values = Vec::with_capacity(block.properties.len());
        for property in &block.properties {
            let value_index = *value_ordinals.get(&property.name).ok_or_else(|| {
                format!("{}: state is missing property {}", block.id, property.name)
            })?;
            let serialized = property.values.get(value_index).ok_or_else(|| {
                format!(
                    "{}: value index {value_index} out of range for {}",
                    block.id, property.name
                )
            })?;
            values.push(parse_value(property, serialized)?);
        }
        Ok(SyntheticState {
            keys: property_keys(block),
            values,
        })
    }

    /// Build the synthetic state from serialized property values (the shape the
    /// VM carries per state).
    pub fn from_values(
        block: &states::BlockStates,
        values: &BTreeMap<String, String>,
    ) -> Result<SyntheticState, String> {
        let keys = property_keys(block);
        let mut state_values = Vec::with_capacity(keys.len());
        for key in &keys {
            let serialized = values
                .get(&key.name)
                .ok_or_else(|| format!("{}: no value for property {}", block.id, key.name))?;
            state_values.push(parse_serialized(key, serialized)?);
        }
        Ok(SyntheticState {
            keys,
            values: state_values,
        })
    }

    /// `StateHolder.propertyKeys`.
    pub fn property_keys(&self) -> &[PropertyKey] {
        &self.keys
    }

    /// `StateHolder.propertyValues`.
    pub fn property_values(&self) -> &[StateValue] {
        &self.values
    }

    /// `StateHolder.isSingletonState()`: `propertyKeys.length == 0`.
    pub fn is_singleton_state(&self) -> bool {
        self.keys.is_empty()
    }

    /// `StateHolder.valueIndex(Property)`: identity scan of `propertyKeys`.
    ///
    /// Property identity is the slot in the block's property list; a name is
    /// unique inside one block, so the scan is by name over the array order.
    /// Returns `None` where vanilla returns `-1`.
    pub fn value_index(&self, name: &str) -> Option<usize> {
        let mut index = 0usize;
        while index < self.keys.len() {
            if self.keys[index].name == name {
                return Some(index);
            }
            index += 1;
        }
        None
    }

    /// `StateHolder.hasProperty(Property)`.
    pub fn has_property(&self, name: &str) -> bool {
        self.value_index(name).is_some()
    }

    /// `StateHolder.getValue(Property)`.
    pub fn get_value(&self, name: &str) -> Option<&StateValue> {
        self.value_index(name).map(|index| &self.values[index])
    }

    /// `StateHolder.getNullableValue(Property)` as an `Option`.
    pub fn get_optional_value(&self, name: &str) -> Option<&StateValue> {
        self.get_value(name)
    }

    /// `StateHolder.getValueOrElse(Property, fallback)`.
    pub fn get_value_or_else<'a>(&'a self, name: &str, fallback: &'a StateValue) -> &'a StateValue {
        self.get_value(name).unwrap_or(fallback)
    }

    /// `StateHolder.getProperties()`.
    pub fn get_properties(&self) -> &[PropertyKey] {
        &self.keys
    }

    /// `StateHolder.getValues()`: the `(property, value)` pairs, in key order.
    pub fn get_values(&self) -> Vec<(&PropertyKey, &StateValue)> {
        self.keys.iter().zip(self.values.iter()).collect::<Vec<_>>()
    }

    /// `StateHolder.setValue(Property, value)` as a new state (the array slot is
    /// replaced, exactly like vanilla's neighbour lookup).
    pub fn set_value(&self, name: &str, value: StateValue) -> Result<SyntheticState, String> {
        let index = self
            .value_index(name)
            .ok_or_else(|| format!("setValue on unknown property {name}"))?;
        let mut next = self.clone();
        next.values[index] = value;
        Ok(next)
    }
}

fn property_keys(block: &states::BlockStates) -> Vec<PropertyKey> {
    block
        .properties
        .iter()
        .enumerate()
        .map(|(ordinal, property)| PropertyKey {
            ordinal,
            name: property.name.clone(),
            kind: kind_of(property),
        })
        .collect()
}

fn kind_of(property: &states::Property) -> PropertyKind {
    let is_boolean = property.values.len() == 2
        && property.values.iter().any(|value| value == "true")
        && property.values.iter().any(|value| value == "false");
    if is_boolean {
        return PropertyKind::Bool;
    }
    if !property.values.is_empty()
        && property
            .values
            .iter()
            .all(|value| value.parse::<i32>().is_ok())
    {
        return PropertyKind::Int;
    }
    PropertyKind::Enum {
        values: property.values.clone(),
    }
}

fn parse_value(property: &states::Property, serialized: &str) -> Result<StateValue, String> {
    let key = PropertyKey {
        ordinal: 0,
        name: property.name.clone(),
        kind: kind_of(property),
    };
    parse_serialized(&key, serialized)
}

fn parse_serialized(key: &PropertyKey, serialized: &str) -> Result<StateValue, String> {
    match &key.kind {
        PropertyKind::Bool => match serialized {
            "true" => Ok(StateValue::Bool(true)),
            "false" => Ok(StateValue::Bool(false)),
            other => Err(format!("property {}: {other} is not a boolean", key.name)),
        },
        PropertyKind::Int => serialized
            .parse::<i32>()
            .map(StateValue::Int)
            .map_err(|error| format!("property {}: {error}", key.name)),
        PropertyKind::Enum { values } => {
            let index = values
                .iter()
                .position(|value| value == serialized)
                .ok_or_else(|| format!("property {}: {serialized} is not a value", key.name))?;
            Ok(StateValue::Enum {
                index,
                name: serialized.to_string(),
            })
        }
    }
}

// ---------------------------------------------------------------------------
// The unblocked family: MultifaceBlock.getShape
// ---------------------------------------------------------------------------

/// `MultifaceBlock.DIRECTIONS` is `Direction.values()` in enum order.
pub const DIRECTIONS: [&str; 6] = ["down", "up", "north", "south", "west", "east"];

/// `Block.boxZ(16, 0, 1)`: a 1/16 plate on the north face, in block units.
fn face_plate(direction: &str) -> Option<[f64; 6]> {
    let thickness = 1.0 / 16.0;
    Some(match direction {
        "north" => [0.0, 0.0, 0.0, 1.0, 1.0, thickness],
        "south" => [0.0, 0.0, 1.0 - thickness, 1.0, 1.0, 1.0],
        "west" => [0.0, 0.0, 0.0, thickness, 1.0, 1.0],
        "east" => [1.0 - thickness, 0.0, 0.0, 1.0, 1.0, 1.0],
        "up" => [0.0, 1.0 - thickness, 0.0, 1.0, 1.0, 1.0],
        "down" => [0.0, 0.0, 0.0, 1.0, thickness, 1.0],
        _ => return None,
    })
}

/// `StateHolder.getValue` for one of the six face booleans (vanilla's `hasFace`).
fn has_face(state: &SyntheticState, direction: &str) -> bool {
    state
        .get_value(direction)
        .and_then(StateValue::as_bool)
        .unwrap_or(false)
}

/// `MultifaceBlock.getShape`: OR the plate of every set face; an empty union is
/// `Shapes.block()` (the all-false state).
pub fn multiface_shape(state: &SyntheticState) -> VoxelShape {
    let mut boxes = Vec::new();
    for direction in DIRECTIONS {
        if has_face(state, direction) {
            if let Some(plate) = face_plate(direction) {
                boxes.push(plate);
            }
        }
    }
    if boxes.is_empty() {
        VoxelShape::block()
    } else {
        VoxelShape::from_boxes(&boxes)
    }
}

/// Encode a `VoxelShape` as the dictionary entry the shape tables store.
pub fn encode_voxel(shape: &VoxelShape) -> geometry::Shape {
    let encoded = shape.encode();
    geometry::Shape {
        x: encoded.x,
        y: encoded.y,
        z: encoded.z,
        canonical: encoded.canonical,
        cube: encoded.cube,
        cube_axes: encoded.cube_axes,
        discrete_min: encoded.discrete_min,
        discrete_max: encoded.discrete_max,
        occupied: encoded.occupied,
    }
}

/// What the derivation produced.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Coverage {
    /// States with a derived occlusion shape.
    pub states: usize,
    /// Classes those states belong to.
    pub classes: BTreeSet<String>,
    /// States per class, for the report.
    pub per_class: BTreeMap<String, usize>,
}

/// Derive the occlusion shape of every state of the multiface family.
///
/// `getOcclusionShape` defaults to `getShape`, which is what
/// [`multiface_shape`] computes. The returned vector is indexed like the state
/// table; only the three classes' states are `Some`.
pub fn multiface_occlusion_table(
    jar_path: &Path,
) -> Result<(Vec<Option<geometry::Shape>>, Coverage), String> {
    let registry = block::registry(jar_path)?;
    let table = states::state_table(jar_path)?;
    if registry.len() != table.blocks.len() {
        return Err("the block registry and the state table disagree".to_string());
    }
    let total = table.state_count();
    let mut shapes: Vec<Option<geometry::Shape>> = vec![None; total];
    let mut coverage = Coverage::default();
    let mut index = 0usize;
    for (block_index, row) in table.blocks.iter().enumerate() {
        let class = &registry[block_index].class;
        let covered = is_multiface_class(class);
        for state_index in 0..row.state_count {
            if covered {
                let state = SyntheticState::from_index(row, state_index)?;
                shapes[index] = Some(encode_voxel(&multiface_shape(&state)));
                coverage.states += 1;
                coverage.classes.insert(class.clone());
                *coverage.per_class.entry(class.clone()).or_insert(0) += 1;
            }
            index += 1;
        }
    }
    Ok((shapes, coverage))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_boolean_state_has_a_key_per_property_in_definition_order() {
        let block = states::BlockStates {
            id: "minecraft:test".to_string(),
            properties: vec![
                states::Property {
                    name: "waterlogged".to_string(),
                    values: vec!["true".to_string(), "false".to_string()],
                },
                states::Property {
                    name: "north".to_string(),
                    values: vec!["true".to_string(), "false".to_string()],
                },
            ],
            state_count: 4,
            default_index: 0,
        };
        let state = SyntheticState::from_index(&block, 2).expect("state 2");
        // Sorted names (north, waterlogged) enumerate last-fastest for state ids,
        // but the arrays stay in definition order.
        assert_eq!(
            state.property_keys()[0].name,
            "waterlogged",
            "keys keep the definition order"
        );
        assert_eq!(state.value_index("waterlogged"), Some(0));
        assert_eq!(state.value_index("north"), Some(1));
        assert_eq!(state.value_index("missing"), None);
        assert_eq!(state.get_value("north"), Some(&StateValue::Bool(false)));
        assert_eq!(
            state.get_value("waterlogged"),
            Some(&StateValue::Bool(true))
        );
        assert_eq!(state.get_values().len(), 2);
        assert!(!state.is_singleton_state());
    }

    #[test]
    fn a_singleton_state_has_no_keys() {
        let block = states::BlockStates {
            id: "minecraft:stone".to_string(),
            properties: Vec::new(),
            state_count: 1,
            default_index: 0,
        };
        let state = SyntheticState::from_index(&block, 0).expect("state 0");
        assert!(state.is_singleton_state());
        assert!(state.property_keys().is_empty());
        assert!(state.get_values().is_empty());
    }

    #[test]
    fn an_integer_and_an_enum_property_keep_their_ordinals() {
        let block = states::BlockStates {
            id: "minecraft:test".to_string(),
            properties: vec![
                states::Property {
                    name: "facing".to_string(),
                    values: vec![
                        "north".to_string(),
                        "east".to_string(),
                        "south".to_string(),
                        "west".to_string(),
                    ],
                },
                states::Property {
                    name: "age".to_string(),
                    values: (0..=3).map(|value| value.to_string()).collect(),
                },
            ],
            state_count: 16,
            default_index: 0,
        };
        let state = SyntheticState::from_index(&block, 0).expect("state 0");
        assert_eq!(
            state.get_value("facing"),
            Some(&StateValue::Enum {
                index: 0,
                name: "north".to_string()
            })
        );
        assert_eq!(state.get_value("age"), Some(&StateValue::Int(0)));
        // State ids enumerate sorted names (age, facing) last-fastest.
        let state = SyntheticState::from_index(&block, 9).expect("state 9");
        assert_eq!(state.get_value("age"), Some(&StateValue::Int(2)));
        assert_eq!(
            state.get_value("facing"),
            Some(&StateValue::Enum {
                index: 1,
                name: "east".to_string()
            })
        );
    }

    #[test]
    fn set_value_replaces_only_that_slot() {
        let block = states::BlockStates {
            id: "minecraft:test".to_string(),
            properties: vec![states::Property {
                name: "north".to_string(),
                values: vec!["true".to_string(), "false".to_string()],
            }],
            state_count: 2,
            default_index: 1,
        };
        let state = SyntheticState::from_index(&block, 1).expect("state 1");
        assert_eq!(state.get_value("north"), Some(&StateValue::Bool(false)));
        let next = state
            .set_value("north", StateValue::Bool(true))
            .expect("setValue");
        assert_eq!(next.get_value("north"), Some(&StateValue::Bool(true)));
        assert_eq!(state.get_value("north"), Some(&StateValue::Bool(false)));
    }
}
