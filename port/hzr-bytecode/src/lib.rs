//! Read the block constants out of the player's own client.jar.
//!
//! Nothing here ships game data: every number comes from the jar the player
//! already has on their device.

pub mod classfile;
pub mod disasm;
pub mod interp;
