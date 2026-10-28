//! Nucleus Co-op on Windows: background streams and co-op without a second
//! Windows session and without touching the PC's displays.
//!
//! The OS-independent parts (the pinned release, handler lookup, the game
//! profile, window choice and placement, the co-op session's state) live in
//! `butterpollo_core::nucleus`; this module turns them into files, processes,
//! windows and virtual pads.
pub mod desktop;
pub mod files;
pub mod install;
pub mod instances;
pub mod pads;
