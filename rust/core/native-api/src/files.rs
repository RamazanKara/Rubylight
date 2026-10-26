//! `GET /files`, `POST /files` and `GET /files/content`: files a paired
//! device sends to the PC's transfer folder and fetches back.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileList {
    /// The transfer folder on the PC, for the user to find the files.
    pub folder: String,
    /// What the user may still write to the folder's volume.
    pub free_bytes: u64,
    /// The largest upload the host accepts.
    pub max_file_bytes: u64,
    /// Plain files only, by name: no folders, links or uploads in progress.
    pub files: Vec<FileEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntry {
    pub name: String,
    pub size: u64,
    pub modified_unix_ms: u64,
}

/// The `201` answer to an upload.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Uploaded {
    /// The name the file got, with ` (n)` when the sent name was taken.
    pub name: String,
    /// The bytes the host received and wrote.
    pub size: u64,
}
