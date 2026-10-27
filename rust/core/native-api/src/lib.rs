//! Wire types of the Rubylight native API v1.
//!
//! The host serves them under [`PREFIX`] and Nightjar's `nightjar-core` reads
//! them, so this crate depends on `serde` only and builds under edition 2021
//! and Rust 1.80. The contract is `docs/native-api.md`.
//!
//! Changes within v1 are additive: response types ignore unknown fields,
//! enums decode values they don't know as `Unknown`, and request types reject
//! unknown fields so typos fail loudly.
pub mod audio;
pub mod capabilities;
pub mod error;
pub mod files;
pub mod folders;
pub mod library;
pub mod radeon;
pub mod session;
pub mod usb;
pub mod wake;

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
    /// `POST /session/couch-menu` opens and closes the menu the host draws into
    /// the stream (since 1.1.0).
    pub const COUCH_MENU: &str = "session.couch_menu";
    /// `GET /wake` lists the PC's wake targets.
    pub const WAKE: &str = "wake";
    /// `GET /audio` and its writes carry `audio.night_mode`, and launches take
    /// `audioNightMode`.
    pub const AUDIO_NIGHT_MODE: &str = "audio.night_mode";
    /// `GET /audio` and its writes carry `audio.headphone_surround`, and
    /// launches take `audioHeadphoneSurround`.
    pub const AUDIO_HEADPHONE_SURROUND: &str = "audio.headphone_surround";
    /// `POST /files` answers: the host's File transfer setting is on.
    pub const FILES_UPLOAD: &str = "files.upload";
    /// `GET /files` and `GET /files/content` answer, as above.
    pub const FILES_DOWNLOAD: &str = "files.download";
    /// USB forwarding: the `0x553A` control channel and [`crate::usb::TUNNEL_PATH`]
    /// answer, while the host's `usb_forwarding` setting is on and its USB/IP
    /// driver is ready.
    pub const USB: &str = "usb";
    /// `GET` and `POST /actions/clipboard?type=png` carry images, while the host's
    /// `clipboard_image` setting is on. Defined for 3.0; not yet served.
    pub const CLIPBOARD_IMAGE: &str = "clipboard.image";
    /// The [`crate::folders`] routes browse and transfer the device's shared
    /// folders. Defined for 3.0; not yet served.
    pub const FOLDERS: &str = "folders";
    /// `/folders/{id}/dav/` mounts a shared folder over WebDAV (class 1).
    /// Defined for 3.0; not yet served.
    pub const FOLDERS_MOUNT: &str = "folders.mount";
}
