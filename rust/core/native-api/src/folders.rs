//! `GET /folders` and the routes under it: the folders the PC shares with
//! this device (feature `folders`).
//!
//! A device sees only the folders it has a grant for. `GET` of
//! `/folders/{id}/entries?path=` lists a directory, and `GET` and `PUT` of
//! `/folders/{id}/content?path=` move a file (`PUT` needs `write` and answers
//! as `POST /files` does). `/folders/{id}/dav/` would serve the folder over
//! WebDAV class 1 (`folders.mount`); it is defined for 3.0, not yet served.
use serde::{Deserialize, Serialize};

/// Path under [`crate::PREFIX`].
pub const FOLDERS_PATH: &str = "/folders";
/// `/folders/{id}` + this, with `?path=`.
pub const ENTRIES_PATH: &str = "/entries";
/// `/folders/{id}` + this, with `?path=`.
pub const CONTENT_PATH: &str = "/content";
/// `/folders/{id}` + this: the WebDAV root.
pub const DAV_PATH: &str = "/dav/";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FolderList {
    pub folders: Vec<SharedFolder>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedFolder {
    /// The id the routes take; stable across renames.
    pub id: String,
    /// What the console calls the folder.
    pub name: String,
    pub access: FolderAccess,
}

/// What a grant allows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FolderAccess {
    Read,
    /// Read and write.
    Write,
    /// A value from a newer host. Clients treat it as `read`.
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FolderEntries {
    /// The listed directory, relative to the folder, with `/` separators; empty
    /// for the folder itself.
    pub path: String,
    pub entries: Vec<FolderEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FolderEntry {
    pub name: String,
    pub kind: EntryKind,
    /// Bytes; 0 for a directory.
    pub size: u64,
    pub modified_unix_ms: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EntryKind {
    File,
    Dir,
    /// A kind from a newer host.
    #[serde(other)]
    Unknown,
}
