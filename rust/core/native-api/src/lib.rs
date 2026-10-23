//! Wire types of the Rubylight native API v1.
//!
//! The host serves them under [`PREFIX`] and Nightjar's `nightjar-core` reads
//! them, so this crate depends on `serde` only and builds under edition 2021
//! and Rust 1.80. The contract is `docs/native-api.md`.
//!
//! Changes within v1 are additive: response types ignore unknown fields,
//! enums decode values they don't know as `Unknown`, and request types reject
//! unknown fields so typos fail loudly.
pub mod capabilities;
pub mod error;
pub mod library;
pub mod radeon;
pub mod session;

/// The version this crate describes. It is also the `NativeApiVersion` value
/// a paired client sees in `/serverinfo`.
pub const API_VERSION: u32 = 1;
/// Path prefix of every endpoint.
pub const PREFIX: &str = "/api/native/v1";
/// The `/serverinfo` XML element that announces the API.
pub const SERVERINFO_FIELD: &str = "NativeApiVersion";

/// Feature strings in `Capabilities::features`. Clients ignore unknown ones.
pub mod feature {
    pub const LIBRARY: &str = "library";
    pub const SESSION: &str = "session";
    pub const CLOSE_GAME: &str = "session.close_game";
    pub const RADEON_READ: &str = "radeon.read";
    pub const RADEON_WRITE: &str = "radeon.write";
    /// `Session::launcher` is filled in.
    pub const SESSION_LAUNCHER: &str = "session.launcher";
    /// `Session::radeon_profile` is filled in, and apps carry a Radeon profile.
    pub const RADEON_PROFILE: &str = "radeon.profile";
}
