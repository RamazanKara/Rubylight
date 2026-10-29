//! Managed game files (issue #63): keys in a game's own `.ini`, such as the frame limit of an
//! upscaler or frame generation mod (OptiScaler, AMD-NR), that the host sets for a stream and
//! puts back when the session ends.
//!
//! An app lists them in `game-files`, and a device's block in `device-overrides` can list its
//! own: a file, a section, a key and a value. The value is fixed text or a number, or follows
//! the stream's frame rate: `{stream_fps}` with an optional whole offset (`{stream_fps-1}`).
//!
//! Only the declared keys change; the rest of the file stays byte for byte, with its line
//! endings, byte order mark, comments and key order. A key is found as INI readers find it:
//! `[Section]` headers and `key=value` lines, names compared without regard to ASCII case,
//! blanks around names and values ignored, the first match counting. A missing file, section
//! or key is skipped with a warning and never created; the game still starts.
//!
//! The host runs as SYSTEM, so it takes only an existing `.ini` file of at most
//! [`MAX_FILE_BYTES`] inside one of the app's folders (its working directory, the folder of its
//! program, its Steam install folder) once every link and junction is resolved. Network and
//! device paths are refused, and so are a drive's root and the Windows folder as an app's
//! folder. A file is written whole into a temporary file beside it and swapped in, so a reader
//! sees the old file or the new one, and the swap keeps the file's permissions and attributes.
//!
//! Restore is per key, not a backup of the file: before a write, the journal records each key's
//! original value and the value written. A restore puts the original back only while the key
//! still holds the written value; a value changed since (in the game's own overlay) is the
//! user's and stays. The host ties the restore to the session (`rust/host/src/game_files.rs`).
use crate::state;
use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    io::{Read, Write},
    ops::Range,
    path::{Path, PathBuf},
};

/// The app key, and the key in a device's block, that lists the entries.
pub const SETTING: &str = "game-files";
/// The most entries one list holds.
pub const MAX_ENTRIES: usize = 32;
/// The largest file the host edits.
pub const MAX_FILE_BYTES: u64 = 1 << 20;
/// The journal's file name inside the profile (config) directory.
pub const JOURNAL_FILE: &str = "game-files-journal.json";
/// The longest section, key or value.
const MAX_TEXT: usize = 256;
/// The longest path as written.
const MAX_PATH: usize = 1024;
/// The largest offset from the stream's frame rate.
const MAX_OFFSET: i64 = 1000;
const VALUE_FORMS: &str = "a value is fixed text, a number or {stream_fps} with an optional whole offset such as {stream_fps-1}";

/// One key to manage, as an app or a device's block lists it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub path: String,
    #[serde(default)]
    pub section: String,
    pub key: String,
    pub value: Value,
}

/// What an entry's value stands for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Template {
    Fixed(String),
    /// The stream's frame rate plus this offset.
    StreamFps(i64),
}

impl Template {
    pub fn parse(value: &Value) -> Result<Self> {
        let text = match value {
            Value::String(text) => text.trim().to_owned(),
            Value::Number(number) => number.to_string(),
            Value::Bool(on) => on.to_string(),
            _ => bail!("{VALUE_FORMS}"),
        };
        if let Some(inner) = text.strip_prefix('{') {
            let Some(inner) = inner.strip_suffix('}') else {
                bail!("{VALUE_FORMS}");
            };
            return placeholder(inner);
        }
        if text.contains(['{', '}']) {
            bail!("{VALUE_FORMS}");
        }
        check_text(&text, "value")?;
        Ok(Self::Fixed(text))
    }

    /// The text to write for a stream at `fps`.
    pub fn resolve(&self, fps: Option<u32>) -> Result<String> {
        match self {
            Self::Fixed(text) => Ok(text.clone()),
            Self::StreamFps(offset) => {
                let fps = fps.context("the stream did not state its frame rate")?;
                let value = i64::from(fps) + offset;
                if value < 1 {
                    bail!("the stream's frame rate {fps} with {offset:+} is below 1");
                }
                Ok(value.to_string())
            }
        }
    }
}

/// `stream_fps`, `stream_fps+N` or `stream_fps-N`; blanks do not matter.
fn placeholder(inner: &str) -> Result<Template> {
    let compact: String = inner.chars().filter(|c| !c.is_whitespace()).collect();
    let Some(rest) = compact.strip_prefix("stream_fps") else {
        bail!("{VALUE_FORMS}");
    };
    if rest.is_empty() {
        return Ok(Template::StreamFps(0));
    }
    let (sign, digits) = if let Some(digits) = rest.strip_prefix('+') {
        (1, digits)
    } else if let Some(digits) = rest.strip_prefix('-') {
        (-1, digits)
    } else {
        bail!("{VALUE_FORMS}");
    };
    if digits.is_empty() || digits.len() > 4 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        bail!("{VALUE_FORMS}");
    }
    let offset: i64 = digits.parse()?;
    if offset > MAX_OFFSET {
        bail!("the offset from the stream's frame rate is at most {MAX_OFFSET}");
    }
    Ok(Template::StreamFps(sign * offset))
}

/// One line of text of at most [`MAX_TEXT`] characters.
fn check_text(text: &str, what: &str) -> Result<()> {
    if text.chars().count() > MAX_TEXT {
        bail!("the {what} is longer than {MAX_TEXT} characters");
    }
    if text.chars().any(char::is_control) {
        bail!("the {what} must be one line of text");
    }
    Ok(())
}

/// The entries of `list`, an app's or a device block's `game-files`, checked. A null is none.
pub fn parse(list: &Value) -> Result<Vec<Entry>> {
    let items = match list {
        Value::Null => return Ok(Vec::new()),
        Value::Array(items) => items,
        _ => bail!("{SETTING} must be a list of files, sections, keys and values"),
    };
    if items.len() > MAX_ENTRIES {
        bail!("{SETTING} lists at most {MAX_ENTRIES} keys");
    }
    items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let entry: Entry = serde_json::from_value(item.clone())
                .map_err(|_| anyhow!("it needs a path, a key and a value"))
                .and_then(|entry| check_entry(&entry).map(|()| entry))
                .with_context(|| format!("{SETTING} entry {}", index + 1))?;
            Ok(entry)
        })
        .collect()
}

/// The entries an app record (`extra`) lists, checked; none when it lists none.
pub fn entries(extra: &BTreeMap<String, Value>) -> Result<Vec<Entry>> {
    extra.get(SETTING).map_or(Ok(Vec::new()), parse)
}

fn check_entry(entry: &Entry) -> Result<()> {
    check_path(&entry.path)?;
    let section = entry.section.trim();
    check_text(section, "section")?;
    if section.contains(']') {
        bail!("the section is the name between [ and ], without them");
    }
    let key = entry.key.trim();
    if key.is_empty() {
        bail!("the key is empty");
    }
    check_text(key, "key")?;
    if key.contains('=') || key.starts_with(['[', ';', '#']) {
        bail!("the key is the name before the =");
    }
    Template::parse(&entry.value)?;
    Ok(())
}

/// Whether two entries (as JSON) name the same file, section and key. The host compares
/// names without regard to ASCII case and paths with either slash.
pub fn same_key(a: &Value, b: &Value) -> bool {
    let field = |entry: &Value, name: &str| {
        entry
            .get(name)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .replace('/', "\\")
            .to_ascii_lowercase()
    };
    ["path", "section", "key"]
        .iter()
        .all(|name| field(a, name) == field(b, name))
}

/// Check a path as written, before any file is looked at: one line, an `.ini` file, no `..`,
/// no network or device path, no stream name, and either a drive letter or a name inside the
/// app's folder.
pub fn check_path(path: &str) -> Result<()> {
    let path = path.trim();
    if path.is_empty() {
        bail!("no file is named");
    }
    if path.chars().count() > MAX_PATH || path.chars().any(char::is_control) {
        bail!("the file must be one line of at most {MAX_PATH} characters");
    }
    let bytes = path.as_bytes();
    if matches!(bytes.first(), Some(b'\\' | b'/')) {
        bail!(
            "{path}: network and device paths are not taken; start with a drive letter or name a file in the app's folder"
        );
    }
    let drive = bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/');
    if path[if drive { 2 } else { 0 }..].contains(':') {
        bail!("{path}: a colon belongs only after the drive letter");
    }
    if path.split(['\\', '/']).any(|part| part.trim() == "..") {
        bail!("{path}: .. is not taken; name the file inside the app's folder");
    }
    if !Path::new(path)
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("ini"))
    {
        bail!("{path}: only .ini files are managed");
    }
    Ok(())
}

/// Why a file is not taken.
#[derive(Debug)]
pub enum Unusable {
    /// It, or a folder to find it in, does not exist. Saving an app allows this: the game may
    /// not be installed yet.
    Missing(String),
    Refused(String),
}
impl fmt::Display for Unusable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing(why) | Self::Refused(why) => f.write_str(why),
        }
    }
}
impl std::error::Error for Unusable {}

/// Whether `path` is on a local drive.
fn local(path: &Path) -> bool {
    #[cfg(windows)]
    {
        use std::path::{Component, Prefix};
        matches!(
            path.components().next(),
            Some(Component::Prefix(prefix))
                if matches!(prefix.kind(), Prefix::VerbatimDisk(_) | Prefix::Disk(_))
        )
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        true
    }
}

/// How the host reaches game files: runs the file work it is given with the rights Windows
/// checks that work against, or fails without running it. The host acts as the signed-in
/// Windows user: a game's folder is usually the user's to change, so a link or junction put in
/// it between a check and a write could otherwise lead a host running as SYSTEM to any file.
/// As the user, it leads nowhere the user could not write anyway. The journal is the host's
/// own and is not reached this way.
pub type Access = fn(&mut dyn FnMut()) -> std::io::Result<()>;

/// [`Access`] as the calling process, for tests and tools.
pub fn as_host(work: &mut dyn FnMut()) -> std::io::Result<()> {
    work();
    Ok(())
}

/// Run `work` through `access`.
fn reach<T>(access: Access, work: impl FnOnce() -> T) -> std::io::Result<T> {
    let mut work = Some(work);
    let mut done = None;
    access(&mut || done = work.take().map(|work| work()))?;
    done.ok_or_else(|| std::io::Error::other("the game file work did not run"))
}

/// The folders, of `candidates`, that an app's files may be in: those that exist, resolved
/// through `access`, on a local drive, and neither a drive's root nor inside the Windows
/// folder.
pub fn folders(candidates: impl IntoIterator<Item = PathBuf>, access: Access) -> Vec<PathBuf> {
    let windows = std::env::var_os("SystemRoot").and_then(|root| std::fs::canonicalize(root).ok());
    let mut found: Vec<PathBuf> = Vec::new();
    for candidate in candidates {
        if candidate.as_os_str().is_empty() || !candidate.is_absolute() {
            continue;
        }
        let resolved = reach(access, || {
            std::fs::canonicalize(&candidate)
                .ok()
                .map(|folder| (folder.is_dir(), folder))
        });
        let (is_dir, folder) = match resolved {
            Ok(Some(found)) => found,
            Ok(None) => continue,
            Err(error) => {
                tracing::warn!(folder = %candidate.display(), %error, "this folder of the app cannot be read as the Windows user; managed game files are not taken from it");
                continue;
            }
        };
        let refused = !is_dir
            || !local(&folder)
            || folder.parent().is_none()
            || windows
                .as_ref()
                .is_some_and(|root| folder.starts_with(root));
        if refused {
            tracing::warn!(folder = %candidate.display(), "this folder of the app is a drive's root, the Windows folder or not a local folder; managed game files are not taken from it");
        } else if !found.contains(&folder) {
            found.push(folder);
        }
    }
    found
}

/// The file `path` names: resolved through `access`, an existing `.ini` file of at most
/// [`MAX_FILE_BYTES`] inside one of `folders` (see [`folders`]). A relative path starts at
/// `base`, the app's working directory. Without the access (nobody signed in), it is missing.
pub fn resolve(
    path: &str,
    base: Option<&Path>,
    folders: &[PathBuf],
    access: Access,
) -> Result<PathBuf, Unusable> {
    check_path(path).map_err(|error| Unusable::Refused(format!("{error:#}")))?;
    let path = path.trim();
    let written = Path::new(path);
    let candidate = if written.is_absolute() {
        written.to_path_buf()
    } else {
        let Some(base) = base.filter(|base| !base.as_os_str().is_empty()) else {
            return Err(Unusable::Missing(format!(
                "{path}: the app has no folder to find it in"
            )));
        };
        base.join(written)
    };
    reach(access, || resolve_file(path, &candidate, folders)).unwrap_or_else(|error| {
        Err(Unusable::Missing(format!(
            "{path}: the host cannot act as the Windows user: {error}"
        )))
    })
}

fn resolve_file(path: &str, candidate: &Path, folders: &[PathBuf]) -> Result<PathBuf, Unusable> {
    let file = match std::fs::canonicalize(candidate) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(Unusable::Missing(format!("{path}: no such file")));
        }
        Err(error) => return Err(Unusable::Refused(format!("{path}: {error}"))),
    };
    if !local(&file) || !folders.iter().any(|folder| file.starts_with(folder)) {
        return Err(Unusable::Refused(format!(
            "{path} is outside the app's folders (its working directory, the folder of its program and its Steam install folder)"
        )));
    }
    if !file
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("ini"))
    {
        return Err(Unusable::Refused(format!(
            "{path} leads to a file that is not an .ini file"
        )));
    }
    let metadata =
        std::fs::metadata(&file).map_err(|error| Unusable::Refused(format!("{path}: {error}")))?;
    if !metadata.is_file() {
        return Err(Unusable::Refused(format!("{path} is not a file")));
    }
    if metadata.len() > MAX_FILE_BYTES {
        return Err(Unusable::Refused(format!(
            "{path} is larger than 1 MiB, more than a settings file"
        )));
    }
    Ok(file)
}

/// A code unit of a text file: a byte (UTF-8, or another encoding that keeps ASCII as it is)
/// or a UTF-16 unit.
trait Unit: Copy {
    fn code(self) -> u32;
    fn text(units: &[Self]) -> String;
}
impl Unit for u8 {
    fn code(self) -> u32 {
        u32::from(self)
    }
    fn text(units: &[Self]) -> String {
        String::from_utf8_lossy(units).into_owned()
    }
}
impl Unit for u16 {
    fn code(self) -> u32 {
        u32::from(self)
    }
    fn text(units: &[Self]) -> String {
        String::from_utf16_lossy(units)
    }
}

fn blank(code: u32) -> bool {
    code == u32::from(b' ') || code == u32::from(b'\t')
}

fn trim<T: Unit>(text: &[T], mut range: Range<usize>) -> Range<usize> {
    while range.start < range.end && blank(text[range.start].code()) {
        range.start += 1;
    }
    while range.end > range.start && blank(text[range.end - 1].code()) {
        range.end -= 1;
    }
    range
}

fn find<T: Unit>(text: &[T], range: Range<usize>, wanted: u8) -> Option<usize> {
    range
        .into_iter()
        .find(|&at| text[at].code() == u32::from(wanted))
}

/// Where the value of `key` in `[section]` is: the first such key, blanks around the value
/// left out. An empty section stands for the keys before the first header.
fn locate<T: Unit>(text: &[T], section: &str, key: &str) -> Option<Range<usize>> {
    let (section, key) = (section.trim(), key.trim());
    let mut inside = section.is_empty();
    let mut start = 0;
    while start < text.len() {
        let newline = find(text, start..text.len(), b'\n').unwrap_or(text.len());
        let mut end = newline;
        if end > start && text[end - 1].code() == u32::from(b'\r') {
            end -= 1;
        }
        let line = trim(text, start..end);
        if !line.is_empty() {
            let first = text[line.start].code();
            if first == u32::from(b';') || first == u32::from(b'#') {
                // A comment.
            } else if first == u32::from(b'[') {
                if let Some(close) = find(text, line.start + 1..line.end, b']') {
                    let name = trim(text, line.start + 1..close);
                    inside = T::text(&text[name]).eq_ignore_ascii_case(section);
                }
            } else if inside && let Some(equals) = find(text, line.clone(), b'=') {
                let name = trim(text, line.start..equals);
                if T::text(&text[name]).eq_ignore_ascii_case(key) {
                    return Some(trim(text, equals + 1..line.end));
                }
            }
        }
        start = newline + 1;
    }
    None
}

const UTF8_BOM: &[u8] = &[0xEF, 0xBB, 0xBF];

/// Whether the file is UTF-16 by its byte order mark, and which byte order.
fn wide(bytes: &[u8]) -> Option<bool> {
    match bytes {
        [0xFF, 0xFE, ..] => Some(false),
        [0xFE, 0xFF, ..] => Some(true),
        _ => None,
    }
}

fn units16(bytes: &[u8], big_endian: bool) -> Result<Vec<u16>> {
    if bytes.len() % 2 != 0 {
        bail!("the file is not whole UTF-16 text");
    }
    Ok(bytes
        .chunks_exact(2)
        .map(|pair| {
            let pair = [pair[0], pair[1]];
            if big_endian {
                u16::from_be_bytes(pair)
            } else {
                u16::from_le_bytes(pair)
            }
        })
        .collect())
}

/// The value of `key` in `[section]` of a file's `bytes`, or `None` when it has no such key.
pub fn value_of(bytes: &[u8], section: &str, key: &str) -> Result<Option<String>> {
    match wide(bytes) {
        None => {
            let skip = if bytes.starts_with(UTF8_BOM) { 3 } else { 0 };
            let Some(range) = locate(&bytes[skip..], section, key) else {
                return Ok(None);
            };
            let value = &bytes[skip + range.start..skip + range.end];
            std::str::from_utf8(value)
                .map(|value| Some(value.to_owned()))
                .map_err(|_| anyhow!("the value of {key} is not UTF-8 text"))
        }
        Some(big_endian) => {
            let units = units16(&bytes[2..], big_endian)?;
            let Some(range) = locate(&units, section, key) else {
                return Ok(None);
            };
            String::from_utf16(&units[range])
                .map(Some)
                .map_err(|_| anyhow!("the value of {key} is not UTF-16 text"))
        }
    }
}

/// `bytes` with the value of `key` in `[section]` replaced by `value`, every other byte as it
/// was; `None` when the file has no such key.
pub fn with_value(bytes: &[u8], section: &str, key: &str, value: &str) -> Result<Option<Vec<u8>>> {
    check_text(value, "value")?;
    set_value(bytes, section, key, value)
}

/// [`with_value`] for a value read from the file before: an original goes back as it was, even
/// one longer than a value an entry may set.
fn set_value(bytes: &[u8], section: &str, key: &str, value: &str) -> Result<Option<Vec<u8>>> {
    match wide(bytes) {
        None => {
            let skip = if bytes.starts_with(UTF8_BOM) { 3 } else { 0 };
            let Some(range) = locate(&bytes[skip..], section, key) else {
                return Ok(None);
            };
            let mut out = Vec::with_capacity(bytes.len() + value.len());
            out.extend_from_slice(&bytes[..skip + range.start]);
            out.extend_from_slice(value.as_bytes());
            out.extend_from_slice(&bytes[skip + range.end..]);
            Ok(Some(out))
        }
        Some(big_endian) => {
            let units = units16(&bytes[2..], big_endian)?;
            let Some(range) = locate(&units, section, key) else {
                return Ok(None);
            };
            let mut out = bytes[..2 + range.start * 2].to_vec();
            for unit in value.encode_utf16() {
                out.extend_from_slice(&if big_endian {
                    unit.to_be_bytes()
                } else {
                    unit.to_le_bytes()
                });
            }
            out.extend_from_slice(&bytes[2 + range.end * 2..]);
            Ok(Some(out))
        }
    }
}

/// Whether `key` names a frame limit: `fps`, `framerate` or `limit` is part of it.
pub fn limits_frames(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    ["fps", "framerate", "limit"]
        .iter()
        .any(|word| key.contains(word))
}

/// A key to set for a stream: its file resolved and its value worked out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub file: PathBuf,
    pub section: String,
    pub key: String,
    pub value: String,
}
impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} [{}] {}",
            self.file.display(),
            self.section.trim(),
            self.key.trim()
        )
    }
}

/// The targets that set a frame limit other than `limiter`, the rate the host's frame limiter
/// holds the game to. 0 (no limit in most tools) is no other limit.
pub fn limit_conflicts(targets: &[Target], limiter: u32) -> impl Iterator<Item = &Target> {
    targets.iter().filter(move |target| {
        limits_frames(&target.key)
            && target
                .value
                .trim()
                .parse::<f64>()
                .is_ok_and(|value| value > 0.0 && (value - f64::from(limiter)).abs() >= 0.5)
    })
}

/// What a write or a restore did. Each line names the file, section and key.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub written: Vec<String>,
    pub restored: Vec<String>,
    /// Restores left alone: the key holds another value than the host wrote, the user's.
    pub kept: Vec<String>,
    /// Keys not written or not put back, with why; nothing of theirs changed.
    pub skipped: Vec<String>,
    /// Keys that stay journaled because their file could not be written, with why.
    pub failed: Vec<String>,
}

/// The keys `entries` set for a stream at `fps`, with each file resolved (see [`resolve`]).
/// An entry that cannot be used is skipped with a warning; the rest go on.
pub fn targets(
    entries: &[Entry],
    fps: Option<u32>,
    base: Option<&Path>,
    folders: &[PathBuf],
    access: Access,
    report: &mut Report,
) -> Vec<Target> {
    let mut found: Vec<Target> = Vec::new();
    for entry in entries {
        let named = format!(
            "{} [{}] {}",
            entry.path.trim(),
            entry.section.trim(),
            entry.key.trim()
        );
        let target = check_entry(entry)
            .and_then(|()| Template::parse(&entry.value)?.resolve(fps))
            .and_then(|value| {
                let file = resolve(&entry.path, base, folders, access)?;
                Ok(Target {
                    file,
                    section: entry.section.trim().to_owned(),
                    key: entry.key.trim().to_owned(),
                    value,
                })
            });
        match target {
            Ok(target) => {
                found.retain(|other| {
                    !(other.file == target.file && same(other, &target.section, &target.key))
                });
                found.push(target);
            }
            Err(error) => skip(report, &named, &format!("{error:#}")),
        }
    }
    found
}

fn same(target: &Target, section: &str, key: &str) -> bool {
    target.section.eq_ignore_ascii_case(section.trim())
        && target.key.eq_ignore_ascii_case(key.trim())
}

fn skip(report: &mut Report, what: &str, why: &str) {
    tracing::warn!(
        key = what,
        reason = why,
        "a managed game file key is skipped; the game starts without it"
    );
    report.skipped.push(format!("{what}: {why}"));
}

/// One key the host changed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Change {
    /// The device whose session wrote it.
    pub session: String,
    /// The file, resolved when it was written.
    pub path: PathBuf,
    pub section: String,
    pub key: String,
    /// The value before the host's first write; it goes back while the key holds `written`.
    pub original: String,
    pub written: String,
}
impl Change {
    fn is(&self, path: &Path, section: &str, key: &str) -> bool {
        self.path == path
            && self.section.trim().eq_ignore_ascii_case(section.trim())
            && self.key.trim().eq_ignore_ascii_case(key.trim())
    }
}
impl fmt::Display for Change {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} [{}] {}",
            self.path.display(),
            self.section.trim(),
            self.key.trim()
        )
    }
}

/// The keys the host changed and has not put back yet.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Journal {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub changes: Vec<Change>,
}
impl Journal {
    /// A missing file is an empty journal. An unreadable one is an error: the originals in it
    /// must not be forgotten, and nothing is written while it cannot be read.
    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read(path) {
            Ok(bytes) => match state::json_bytes(&bytes) {
                None => Ok(Self::default()),
                Some(json) => serde_json::from_slice(json)
                    .with_context(|| format!("invalid game files journal {}", path.display())),
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error).with_context(|| format!("reading {}", path.display())),
        }
    }
    /// Atomic, and synced before it returns.
    pub fn save(&self, path: &Path) -> Result<()> {
        state::write_json(
            path,
            &Self {
                version: 1,
                changes: self.changes.clone(),
            },
        )
    }
}

/// Read a file the host may edit: at most [`MAX_FILE_BYTES`].
fn read(file: &Path) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    std::fs::File::open(file)?
        .take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(std::io::Error::other("the file is larger than 1 MiB"));
    }
    Ok(bytes)
}

/// Write `bytes` over `file` in one step: a temporary file beside it, then a swap that keeps
/// the file's permissions and attributes. A read-only file is not overwritten.
fn replace(file: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let (Some(parent), Some(name)) = (file.parent(), file.file_name()) else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "the file has no folder",
        ));
    };
    let temporary = parent.join(format!(
        ".{}.{}.tmp",
        name.to_string_lossy(),
        uuid::Uuid::new_v4()
    ));
    let result = (|| {
        let mut out = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        out.write_all(bytes)?;
        out.sync_all()?;
        drop(out);
        swap(file, &temporary)
    })();
    let Err(error) = result else {
        return Ok(());
    };
    if std::fs::symlink_metadata(file).is_ok() {
        let _ = std::fs::remove_file(&temporary);
        return Err(error);
    }
    if std::fs::symlink_metadata(&temporary).is_err() {
        return Err(error);
    }
    // A swap that failed halfway (ERROR_UNABLE_TO_MOVE_REPLACEMENT, when the new file is held
    // open) has removed the file and left the new one under its temporary name: move it in.
    // The temporary file is the only copy now, so it is never deleted here.
    std::fs::rename(&temporary, file).map_err(|again| {
        std::io::Error::other(format!(
            "{error}; the file is gone and its new contents are in {}: {again}",
            temporary.display()
        ))
    })
}

#[cfg(windows)]
fn swap(file: &Path, replacement: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    unsafe extern "system" {
        fn ReplaceFileW(
            replaced: *const u16,
            replacement: *const u16,
            backup: *const u16,
            flags: u32,
            exclude: *mut std::ffi::c_void,
            reserved: *mut std::ffi::c_void,
        ) -> i32;
    }
    let wide = |path: &Path| -> std::io::Result<Vec<u16>> {
        let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
        if wide.contains(&0) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "file path contains a NUL",
            ));
        }
        wide.push(0);
        Ok(wide)
    };
    let (file, replacement) = (wide(file)?, wide(replacement)?);
    // SAFETY: both paths are NUL-terminated UTF-16 buffers that outlive the call; no backup
    // name, no flags and the two reserved pointers null, as the API requires.
    let replaced = unsafe {
        ReplaceFileW(
            file.as_ptr(),
            replacement.as_ptr(),
            std::ptr::null(),
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if replaced == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(windows))]
fn swap(file: &Path, replacement: &Path) -> std::io::Result<()> {
    std::fs::rename(replacement, file)
}

/// Put `change`'s original back into `bytes` if the key still holds what the host wrote.
fn put_back(bytes: &mut Vec<u8>, change: &Change, report: &mut Report) {
    match value_of(bytes, &change.section, &change.key) {
        Ok(Some(current)) if current == change.written => {
            match set_value(bytes, &change.section, &change.key, &change.original) {
                Ok(Some(next)) => {
                    *bytes = next;
                    report.restored.push(change.to_string());
                }
                Ok(None) => report.kept.push(format!("{change}: no longer in the file")),
                Err(error) => report.skipped.push(format!("{change}: {error:#}")),
            }
        }
        Ok(Some(current)) => {
            tracing::info!(key = %change, value = %current, written = %change.written, "a managed game file key changed since the host wrote it; the new value stays");
            report.kept.push(format!("{change}: changed to {current}"));
        }
        Ok(None) => report.kept.push(format!("{change}: no longer in the file")),
        Err(error) => report.skipped.push(format!("{change}: {error:#}")),
    }
}

/// Set `targets` for `session`'s stream (a device's uuid). For each file, the journal takes
/// every key's original before the file changes. Another session's change to one of these keys
/// goes back first, so the original recorded is the PC's own value. A key the file does not
/// have, or a file that cannot be read, is skipped with a warning; a file that cannot be
/// written is left as it was. The files are reached through `access`. Errors only when the
/// journal cannot be read: nothing is written.
pub fn apply(
    journal_path: &Path,
    session: &str,
    targets: &[Target],
    access: Access,
) -> Result<Report> {
    let mut journal = Journal::load(journal_path)?;
    let mut report = Report::default();
    let mut files: Vec<&Path> = Vec::new();
    for target in targets {
        if !files.contains(&target.file.as_path()) {
            files.push(&target.file);
        }
    }
    for file in files {
        let wanted: Vec<&Target> = targets.iter().filter(|t| t.file == file).collect();
        let found = reach(access, || {
            let bytes = read(file)?;
            // A file that is still the one resolved: a link put in its place since is not followed.
            if std::fs::canonicalize(file)? != file {
                bail!("the file was replaced by a link");
            }
            Ok(bytes)
        })
        .map_err(anyhow::Error::from)
        .and_then(|found| found);
        let bytes = match found {
            Ok(bytes) => bytes,
            Err(error) => {
                for target in &wanted {
                    skip(&mut report, &target.to_string(), &format!("{error:#}"));
                }
                continue;
            }
        };
        let mut next = journal.clone();
        let mut edited = bytes.clone();
        let mut done = Report::default();
        let others: Vec<Change> = next
            .changes
            .iter()
            .filter(|change| {
                change.session != session
                    && wanted
                        .iter()
                        .any(|target| change.is(file, &target.section, &target.key))
            })
            .cloned()
            .collect();
        for other in &others {
            put_back(&mut edited, other, &mut done);
            next.changes.retain(|change| change != other);
        }
        for target in &wanted {
            let current = match value_of(&edited, &target.section, &target.key) {
                Ok(Some(current)) => current,
                Ok(None) => {
                    skip(
                        &mut done,
                        &target.to_string(),
                        "the file has no such key in that section; it is not added",
                    );
                    continue;
                }
                Err(error) => {
                    skip(&mut done, &target.to_string(), &format!("{error:#}"));
                    continue;
                }
            };
            if current == target.value {
                continue;
            }
            let own = next.changes.iter_mut().find(|change| {
                change.session == session && change.is(file, &target.section, &target.key)
            });
            match own {
                // Written before in this session: the first original stays, unless the key was
                // changed since, which makes that value the one to go back to.
                Some(own) => {
                    if own.written != current {
                        own.original = current;
                    }
                    own.written = target.value.clone();
                }
                None => next.changes.push(Change {
                    session: session.to_owned(),
                    path: file.to_path_buf(),
                    section: target.section.clone(),
                    key: target.key.clone(),
                    original: current,
                    written: target.value.clone(),
                }),
            }
            match with_value(&edited, &target.section, &target.key, &target.value) {
                Ok(Some(changed)) => {
                    edited = changed;
                    done.written.push(format!("{target} = {}", target.value));
                }
                Ok(None) => unreachable!("the key was just found"),
                Err(error) => skip(&mut done, &target.to_string(), &format!("{error:#}")),
            }
        }
        // The journal first, still with the other sessions' entries this write puts back: a
        // crash before the swap finds the file as it was, and those entries then still match it
        // (a restore puts their originals back and finds this session's keys unwritten). After
        // the swap their keys hold this session's values, so a left-over entry is left alone.
        let mut before = next.clone();
        before.changes.extend(others.iter().cloned());
        if before != journal
            && let Err(error) = before.save(journal_path)
        {
            for target in &wanted {
                skip(
                    &mut report,
                    &target.to_string(),
                    &format!("the journal could not be written: {error:#}"),
                );
            }
            continue;
        }
        if edited != bytes
            && let Err(error) = reach(access, || replace(file, &edited)).and_then(|done| done)
        {
            for target in &wanted {
                skip(
                    &mut report,
                    &target.to_string(),
                    &format!("the file could not be written: {error}"),
                );
            }
            if before != journal
                && let Err(error) = journal.save(journal_path)
            {
                tracing::error!(
                    error = format!("{error:#}"),
                    "the game files journal could not be put back after a failed write; its entries for that file are dropped at their restore"
                );
            }
            continue;
        }
        if next != before
            && let Err(error) = next.save(journal_path)
        {
            tracing::warn!(
                error = format!("{error:#}"),
                "the game files journal still lists keys another device's stream had changed; their restore finds the new values and leaves them"
            );
        }
        journal = next;
        report.written.append(&mut done.written);
        report.restored.append(&mut done.restored);
        report.kept.append(&mut done.kept);
        report.skipped.append(&mut done.skipped);
    }
    Ok(report)
}

/// Which journaled changes a restore puts back.
#[derive(Clone, Copy, Debug)]
pub enum Scope<'a> {
    /// Every change, for a host start after a crash or a shutdown.
    All,
    /// The changes of one device's session.
    Session(&'a str),
    /// Every change but those of these devices' sessions, which still run.
    Except(&'a BTreeSet<String>),
}
impl Scope<'_> {
    fn matches(&self, change: &Change) -> bool {
        match self {
            Self::All => true,
            Self::Session(session) => change.session == *session,
            Self::Except(running) => !running.contains(&change.session),
        }
    }
}

/// What a restore finds where a journaled file was.
enum Found {
    File(Vec<u8>),
    /// A link or junction now: it is not followed.
    Link,
    /// The file is gone from a folder that is still there.
    Gone,
}

/// Look at `file` for a restore. A folder that is missing too (a drive not mounted yet) is an
/// error, so the changes stay journaled.
fn look_up(file: &Path) -> std::io::Result<Found> {
    match std::fs::canonicalize(file) {
        Ok(resolved) if resolved == file => read(file).map(Found::File),
        Ok(_) => Ok(Found::Link),
        Err(error)
            if error.kind() == std::io::ErrorKind::NotFound
                && file.parent().is_some_and(Path::is_dir) =>
        {
            Ok(Found::Gone)
        }
        Err(error) => Err(error),
    }
}

/// Put back the changes `scope` selects: an original goes back only while its key still holds
/// what the host wrote. A key changed since, a file gone or a file that is now a link leaves
/// the file as it is and drops the change. A file that cannot be reached, read or written
/// (through `access`) keeps its changes journaled for the next restore. Errors only when the
/// journal cannot be read or saved.
pub fn restore(journal_path: &Path, scope: Scope<'_>, access: Access) -> Result<Report> {
    let mut journal = Journal::load(journal_path)?;
    let mut report = Report::default();
    let mut files: Vec<PathBuf> = Vec::new();
    for change in journal.changes.iter().filter(|c| scope.matches(c)) {
        if !files.contains(&change.path) {
            files.push(change.path.clone());
        }
    }
    if files.is_empty() {
        return Ok(report);
    }
    for file in files {
        let changes: Vec<Change> = journal
            .changes
            .iter()
            .filter(|c| c.path == file && scope.matches(c))
            .cloned()
            .collect();
        let drop_them = |journal: &mut Journal| {
            journal
                .changes
                .retain(|c| !(c.path == file && scope.matches(c)));
        };
        let failed = |report: &mut Report, error: &dyn fmt::Display| {
            for change in &changes {
                tracing::error!(key = %change, %error, "a managed game file key could not be put back; it stays journaled");
                report.failed.push(format!("{change}: {error}"));
            }
        };
        let bytes = match reach(access, || look_up(&file)).and_then(|found| found) {
            Ok(Found::File(bytes)) => bytes,
            Ok(Found::Link) => {
                for change in &changes {
                    skip(
                        &mut report,
                        &change.to_string(),
                        "the file was replaced by a link; it is left alone",
                    );
                }
                drop_them(&mut journal);
                continue;
            }
            Ok(Found::Gone) => {
                for change in &changes {
                    skip(&mut report, &change.to_string(), "the file is gone");
                }
                drop_them(&mut journal);
                continue;
            }
            Err(error) => {
                failed(&mut report, &error);
                continue;
            }
        };
        let mut edited = bytes.clone();
        let mut done = Report::default();
        for change in &changes {
            put_back(&mut edited, change, &mut done);
        }
        if edited != bytes
            && let Err(error) = reach(access, || replace(&file, &edited)).and_then(|done| done)
        {
            failed(&mut report, &error);
            continue;
        }
        drop_them(&mut journal);
        report.restored.append(&mut done.restored);
        report.kept.append(&mut done.kept);
        report.skipped.append(&mut done.skipped);
    }
    journal.save(journal_path)?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const OPTISCALER: &str = "; OptiScaler settings\r\n[Framerate]\r\n; 0 = no limit\r\nFramerateLimit = 59\r\n\r\n[Upscalers]\r\nDx12Upscaler=fsr31\r\n";

    // The tests reach their files as the test process.
    fn folders(candidates: impl IntoIterator<Item = PathBuf>) -> Vec<PathBuf> {
        super::folders(candidates, as_host)
    }
    fn resolve(path: &str, base: Option<&Path>, folders: &[PathBuf]) -> Result<PathBuf, Unusable> {
        super::resolve(path, base, folders, as_host)
    }
    fn targets(
        entries: &[Entry],
        fps: Option<u32>,
        base: Option<&Path>,
        folders: &[PathBuf],
        report: &mut Report,
    ) -> Vec<Target> {
        super::targets(entries, fps, base, folders, as_host, report)
    }
    fn apply(journal: &Path, session: &str, targets: &[Target]) -> Result<Report> {
        super::apply(journal, session, targets, as_host)
    }
    fn restore(journal: &Path, scope: Scope<'_>) -> Result<Report> {
        super::restore(journal, scope, as_host)
    }
    fn no_user(_: &mut dyn FnMut()) -> std::io::Result<()> {
        Err(std::io::Error::other("nobody is signed in"))
    }

    #[test]
    fn values_are_fixed_or_follow_the_stream() {
        let parse = |value: Value| Template::parse(&value);
        assert_eq!(
            parse(json!("{stream_fps}")).unwrap(),
            Template::StreamFps(0)
        );
        assert_eq!(
            parse(json!("{stream_fps-1}")).unwrap(),
            Template::StreamFps(-1)
        );
        assert_eq!(
            parse(json!(" { stream_fps + 2 } ")).unwrap(),
            Template::StreamFps(2)
        );
        assert_eq!(
            parse(json!("{stream_fps+0}")).unwrap(),
            Template::StreamFps(0)
        );
        assert_eq!(parse(json!(59)).unwrap(), Template::Fixed("59".into()));
        assert_eq!(
            parse(json!(" auto ")).unwrap(),
            Template::Fixed("auto".into())
        );
        assert_eq!(parse(json!(true)).unwrap(), Template::Fixed("true".into()));
        for bad in [
            json!("{base_fps}"),
            json!("{stream_fps*2}"),
            json!("{stream_fps-}"),
            json!("{stream_fps-1.5}"),
            json!("{stream_fps-99999}"),
            json!("{stream_fps"),
            json!("x{stream_fps}"),
            json!("two\nlines"),
            json!(null),
            json!([1]),
        ] {
            assert!(parse(bad.clone()).is_err(), "{bad}");
        }
        assert_eq!(Template::StreamFps(-1).resolve(Some(60)).unwrap(), "59");
        assert_eq!(Template::StreamFps(0).resolve(Some(120)).unwrap(), "120");
        assert!(Template::StreamFps(-1).resolve(Some(1)).is_err());
        assert!(Template::StreamFps(0).resolve(None).is_err());
        assert_eq!(
            Template::Fixed("auto".into()).resolve(None).unwrap(),
            "auto"
        );
    }

    #[test]
    fn only_the_value_changes_and_every_other_byte_stays() {
        let bytes = OPTISCALER.as_bytes();
        assert_eq!(
            value_of(bytes, "framerate", "frameratelimit")
                .unwrap()
                .as_deref(),
            Some("59")
        );
        let edited = with_value(bytes, "Framerate", "FramerateLimit", "120")
            .unwrap()
            .unwrap();
        assert_eq!(
            String::from_utf8(edited.clone()).unwrap(),
            OPTISCALER.replace("FramerateLimit = 59", "FramerateLimit = 120")
        );
        // And back to the very bytes it had.
        let back = with_value(&edited, "Framerate", "FramerateLimit", "59")
            .unwrap()
            .unwrap();
        assert_eq!(back, bytes);
    }

    #[test]
    fn a_byte_order_mark_and_utf16_files_are_kept() {
        let mut utf8 = UTF8_BOM.to_vec();
        utf8.extend_from_slice(b"[Framerate]\nFramerateLimit=59\n");
        let edited = with_value(&utf8, "Framerate", "FramerateLimit", "120")
            .unwrap()
            .unwrap();
        assert!(edited.starts_with(UTF8_BOM));
        assert_eq!(&edited[3..], b"[Framerate]\nFramerateLimit=120\n");

        let text = "[Framerate]\r\nFramerateLimit=59\r\nName=Ça\r\n";
        for big_endian in [false, true] {
            let mut wide_bytes = if big_endian {
                vec![0xFE, 0xFF]
            } else {
                vec![0xFF, 0xFE]
            };
            for unit in text.encode_utf16() {
                wide_bytes.extend_from_slice(&if big_endian {
                    unit.to_be_bytes()
                } else {
                    unit.to_le_bytes()
                });
            }
            assert_eq!(
                value_of(&wide_bytes, "Framerate", "Name")
                    .unwrap()
                    .as_deref(),
                Some("Ça")
            );
            let edited = with_value(&wide_bytes, "Framerate", "FramerateLimit", "120")
                .unwrap()
                .unwrap();
            let back = with_value(&edited, "Framerate", "FramerateLimit", "59")
                .unwrap()
                .unwrap();
            assert_eq!(back, wide_bytes);
            assert_eq!(
                value_of(&edited, "Framerate", "FramerateLimit")
                    .unwrap()
                    .as_deref(),
                Some("120")
            );
        }
    }

    #[test]
    fn keys_are_found_as_ini_readers_find_them() {
        let text = b"Global = 1\n[First]\n;Key=commented\n#Key=also\nOther=2\n[ Second ]\n  key   =   a value  \nKey=later duplicate\n[First]\nKey=after\n[Broken\nKey=x\n";
        assert_eq!(value_of(text, "", "global").unwrap().as_deref(), Some("1"));
        assert_eq!(
            value_of(text, "second", "KEY").unwrap().as_deref(),
            Some("a value")
        );
        // The section of a header without a closing ] goes on.
        assert_eq!(
            value_of(text, "first", "key").unwrap().as_deref(),
            Some("after")
        );
        assert_eq!(value_of(text, "Third", "Key").unwrap(), None);
        assert_eq!(value_of(text, "First", "Missing").unwrap(), None);
        assert_eq!(with_value(text, "First", "Missing", "1").unwrap(), None);
        // An empty value takes the new one in its place.
        let edited = with_value(b"[A]\nKey=\nNext=1", "A", "Key", "60")
            .unwrap()
            .unwrap();
        assert_eq!(edited, b"[A]\nKey=60\nNext=1");
        assert!(value_of(b"[A]\nKey=\xff\n", "A", "Key").is_err());
    }

    #[test]
    fn entries_are_checked_when_the_app_is_saved() {
        let entry = |path: &str, section: &str, key: &str, value: Value| json!([{"path": path, "section": section, "key": key, "value": value}]);
        assert!(
            parse(&entry(
                "OptiScaler.ini",
                "Framerate",
                "FramerateLimit",
                json!("{stream_fps-1}")
            ))
            .is_ok()
        );
        assert!(
            parse(&entry(
                r"C:\Games\RDR2\OptiScaler.ini",
                "",
                "Limit",
                json!(60)
            ))
            .is_ok()
        );
        assert!(parse(&json!(null)).unwrap().is_empty());
        for (path, section, key, value) in [
            ("OptiScaler.txt", "A", "B", json!(1)),
            (r"..\OptiScaler.ini", "A", "B", json!(1)),
            (r"sub\..\..\x.ini", "A", "B", json!(1)),
            (r"\\server\share\x.ini", "A", "B", json!(1)),
            (r"\\?\C:\x.ini", "A", "B", json!(1)),
            (r"\\.\C:\x.ini", "A", "B", json!(1)),
            (r"\Games\x.ini", "A", "B", json!(1)),
            (r"C:x.ini", "A", "B", json!(1)),
            (r"x.ini:stream", "A", "B", json!(1)),
            ("", "A", "B", json!(1)),
            ("x.ini", "[A]", "B", json!(1)),
            ("x.ini", "A", "", json!(1)),
            ("x.ini", "A", "B=C", json!(1)),
            ("x.ini", "A", "B", json!("{base_fps}")),
        ] {
            assert!(
                parse(&entry(path, section, key, value.clone())).is_err(),
                "{path} {section} {key} {value}"
            );
        }
        assert!(parse(&json!({"path": "x.ini"})).is_err());
        assert!(parse(&json!([{"path": "x.ini", "key": "B"}])).is_err());
        let many: Vec<Value> = (0..=MAX_ENTRIES)
            .map(|i| json!({"path": "x.ini", "key": format!("k{i}"), "value": 1}))
            .collect();
        assert!(parse(&Value::Array(many)).is_err());
    }

    /// A game folder with an OptiScaler file, the journal beside it in another folder. The
    /// folder and the file are as a user writes them; `resolved` is the file as the host
    /// resolves it.
    struct Game {
        _root: tempfile::TempDir,
        folder: PathBuf,
        file: PathBuf,
        resolved: PathBuf,
        journal: PathBuf,
    }
    fn game() -> Game {
        let root = tempfile::tempdir().unwrap();
        let folder = root.path().join("RDR2");
        std::fs::create_dir(&folder).unwrap();
        let file = folder.join("OptiScaler.ini");
        std::fs::write(&file, OPTISCALER).unwrap();
        let journal = root.path().join("config").join(JOURNAL_FILE);
        Game {
            resolved: std::fs::canonicalize(&file).unwrap(),
            _root: root,
            file,
            journal,
            folder,
        }
    }
    fn limit(value: &str) -> Entry {
        Entry {
            path: "OptiScaler.ini".into(),
            section: "Framerate".into(),
            key: "FramerateLimit".into(),
            value: json!(value),
        }
    }
    fn current(file: &Path) -> String {
        value_of(&std::fs::read(file).unwrap(), "Framerate", "FramerateLimit")
            .unwrap()
            .unwrap()
    }

    #[test]
    fn files_resolve_only_inside_the_apps_folders() {
        let game = game();
        let folders = folders([game.folder.clone()]);
        assert_eq!(folders, [std::fs::canonicalize(&game.folder).unwrap()]);
        let resolve = |path: &str| resolve(path, Some(&game.folder), &folders);
        assert_eq!(resolve("OptiScaler.ini").unwrap(), game.resolved);
        assert_eq!(
            resolve(&game.file.to_string_lossy()).unwrap(),
            game.resolved
        );
        assert!(matches!(resolve("missing.ini"), Err(Unusable::Missing(_))));
        assert!(matches!(
            self::resolve("OptiScaler.ini", None, &folders),
            Err(Unusable::Missing(_))
        ));
        // A file beside the folder, named with its full path, is outside it.
        let outside = game.folder.parent().unwrap().join("outside.ini");
        std::fs::write(&outside, "[A]\nB=1\n").unwrap();
        assert!(matches!(
            resolve(&outside.to_string_lossy()),
            Err(Unusable::Refused(_))
        ));
        assert!(matches!(
            resolve(r"..\outside.ini"),
            Err(Unusable::Refused(_))
        ));
        // A folder named .ini is not a file.
        std::fs::create_dir(game.folder.join("dir.ini")).unwrap();
        assert!(matches!(resolve("dir.ini"), Err(Unusable::Refused(_))));
        let big = game.folder.join("big.ini");
        std::fs::write(&big, vec![b'a'; MAX_FILE_BYTES as usize + 1]).unwrap();
        assert!(matches!(resolve("big.ini"), Err(Unusable::Refused(_))));
        // A drive's root and the Windows folder are never an app's folder.
        let windows = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        assert!(self::folders([PathBuf::from(r"C:\"), PathBuf::from(&windows)]).is_empty());
        assert!(self::folders([PathBuf::from("relative")]).is_empty());
    }

    #[test]
    #[cfg(windows)]
    fn a_junction_out_of_the_folder_is_not_followed() {
        let game = game();
        let elsewhere = game.folder.parent().unwrap().join("elsewhere");
        std::fs::create_dir(&elsewhere).unwrap();
        std::fs::write(elsewhere.join("secret.ini"), "[A]\nB=1\n").unwrap();
        let link = game.folder.join("link");
        let made = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&link)
            .arg(&elsewhere)
            .output()
            .unwrap();
        assert!(made.status.success(), "{made:?}");
        let folders = folders([game.folder.clone()]);
        assert!(matches!(
            resolve(r"link\secret.ini", Some(&game.folder), &folders),
            Err(Unusable::Refused(_))
        ));
    }

    #[test]
    fn a_stream_sets_the_key_and_its_end_puts_the_original_back() {
        let game = game();
        let folders = folders([game.folder.clone()]);
        let mut report = Report::default();
        let targets = targets(
            &[limit("{stream_fps}")],
            Some(120),
            Some(&game.folder),
            &folders,
            &mut report,
        );
        assert!(report.skipped.is_empty(), "{report:?}");
        let report = apply(&game.journal, "phone", &targets).unwrap();
        assert_eq!(report.written.len(), 1, "{report:?}");
        assert_eq!(current(&game.file), "120");
        assert_eq!(
            std::fs::read_to_string(&game.file).unwrap(),
            OPTISCALER.replace("= 59", "= 120")
        );
        // The original is journaled on disk.
        let journal = Journal::load(&game.journal).unwrap();
        assert_eq!(journal.changes.len(), 1);
        assert_eq!(
            (
                journal.changes[0].original.as_str(),
                journal.changes[0].written.as_str()
            ),
            ("59", "120")
        );
        // Another device's session leaves it.
        assert_eq!(
            restore(&game.journal, Scope::Session("tv")).unwrap(),
            Report::default()
        );
        assert_eq!(current(&game.file), "120");
        let report = restore(&game.journal, Scope::Session("phone")).unwrap();
        assert_eq!(report.restored.len(), 1, "{report:?}");
        assert_eq!(std::fs::read_to_string(&game.file).unwrap(), OPTISCALER);
        assert!(Journal::load(&game.journal).unwrap().changes.is_empty());
    }

    #[test]
    fn a_value_the_user_changed_meanwhile_stays() {
        let game = game();
        let folders = folders([game.folder.clone()]);
        let targets = targets(
            &[limit("120")],
            None,
            Some(&game.folder),
            &folders,
            &mut Report::default(),
        );
        apply(&game.journal, "phone", &targets).unwrap();
        // The game's overlay saves another limit.
        let saved = OPTISCALER.replace("= 59", "= 90").replace("fsr31", "xess");
        std::fs::write(&game.file, &saved).unwrap();
        let report = restore(&game.journal, Scope::All).unwrap();
        assert!(
            report.restored.is_empty() && report.kept.len() == 1,
            "{report:?}"
        );
        assert_eq!(std::fs::read_to_string(&game.file).unwrap(), saved);
        assert!(Journal::load(&game.journal).unwrap().changes.is_empty());
    }

    #[test]
    fn a_host_start_after_a_crash_puts_back_what_the_journal_holds() {
        let game = game();
        let folders = folders([game.folder.clone()]);
        let targets = targets(
            &[limit("{stream_fps-1}")],
            Some(120),
            Some(&game.folder),
            &folders,
            &mut Report::default(),
        );
        apply(&game.journal, "phone", &targets).unwrap();
        assert_eq!(current(&game.file), "119");
        // The host stops without a restore; the next start restores everything journaled.
        let report = restore(&game.journal, Scope::All).unwrap();
        assert_eq!(report.restored.len(), 1);
        assert_eq!(std::fs::read_to_string(&game.file).unwrap(), OPTISCALER);
        // A crash after the journal and before the write leaves the file as it is.
        let mut journal = Journal::default();
        journal.changes.push(Change {
            session: "phone".into(),
            path: game.resolved.clone(),
            section: "Framerate".into(),
            key: "FramerateLimit".into(),
            original: "30".into(),
            written: "120".into(),
        });
        journal.save(&game.journal).unwrap();
        let report = restore(&game.journal, Scope::All).unwrap();
        assert!(report.restored.is_empty() && report.kept.len() == 1);
        assert_eq!(std::fs::read_to_string(&game.file).unwrap(), OPTISCALER);
        // A file gone since drops its changes.
        journal.save(&game.journal).unwrap();
        std::fs::remove_file(&game.file).unwrap();
        let report = restore(&game.journal, Scope::All).unwrap();
        assert_eq!(report.skipped.len(), 1);
        assert!(Journal::load(&game.journal).unwrap().changes.is_empty());
        // A folder gone too, as a drive not mounted yet, keeps them for a later restore.
        journal.changes[0].path = game.folder.join("unmounted").join("OptiScaler.ini");
        journal.save(&game.journal).unwrap();
        let report = restore(&game.journal, Scope::All).unwrap();
        assert_eq!(report.failed.len(), 1, "{report:?}");
        assert_eq!(
            Journal::load(&game.journal).unwrap().changes,
            journal.changes
        );
    }

    #[test]
    fn a_crash_while_another_devices_key_goes_back_leaves_the_pcs_own_value() {
        let game = game();
        let change = |session: &str, written: &str| Change {
            session: session.into(),
            path: game.resolved.clone(),
            section: "Framerate".into(),
            key: "FramerateLimit".into(),
            original: "59".into(),
            written: written.into(),
        };
        // The TV wrote 60 over the PC's 59. The handheld's write put the 59 back and journaled
        // its 120, keeping the TV's entry until the file changed. In either order, a restore
        // after a crash before the swap (the file at 60) or after it (at 120) gives the 59.
        for order in [["tv", "handheld"], ["handheld", "tv"]] {
            for on_disk in ["60", "120"] {
                let journal = Journal {
                    version: 1,
                    changes: order
                        .iter()
                        .map(|session| change(session, if *session == "tv" { "60" } else { "120" }))
                        .collect(),
                };
                journal.save(&game.journal).unwrap();
                std::fs::write(
                    &game.file,
                    OPTISCALER.replace("= 59", &format!("= {on_disk}")),
                )
                .unwrap();
                restore(&game.journal, Scope::All).unwrap();
                assert_eq!(current(&game.file), "59", "{order:?} {on_disk}");
            }
        }
    }

    #[test]
    fn an_original_goes_back_whatever_its_length() {
        let game = game();
        let long = "x".repeat(MAX_TEXT + 10);
        std::fs::write(
            &game.file,
            format!("[Framerate]\r\nFramerateLimit={long}\r\n"),
        )
        .unwrap();
        let folders = folders([game.folder.clone()]);
        let targets = targets(
            &[limit("60")],
            None,
            Some(&game.folder),
            &folders,
            &mut Report::default(),
        );
        apply(&game.journal, "phone", &targets).unwrap();
        assert_eq!(current(&game.file), "60");
        restore(&game.journal, Scope::All).unwrap();
        assert_eq!(current(&game.file), long);
    }

    #[test]
    fn without_the_windows_user_nothing_is_written_and_restores_wait() {
        let game = game();
        assert!(super::folders([game.folder.clone()], no_user).is_empty());
        let folders = folders([game.folder.clone()]);
        assert!(matches!(
            super::resolve("OptiScaler.ini", Some(&game.folder), &folders, no_user),
            Err(Unusable::Missing(_))
        ));
        let targets = targets(
            &[limit("60")],
            None,
            Some(&game.folder),
            &folders,
            &mut Report::default(),
        );
        let report = super::apply(&game.journal, "phone", &targets, no_user).unwrap();
        assert!(
            report.written.is_empty() && report.skipped.len() == 1,
            "{report:?}"
        );
        assert_eq!(std::fs::read_to_string(&game.file).unwrap(), OPTISCALER);
        assert!(Journal::load(&game.journal).unwrap().changes.is_empty());
        apply(&game.journal, "phone", &targets).unwrap();
        let report = super::restore(&game.journal, Scope::All, no_user).unwrap();
        assert_eq!(report.failed.len(), 1, "{report:?}");
        assert_eq!(current(&game.file), "60");
        restore(&game.journal, Scope::All).unwrap();
        assert_eq!(std::fs::read_to_string(&game.file).unwrap(), OPTISCALER);
    }

    #[test]
    fn a_restore_can_spare_the_sessions_still_running() {
        let game = game();
        let folders = folders([game.folder.clone()]);
        let mut upscaler = limit("xess");
        upscaler.section = "Upscalers".into();
        upscaler.key = "Dx12Upscaler".into();
        for (session, entry) in [("tv", limit("60")), ("phone", upscaler)] {
            let targets = targets(
                &[entry],
                None,
                Some(&game.folder),
                &folders,
                &mut Report::default(),
            );
            apply(&game.journal, session, &targets).unwrap();
        }
        let running = BTreeSet::from(["tv".to_owned()]);
        restore(&game.journal, Scope::Except(&running)).unwrap();
        assert_eq!(
            std::fs::read_to_string(&game.file).unwrap(),
            OPTISCALER.replace("= 59", "= 60")
        );
        restore(&game.journal, Scope::Except(&BTreeSet::new())).unwrap();
        assert_eq!(std::fs::read_to_string(&game.file).unwrap(), OPTISCALER);
    }

    #[test]
    fn a_missing_key_is_skipped_and_the_others_are_written() {
        let game = game();
        let folders = folders([game.folder.clone()]);
        let mut report = Report::default();
        let mut missing = limit("1");
        missing.key = "NotThere".into();
        let mut gone = limit("1");
        gone.path = "gone.ini".into();
        let mut upscaler = limit("xess");
        upscaler.section = "Upscalers".into();
        upscaler.key = "Dx12Upscaler".into();
        let targets = targets(
            &[missing, gone, limit("{stream_fps}"), upscaler],
            None,
            Some(&game.folder),
            &folders,
            &mut report,
        );
        // The missing file, and the rate the stream did not state, are skipped at once.
        assert_eq!(report.skipped.len(), 2, "{report:?}");
        assert_eq!(targets.len(), 2);
        let report = apply(&game.journal, "phone", &targets).unwrap();
        assert_eq!(report.skipped.len(), 1, "{report:?}");
        assert_eq!(report.written.len(), 1, "{report:?}");
        let text = std::fs::read_to_string(&game.file).unwrap();
        assert_eq!(text, OPTISCALER.replace("fsr31", "xess"));
        assert!(!text.contains("NotThere"));
    }

    #[test]
    fn another_devices_change_goes_back_before_the_next_write() {
        let game = game();
        let folders = folders([game.folder.clone()]);
        let at = |fps| {
            targets(
                &[limit("{stream_fps}")],
                Some(fps),
                Some(&game.folder),
                &folders,
                &mut Report::default(),
            )
        };
        apply(&game.journal, "tv", &at(60)).unwrap();
        assert_eq!(current(&game.file), "60");
        let report = apply(&game.journal, "handheld", &at(120)).unwrap();
        assert_eq!(
            (report.restored.len(), report.written.len()),
            (1, 1),
            "{report:?}"
        );
        assert_eq!(current(&game.file), "120");
        let journal = Journal::load(&game.journal).unwrap();
        assert_eq!(journal.changes.len(), 1);
        assert_eq!(journal.changes[0].session, "handheld");
        // The original is the PC's own value, not the TV's.
        assert_eq!(journal.changes[0].original, "59");
        restore(&game.journal, Scope::Session("tv")).unwrap();
        assert_eq!(current(&game.file), "120");
        restore(&game.journal, Scope::Session("handheld")).unwrap();
        assert_eq!(std::fs::read_to_string(&game.file).unwrap(), OPTISCALER);
    }

    #[test]
    fn writing_again_in_one_session_keeps_the_first_original() {
        let game = game();
        let folders = folders([game.folder.clone()]);
        let at = |fps| {
            targets(
                &[limit("{stream_fps}")],
                Some(fps),
                Some(&game.folder),
                &folders,
                &mut Report::default(),
            )
        };
        apply(&game.journal, "phone", &at(60)).unwrap();
        apply(&game.journal, "phone", &at(90)).unwrap();
        let journal = Journal::load(&game.journal).unwrap();
        assert_eq!(
            (
                journal.changes[0].original.as_str(),
                journal.changes[0].written.as_str()
            ),
            ("59", "90")
        );
        // A value the user set in between becomes the one to go back to.
        std::fs::write(&game.file, OPTISCALER.replace("= 59", "= 75")).unwrap();
        apply(&game.journal, "phone", &at(120)).unwrap();
        let journal = Journal::load(&game.journal).unwrap();
        assert_eq!(journal.changes[0].original, "75");
        restore(&game.journal, Scope::All).unwrap();
        assert_eq!(current(&game.file), "75");
    }

    #[test]
    fn an_unreadable_journal_writes_nothing() {
        let game = game();
        std::fs::create_dir_all(game.journal.parent().unwrap()).unwrap();
        std::fs::write(&game.journal, "{not json").unwrap();
        let folders = folders([game.folder.clone()]);
        let targets = targets(
            &[limit("120")],
            None,
            Some(&game.folder),
            &folders,
            &mut Report::default(),
        );
        assert!(apply(&game.journal, "phone", &targets).is_err());
        assert_eq!(std::fs::read_to_string(&game.file).unwrap(), OPTISCALER);
        assert!(restore(&game.journal, Scope::All).is_err());
    }

    #[test]
    #[cfg(windows)]
    fn a_read_only_file_is_not_overwritten_and_nothing_stays_journaled() {
        let game = game();
        let mut permissions = std::fs::metadata(&game.file).unwrap().permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&game.file, permissions.clone()).unwrap();
        let folders = folders([game.folder.clone()]);
        let targets = targets(
            &[limit("120")],
            None,
            Some(&game.folder),
            &folders,
            &mut Report::default(),
        );
        let report = apply(&game.journal, "phone", &targets).unwrap();
        assert!(
            report.written.is_empty() && report.skipped.len() == 1,
            "{report:?}"
        );
        assert_eq!(current(&game.file), "59");
        assert!(Journal::load(&game.journal).unwrap().changes.is_empty());
        // No temporary file is left beside it.
        assert_eq!(std::fs::read_dir(&game.folder).unwrap().count(), 1);
        #[allow(clippy::permissions_set_readonly_false)]
        permissions.set_readonly(false);
        std::fs::set_permissions(&game.file, permissions).unwrap();
    }

    #[test]
    fn frame_limit_keys_that_disagree_with_the_limiter_are_named() {
        let target = |key: &str, value: &str| Target {
            file: PathBuf::from("x.ini"),
            section: String::new(),
            key: key.into(),
            value: value.into(),
        };
        let targets = [
            target("FramerateLimit", "59"),
            target("FPSCap", "120"),
            target("MaxFrameLimit", "0"),
            target("FrameLimit", "auto"),
            target("Upscaler", "59"),
            target("fps", "119.6"),
        ];
        let named: Vec<&str> = limit_conflicts(&targets, 120)
            .map(|t| t.key.as_str())
            .collect();
        assert_eq!(named, ["FramerateLimit"]);
    }
}
