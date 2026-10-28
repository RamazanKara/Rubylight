//! Shared folders under the native API (`/api/native/v1/folders`): the folders the PC shares
//! with this device, their directories, and files moved in and out of them. Same pinned HTTPS
//! and client certificate as file transfer, and the same refusals ([`FileError`]); gate the
//! calls on `Feature::SharedFolders`, and uploads on the folder's [`FolderAccess::Write`].
//!
//! A path is the names below the folder joined by `/`, empty for the folder itself. The host
//! checks every name; this side refuses only what can't be sent (an id the routes can't carry)
//! and `.part` uploads, which the host would refuse anyway.
//!
//! The host serves a file whole, with its exact Content-Length, and ends the connection early if
//! the file changes; it takes no ranges. [`HostClient::save_folder_file`] therefore writes into
//! `<name>.part` and renames it only once every byte arrived, deleting it otherwise, so a retry
//! starts again from the first byte and nothing short or mixed is ever left under the real name.

use super::files::{
    decode, downloaded, uploaded, FileError, Local, TransferCancel, Uploaded, MAX_JSON_BYTES,
    MAX_LIST_BYTES, TRANSFER_TIMEOUT,
};
use super::native_api::PREFIX;
use super::transport::{self, encode_query, Upload};
use super::{Error, HostClient, READ_TIMEOUT};
use serde::Deserialize;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// The suffix of a transfer in progress, on the host and here.
pub const PART_SUFFIX: &str = ".part";
/// The most names a folder path has (the host's limit).
pub const MAX_FOLDER_DEPTH: usize = 64;

/// What a grant allows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FolderAccess {
    Read,
    /// Read and write.
    Write,
    /// A value from a newer host: treat it as `Read`.
    #[serde(other)]
    Unknown,
}

/// A folder the PC shares with this device.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct SharedFolder {
    /// The id the routes take; stable across renames.
    pub id: String,
    /// What the console calls the folder.
    pub name: String,
    pub access: FolderAccess,
}

impl SharedFolder {
    /// Whether this device may upload into the folder.
    pub fn writable(&self) -> bool {
        self.access == FolderAccess::Write
    }
}

#[derive(Deserialize)]
struct FolderList {
    folders: Vec<SharedFolder>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EntryKind {
    File,
    Dir,
    /// A kind from a newer host: neither opened nor downloaded.
    #[serde(other)]
    Unknown,
}

/// One name in a listed directory.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct FolderEntry {
    pub name: String,
    pub kind: EntryKind,
    /// Bytes; 0 for a directory.
    pub size: u64,
    pub modified_unix_ms: u64,
}

/// A directory of a shared folder: folders first, then files, each by name.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct FolderEntries {
    /// The listed directory below the folder, `/`-separated; empty for the folder itself.
    pub path: String,
    pub entries: Vec<FolderEntry>,
}

/// `name` below the directory `dir` (empty for the folder itself).
pub fn child_path(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_owned()
    } else {
        format!("{dir}/{name}")
    }
}

/// The directory holding `path`; empty for a name in the folder itself.
pub fn parent_path(path: &str) -> &str {
    path.rfind('/').map_or("", |slash| &path[..slash])
}

/// The last name of `path`.
pub fn file_name(path: &str) -> &str {
    path.rfind('/').map_or(path, |slash| &path[slash + 1..])
}

/// Whether `name` ends in [`PART_SUFFIX`], in any case, as the host checks it.
pub fn has_part_suffix(name: &str) -> bool {
    name.get(name.len().saturating_sub(PART_SUFFIX.len())..)
        .is_some_and(|end| end.eq_ignore_ascii_case(PART_SUFFIX))
}

/// The name a copy takes on its `n`th collision, as the host and Explorer number them:
/// `name (n).ext`; `n == 0` is the name itself. A leading dot starts a name, not an extension.
pub fn collision_name(name: &str, n: u32) -> String {
    if n == 0 {
        return name.to_owned();
    }
    match name.rfind('.') {
        Some(dot) if dot > 0 => format!("{} ({n}){}", &name[..dot], &name[dot..]),
        _ => format!("{name} ({n})"),
    }
}

/// A folder id as the host's setting allows it: 1 to 64 ASCII letters, digits, `-` or `_`. It
/// is a segment of the URL path, so nothing else is sent.
fn check_id(id: &str) -> Result<(), Error> {
    let valid = (1..=64).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if valid {
        Ok(())
    } else {
        Err(Error::InvalidArgument(format!(
            "not a shared folder id: {id:?}"
        )))
    }
}

/// A path the host could read: at most [`MAX_FOLDER_DEPTH`] names, none of them empty.
fn check_path(path: &str) -> Result<(), Error> {
    if path.is_empty() {
        return Ok(());
    }
    let names = path.split('/');
    if names.clone().count() > MAX_FOLDER_DEPTH || names.clone().any(str::is_empty) {
        return Err(Error::InvalidArgument(format!(
            "not a shared folder path: {path:?}"
        )));
    }
    Ok(())
}

/// Device names Windows opens in any directory, with any extension (`NUL.txt`), as the host
/// refuses them.
const RESERVED_NAMES: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$", "COM0", "COM1", "COM2", "COM3", "COM4",
    "COM5", "COM6", "COM7", "COM8", "COM9", "COM¹", "COM²", "COM³", "LPT0", "LPT1", "LPT2", "LPT3",
    "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9", "LPT¹", "LPT²", "LPT³",
];

/// A name the host sent that can be saved as one file directly in a local directory: a single
/// plain name (no separator, drive, `.` or `..`) and, for `windows`, none of `:` (a drive-relative
/// path or an alternate data stream), `<>"|?*`, control characters, a trailing dot or space
/// (Windows drops them) or a device name, which would write somewhere other than the directory.
fn check_local_name(name: &str, windows: bool) -> Result<(), Error> {
    let refuse = |why: &str| Err(Error::InvalidArgument(format!("not a file name here ({why}): {name:?}")));
    let mut components = Path::new(name).components();
    let single = matches!(
        (components.next(), components.next()),
        (Some(std::path::Component::Normal(only)), None) if only == std::ffi::OsStr::new(name)
    );
    if name.is_empty() || name.contains(['\\', '/']) || !single {
        return refuse("not a single name");
    }
    if !windows {
        return Ok(());
    }
    if name.contains(':') {
        return refuse("a colon");
    }
    if name.contains(['<', '>', '"', '|', '?', '*']) || name.chars().any(char::is_control) {
        return refuse("a character Windows refuses");
    }
    if name.ends_with(['.', ' ']) {
        return refuse("a trailing dot or space");
    }
    // Windows reads `CON.txt` and `CON .txt` as the device too.
    let stem = name.split('.').next().unwrap_or(name).trim_end_matches(' ');
    if RESERVED_NAMES.iter().any(|device| stem.eq_ignore_ascii_case(device)) {
        return refuse("a device name");
    }
    Ok(())
}

/// Picks `<dir>/<name>`, or the first numbered copy, such that neither it nor its `.part` file
/// exists, and creates the `.part` file. Returns the final path and the open `.part` file.
/// `name` must have passed [`check_local_name`]; a path that would leave `dir` is refused.
fn create_part(dir: &Path, name: &str) -> std::io::Result<(PathBuf, PathBuf, std::fs::File)> {
    for n in 0..10_000 {
        let candidate = collision_name(name, n);
        let target = dir.join(&candidate);
        let part = dir.join(format!("{candidate}{PART_SUFFIX}"));
        if target.parent() != Some(dir) || part.parent() != Some(dir) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("{candidate:?} is not a name in {}", dir.display()),
            ));
        }
        if target.exists() {
            continue;
        }
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&part)
        {
            Ok(file) => return Ok((target, part, file)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        format!("no free name for {name} in {}", dir.display()),
    ))
}

/// Deletes the `.part` file of a failed download and returns `error`, or, when the file stays,
/// an error that names it beside `error`.
fn discard_part(part: &Path, error: FileError) -> FileError {
    match std::fs::remove_file(part) {
        Ok(()) => error,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => error,
        Err(e) => FileError::Local(std::io::Error::new(
            e.kind(),
            format!("{error}; the partial file {} is left, it could not be deleted: {e}", part.display()),
        )),
    }
}

impl HostClient {
    /// `api/native/v1/folders[/{id}{route}]` and the request target with `path`.
    fn folders_target(
        &self,
        id: Option<&str>,
        route: &str,
        path: Option<&str>,
    ) -> Result<(String, String), Error> {
        let mut location = format!("{}/folders", PREFIX.trim_start_matches('/'));
        if let Some(id) = id {
            check_id(id)?;
            location.push('/');
            location.push_str(id);
            location.push_str(route);
        }
        let query = match path {
            Some(path) => {
                check_path(path)?;
                Some(format!("path={}", encode_query(path)))
            }
            None => None,
        };
        let target = self.url_target(&location, query.as_deref());
        Ok((location, target))
    }

    /// The folders this device has a grant for, in the host's order.
    pub fn shared_folders(&self) -> Result<Vec<SharedFolder>, FileError> {
        let (path, target) = self.folders_target(None, "", None)?;
        let request = self.files_request(&target, MAX_JSON_BYTES, READ_TIMEOUT)?;
        let list: FolderList = decode(transport::get(&request, &self.cancel)?, &path, 200)?;
        Ok(list.folders)
    }

    /// The directory `path` of the folder `id`.
    pub fn folder_entries(&self, id: &str, path: &str) -> Result<FolderEntries, FileError> {
        let (location, target) = self.folders_target(Some(id), "/entries", Some(path))?;
        let request = self.files_request(&target, MAX_LIST_BYTES, READ_TIMEOUT)?;
        decode(transport::get(&request, &self.cancel)?, &location, 200)
    }

    /// Downloads the file `path` of the folder `id` into `sink` and returns its length,
    /// reporting progress at 0, at most every 256 KiB and 100 ms, and at the end. A body
    /// shorter or longer than its Content-Length is an error, as is cancelling `stop`; the
    /// caller then discards what `sink` got.
    pub fn download_folder_file(
        &self,
        id: &str,
        path: &str,
        sink: &mut dyn Write,
        progress: &mut dyn FnMut(u64, u64),
        stop: &TransferCancel,
    ) -> Result<u64, FileError> {
        let (location, target) = self.folders_target(Some(id), "/content", Some(path))?;
        let request = self.files_request(&target, MAX_JSON_BYTES, TRANSFER_TIMEOUT)?;
        let mut sink = Local::new(sink);
        let mut length = 0;
        let mut counted = |done, total| {
            length = done;
            progress(done, total);
        };
        match transport::download(&request, &self.cancel, &mut sink, &mut counted, &stop.0) {
            Ok(response) => downloaded(response, &location, length),
            Err(error) => Err(sink.or(error)),
        }
    }

    /// Downloads the file `path` of the folder `id` into the directory `dir`: into
    /// `<name>.part`, renamed to `name`, or `name (n)` when that is taken, once every byte
    /// arrived. On any failure the `.part` file is deleted; one that can't be is named in the
    /// error. A name that isn't a plain file name here (`C:x`, `a:b`, `CON` on Windows) is
    /// refused before anything is written. Returns where the file is now.
    pub fn save_folder_file(
        &self,
        id: &str,
        path: &str,
        dir: &Path,
        progress: &mut dyn FnMut(u64, u64),
        stop: &TransferCancel,
    ) -> Result<PathBuf, FileError> {
        let name = file_name(path);
        check_local_name(name, cfg!(windows))?;
        let (target, part, file) = create_part(dir, name).map_err(FileError::Local)?;
        let mut sink = std::io::BufWriter::new(file);
        let result = self
            .download_folder_file(id, path, &mut sink, progress, stop)
            .and_then(|_| {
                let file = sink
                    .into_inner()
                    .map_err(|e| FileError::Local(e.into_error()))?;
                file.sync_all().map_err(FileError::Local)?;
                drop(file);
                if target.exists() {
                    return Err(FileError::Local(std::io::Error::new(
                        std::io::ErrorKind::AlreadyExists,
                        format!("{} appeared during the download", target.display()),
                    )));
                }
                std::fs::rename(&part, &target).map_err(FileError::Local)
            });
        match result {
            Ok(()) => Ok(target),
            // The download failed; a `.part` file left behind would only confuse.
            Err(error) => Err(discard_part(&part, error)),
        }
    }

    /// Uploads exactly `length` bytes of `file` as the file `path` of the folder `id`, whose
    /// directory must exist, reporting progress like `download_folder_file`. The host writes
    /// `<name>.part` and renames it once all of it arrived, numbering a taken name; the answer
    /// says the name it took. `stop` ends the upload with `Error::Cancelled`, and the host
    /// drops the partial file. Needs a `write` grant.
    pub fn upload_folder_file(
        &self,
        id: &str,
        path: &str,
        file: &mut dyn Read,
        length: u64,
        progress: &mut dyn FnMut(u64, u64),
        stop: &TransferCancel,
    ) -> Result<Uploaded, FileError> {
        let name = file_name(path);
        if name.is_empty() || has_part_suffix(name) {
            return Err(
                Error::InvalidArgument(format!("the host takes no upload named {name:?}")).into(),
            );
        }
        let (location, target) = self.folders_target(Some(id), "/content", Some(path))?;
        let request = self.files_request(&target, MAX_JSON_BYTES, TRANSFER_TIMEOUT)?;
        let mut file = Local::new(file);
        let upload = Upload {
            reader: &mut file,
            length,
            content_type: "application/octet-stream",
            method: "PUT",
        };
        match transport::upload(&request, &self.cancel, upload, progress, &stop.0) {
            Ok(response) => uploaded(response, &location, length),
            Err(error) => Err(file.or(error)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::FileRefusal;

    fn response(status: u16, body: &str) -> transport::Response {
        transport::Response {
            status,
            reason: "Reason".into(),
            content_type: Some("application/json".into()),
            body: body.as_bytes().to_vec(),
        }
    }

    #[test]
    fn the_listing_and_its_unknown_values_decode() {
        let body = r#"{"folders":[{"id":"games","name":"Games","access":"read"},
            {"id":"music","name":"Music","access":"write"},{"id":"x","name":"X","access":"admin"}]}"#;
        let list: FolderList = decode(response(200, body), "folders", 200).unwrap();
        let access: Vec<_> = list
            .folders
            .iter()
            .map(|f| (f.id.as_str(), f.access, f.writable()))
            .collect();
        assert_eq!(
            access,
            [
                ("games", FolderAccess::Read, false),
                ("music", FolderAccess::Write, true),
                ("x", FolderAccess::Unknown, false),
            ]
        );
        let body = r#"{"path":"saves","entries":[{"name":"old","kind":"dir","size":0,"modified_unix_ms":1},
            {"name":"slot1.sav","kind":"file","size":4096,"modified_unix_ms":1792000000000},
            {"name":"pipe","kind":"fifo","size":0,"modified_unix_ms":2}]}"#;
        let entries: FolderEntries = decode(response(200, body), "entries", 200).unwrap();
        assert_eq!(entries.path, "saves");
        let kinds: Vec<_> = entries
            .entries
            .iter()
            .map(|e| (e.name.as_str(), e.kind, e.size))
            .collect();
        assert_eq!(
            kinds,
            [
                ("old", EntryKind::Dir, 0),
                ("slot1.sav", EntryKind::File, 4096),
                ("pipe", EntryKind::Unknown, 0)
            ]
        );
        // A listing without its fields is malformed, never empty.
        for broken in [
            "{}",
            r#"{"folders":[{"id":"a"}]}"#,
            r#"{"path":"","entries":[{"name":"a"}]}"#,
        ] {
            assert!(
                decode::<FolderList>(response(200, broken), "folders", 200).is_err()
                    && decode::<FolderEntries>(response(200, broken), "entries", 200).is_err(),
                "{broken}"
            );
        }
    }

    #[test]
    fn the_shared_folder_refusals_are_typed() {
        let error = |status: u16, code: &str| {
            response(
                status,
                &format!(r#"{{"error":{{"code":"{code}","message":"m"}}}}"#),
            )
        };
        for (status, code, expected) in [
            (403, "folders_disabled", FileRefusal::FoldersDisabled),
            (403, "folder_unreadable", FileRefusal::FolderUnreadable),
            (403, "forbidden", FileRefusal::Forbidden),
            (404, "not_found", FileRefusal::NotFound),
            (400, "invalid_name", FileRefusal::InvalidName),
            (409, "transfer_busy", FileRefusal::TransferBusy),
        ] {
            match decode::<FolderEntries>(error(status, code), "entries", 200) {
                Err(FileError::Refused { refusal, .. }) => assert_eq!(refusal, expected, "{code}"),
                other => panic!("{code}: {other:?}"),
            }
        }
    }

    #[test]
    fn paths_join_and_split_on_slashes() {
        assert_eq!(child_path("", "saves"), "saves");
        assert_eq!(child_path("saves", "slot1.sav"), "saves/slot1.sav");
        assert_eq!(parent_path("saves/old/slot1.sav"), "saves/old");
        assert_eq!(parent_path("saves"), "");
        assert_eq!(parent_path(""), "");
        assert_eq!(file_name("saves/slot1.sav"), "slot1.sav");
        assert_eq!(file_name("readme.txt"), "readme.txt");
    }

    #[test]
    fn only_ids_and_paths_the_routes_carry_are_sent() {
        for id in ["games", "a", "A-b_9", &"x".repeat(64)] {
            assert!(check_id(id).is_ok(), "{id}");
        }
        for id in ["", "a/b", "..", "a b", "ä", "a%2F", &"x".repeat(65)] {
            assert!(
                matches!(check_id(id), Err(Error::InvalidArgument(_))),
                "{id}"
            );
        }
        for path in ["", "a", "a/b", &vec!["d"; 64].join("/")] {
            assert!(check_path(path).is_ok(), "{path}");
        }
        for path in ["/a", "a/", "a//b", &vec!["d"; 65].join("/")] {
            assert!(
                matches!(check_path(path), Err(Error::InvalidArgument(_))),
                "{path}"
            );
        }
    }

    #[test]
    fn part_names_and_copies_follow_the_host() {
        for name in ["x.part", "x.PART", ".part", "日本.Part"] {
            assert!(has_part_suffix(name), "{name}");
        }
        for name in ["x.parts", "partial.txt", "part", "x.par"] {
            assert!(!has_part_suffix(name), "{name}");
        }
        assert_eq!(collision_name("photo.jpg", 0), "photo.jpg");
        assert_eq!(collision_name("photo.jpg", 2), "photo (2).jpg");
        assert_eq!(collision_name(".config", 1), ".config (1)");
        assert_eq!(collision_name("README", 1), "README (1)");
    }

    #[test]
    fn a_download_takes_a_free_name_beside_parts_and_files() {
        let dir = std::env::temp_dir().join(format!("nightjar-folders-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), b"kept").unwrap();
        std::fs::write(dir.join("a (1).txt.part"), b"another download").unwrap();
        let (target, part, file) = create_part(&dir, "a.txt").unwrap();
        drop(file);
        assert_eq!(target, dir.join("a (2).txt"));
        assert_eq!(part, dir.join("a (2).txt.part"));
        assert!(part.exists() && !target.exists());
        assert_eq!(std::fs::read(dir.join("a.txt")).unwrap(), b"kept");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_download_saves_only_under_a_plain_local_name() {
        for name in ["a.txt", "日本.sav", ".config", "x (1).zip", "CONSOLE.txt", "COM10", "LPT"] {
            assert!(check_local_name(name, true).is_ok(), "{name}");
            assert!(check_local_name(name, false).is_ok(), "{name}");
        }
        // A drive-relative path, a stream, devices, names Windows rewrites or refuses.
        for name in [
            "C:x", "C:version.dll", "a:b", "a.txt:evil", "CON", "con.txt", "CON .txt", "NUL",
            "COM1", "lpt9.log", "COM¹", "CONIN$", "CONOUT$", "x.", "x ", "a<b", "a>b", "a\"b",
            "a|b", "a?b", "a*b", "a\u{1}b", "a\tb",
        ] {
            assert!(
                matches!(check_local_name(name, true), Err(Error::InvalidArgument(_))),
                "{name:?}"
            );
        }
        // Anywhere: nothing but one name.
        for name in ["", ".", "..", "a/b", "a\\b", "/etc", "\\\\server\\share"] {
            for windows in [false, true] {
                assert!(
                    matches!(check_local_name(name, windows), Err(Error::InvalidArgument(_))),
                    "{name:?} windows {windows}"
                );
            }
        }
        // What this platform's paths would read as more than one name.
        assert_eq!(check_local_name("C:x", false).is_ok(), !cfg!(windows));
    }

    #[test]
    fn a_name_that_would_leave_the_directory_is_never_created() {
        let dir = std::env::temp_dir().join(format!("nightjar-folders-out-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let outside = if cfg!(windows) { "C:x.txt" } else { "/x.txt" };
        let error = create_part(&dir, outside).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput, "{error}");
        assert!(create_part(&dir, "../x.txt").is_err());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_part_file_that_stays_is_named_in_the_error() {
        let dir = std::env::temp_dir().join(format!("nightjar-folders-part-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let failed = || FileError::Host(Error::Cancelled);
        // Deleted, or already gone: the download's own error.
        let part = dir.join("a.txt.part");
        std::fs::write(&part, b"half").unwrap();
        assert!(matches!(discard_part(&part, failed()), FileError::Host(Error::Cancelled)));
        assert!(!part.exists());
        assert!(matches!(discard_part(&part, failed()), FileError::Host(Error::Cancelled)));
        // A `.part` that can't be deleted (here a directory) is reported, with the cause.
        std::fs::create_dir(&part).unwrap();
        match discard_part(&part, failed()) {
            FileError::Local(e) => {
                let text = e.to_string();
                assert!(text.contains("a.txt.part") && text.contains("could not be deleted"), "{text}");
                assert!(text.contains(&Error::Cancelled.to_string()), "{text}");
            }
            other => panic!("{other:?}"),
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
