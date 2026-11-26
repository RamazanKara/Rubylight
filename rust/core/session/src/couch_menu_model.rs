//! Couch menu page model, state machine, and 1x panel renderer.
//!
//! Input capture and layout primitives stay in `butterpollo_input_policy::couch_menu`,
//! while this module owns the host-facing menu pages, native-API item types,
//! and panel drawing.

pub mod model;
pub mod render;
pub mod theme_70;
mod widgets;

pub use model::{
    Action, Applies, Availability, Control, Menu, Setting, Snapshot, Strings, Target, items,
};
pub use render::render;
