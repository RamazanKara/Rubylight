//! Nucleus Co-op: background streaming without a second Windows session,
//! and co-op where several devices each play their own instance of a game.
//!
//! Nucleus Co-op (SplitScreen-Me, GPL-3.0) starts one instance of a game per
//! player through a per-game script, its *handler*, and ProtoInput (MIT)
//! hooks each instance so it reads only its own controller and believes it
//! has the focus. The host writes a Nucleus game profile with one player per
//! slot, each owning a virtual pad of its own, starts Nucleus with that
//! profile and streams each instance's window by window capture, so the
//! PC's own displays and the person at the PC are never touched.
//!
//! Everything here is OS-independent: the pinned release, handler lookup,
//! the profile Nucleus reads, the co-op session's state, input routing,
//! window choice and placement, and which background mode a stream gets.
pub mod coop;
pub mod handler;
pub mod install;
pub mod mode;
pub mod profile;
pub mod window;
