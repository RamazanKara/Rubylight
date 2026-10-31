#![no_std]
//! SDL 3.4.18 (the `SDL` submodule, tag release-3.4.18) with `patches/vulkan-interop.patch`
//! applied by build.rs into the build directory, which is where `SOURCE_DIR` points. The
//! constants are those of the crates.io sdl3-src 3.4.18 that sdl3-sys reads.

/// Location of the patched SDL 3 source code
#[cfg(not(windows))]
pub const SOURCE_DIR: &str = concat!(env!("OUT_DIR"), "/SDL");
#[cfg(windows)]
pub const SOURCE_DIR: &str = concat!(env!("OUT_DIR"), "\\SDL");

/// Revision
pub const REVISION: &str = "SDL-3.4.18-nightjar-vulkan-interop";

/// Version part of the revision
pub const VERSION: &str = "3.4.18";

/// Tag part of the revision
pub const REVISION_TAG: &str = "release-3.4.18";

/// Tag part of the revision without version
pub const REVISION_TAG_BASE: &str = "release";

/// Offset from tag part of the revision
pub const REVISION_OFFSET: &str = "0";

/// Hash part of the revision
pub const REVISION_HASH: &str = "g829a65d76";
