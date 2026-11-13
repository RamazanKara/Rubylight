//! File transfer: a paired client sends files from the phone to the PC and
//! fetches them back. The rules here are pure: which names the host accepts,
//! the name a file takes when one with its name already exists, and the
//! pace that keeps an upload from competing with a running stream.
//! `butterpollo-windows` opens the files as the signed-in user.
use std::time::{Duration, Instant};

/// The longest name the host accepts, counted in UTF-16 units as Windows
/// counts them. The `" (n)"` suffix and `.part` still fit the 255-unit limit
/// of a Windows path component.
pub const MAX_NAME_UNITS: usize = 200;
/// The longest name a file in the transfer folder can have: the numbered
/// copy of the longest name the host accepts.
pub const MAX_STORED_NAME_UNITS: usize = MAX_NAME_UNITS + " (4294967295)".len();
/// An upload is written to its name plus this suffix and renamed when it
/// completes, so a half-written file never carries the final name.
pub const PART_SUFFIX: &str = ".part";

/// Why a requested file name is refused. The native API answers each with
/// `invalid_name`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NameError {
    Empty,
    TooLong,
    /// `/` or `\`: the name must stay in the transfer folder.
    PathSeparator,
    /// `..`, the parent folder.
    ParentFolder,
    ControlCharacter,
    /// An invisible formatting character, such as a bidirectional override
    /// that shows `exe.txt` for `txt.exe`, or a zero-width space.
    FormatCharacter,
    /// `:`, which would name an alternate data stream.
    StreamSeparator,
    /// `<`, `>`, `"`, `|`, `?` or `*`, which Windows refuses in a name.
    InvalidCharacter,
    /// `CON`, `NUL`, `COM1` and the like, with or without an extension.
    ReservedDeviceName,
    /// Windows strips a trailing dot or space, so the file would not keep
    /// the name it was sent with.
    TrailingDotOrSpace,
    /// Ends in `.part`, the suffix of uploads in progress.
    PartSuffix,
}
impl std::fmt::Display for NameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Empty => "the file name is empty",
            Self::TooLong => "the file name is longer than 200 characters",
            Self::PathSeparator => "the file name contains a path separator",
            Self::ParentFolder => "the file name names the parent folder",
            Self::ControlCharacter => "the file name contains a control character",
            Self::FormatCharacter => "the file name contains an invisible formatting character",
            Self::StreamSeparator => "the file name contains a colon",
            Self::InvalidCharacter => "the file name contains a character Windows refuses",
            Self::ReservedDeviceName => "the file name is a reserved Windows device name",
            Self::TrailingDotOrSpace => "the file name ends with a dot or a space",
            Self::PartSuffix => "the file name ends with .part",
        })
    }
}
impl std::error::Error for NameError {}

const RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$", "COM0", "COM1", "COM2", "COM3", "COM4",
    "COM5", "COM6", "COM7", "COM8", "COM9", "COM¹", "COM²", "COM³", "LPT0", "LPT1", "LPT2", "LPT3",
    "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9", "LPT¹", "LPT²", "LPT³",
];

/// Invisible formatting characters (Unicode category Cf) that can make a
/// name look like another: bidirectional marks, embeddings, overrides and
/// isolates, zero-width spaces and joiners of words, the byte order mark.
/// The zero-width joiner and non-joiner stay allowed: emoji sequences and
/// Persian and Indic words need them.
fn format_character(c: char) -> bool {
    matches!(
        c,
        '\u{ad}'
            | '\u{61c}'
            | '\u{180e}'
            | '\u{200b}'
            | '\u{200e}'
            | '\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{206f}'
            | '\u{feff}'
            | '\u{fff9}'..='\u{fffb}'
    )
}

/// Checks a client's file name, which must name a file directly in the
/// transfer folder. Names are refused, never rewritten, so the client
/// learns that its file would not have kept its name.
pub fn check_name(name: &str) -> Result<(), NameError> {
    check(name, MAX_NAME_UNITS)
}

/// Checks the name of a file in the transfer folder, listed or asked for:
/// the rules of [`check_name`], but long enough for the numbered copy of
/// the longest name.
pub fn check_stored_name(name: &str) -> Result<(), NameError> {
    check(name, MAX_STORED_NAME_UNITS)
}

/// The longest name of a file or folder in a shared folder: a whole Windows
/// path component.
pub const MAX_FOLDER_NAME_UNITS: usize = 255;
/// How many folders deep a device may ask for in a shared folder.
pub const MAX_FOLDER_DEPTH: usize = 64;

/// Checks the name of a file or folder in a shared folder, listed or asked
/// for: the rules of [`check_name`], up to [`MAX_FOLDER_NAME_UNITS`].
pub fn check_folder_name(name: &str) -> Result<(), NameError> {
    check(name, MAX_FOLDER_NAME_UNITS)
}

/// Splits a device's path in a shared folder (`saves/slot1.sav`: names
/// between `/`, empty for the folder itself) into its names, each checked
/// with [`check_folder_name`]. Nothing is resolved or rewritten: an empty
/// name (`a//b`, a leading or trailing `/`), `.`, `..`, a stream name, a
/// device name and more than [`MAX_FOLDER_DEPTH`] names are refused, and
/// `\` is refused inside a name.
pub fn folder_path(path: &str) -> Result<Vec<&str>, NameError> {
    if path.is_empty() {
        return Ok(Vec::new());
    }
    let names: Vec<&str> = path.split('/').collect();
    if names.len() > MAX_FOLDER_DEPTH {
        return Err(NameError::TooLong);
    }
    for name in &names {
        check_folder_name(name)?;
    }
    Ok(names)
}

fn check(name: &str, max_units: usize) -> Result<(), NameError> {
    if name.is_empty() {
        return Err(NameError::Empty);
    }
    if name.encode_utf16().count() > max_units {
        return Err(NameError::TooLong);
    }
    if name.contains(['/', '\\']) {
        return Err(NameError::PathSeparator);
    }
    if name == ".." {
        return Err(NameError::ParentFolder);
    }
    if name.chars().any(char::is_control) {
        return Err(NameError::ControlCharacter);
    }
    if name.chars().any(format_character) {
        return Err(NameError::FormatCharacter);
    }
    if name.contains(':') {
        return Err(NameError::StreamSeparator);
    }
    if name.contains(['<', '>', '"', '|', '?', '*']) {
        return Err(NameError::InvalidCharacter);
    }
    if name.ends_with(['.', ' ']) {
        return Err(NameError::TrailingDotOrSpace);
    }
    // Windows reads `CON.txt` and `CON .txt` as the device too.
    let stem = name.split('.').next().unwrap_or(name).trim_end_matches(' ');
    if RESERVED
        .iter()
        .any(|device| stem.eq_ignore_ascii_case(device))
    {
        return Err(NameError::ReservedDeviceName);
    }
    if has_part_suffix(name) {
        return Err(NameError::PartSuffix);
    }
    Ok(())
}

/// Whether `name` ends in [`PART_SUFFIX`], in any case.
pub fn has_part_suffix(name: &str) -> bool {
    name.get(name.len().saturating_sub(PART_SUFFIX.len())..)
        .is_some_and(|end| end.eq_ignore_ascii_case(PART_SUFFIX))
}

/// The name an upload takes on its `n`th collision: `name (n).ext`, the
/// way Windows Explorer and browsers number copies. `n == 0` is the name
/// itself. A leading dot starts a name, not an extension (`.config (1)`).
pub fn collision_name(name: &str, n: u32) -> String {
    if n == 0 {
        return name.to_owned();
    }
    match name.rfind('.') {
        Some(dot) if dot > 0 => format!("{} ({n}){}", &name[..dot], &name[dot..]),
        _ => format!("{name} ({n})"),
    }
}

/// The temporary name an upload is written to.
pub fn part_name(name: &str) -> String {
    format!("{name}{PART_SUFFIX}")
}

/// A token bucket that caps upload writes while a stream runs, so a large
/// file doesn't take the link or the disk from the stream. The caller
/// passes the clock; without a rate every write goes through at once.
#[derive(Debug)]
pub struct Pacer {
    rate: Option<u64>,
    burst: u64,
    /// Allowance in byte-nanoseconds per second (bytes × 10⁹), so refills
    /// and waits stay exact integers. Negative while writes are in debt.
    allowance: i128,
    last: Instant,
}
const NANOS: i128 = 1_000_000_000;
impl Pacer {
    /// `rate` in bytes per second, `None` for unpaced. `burst` bytes may be
    /// written at once after an idle spell; the bucket starts full.
    pub fn new(rate: Option<u64>, burst: u64, now: Instant) -> Self {
        Self {
            rate: rate.filter(|rate| *rate > 0),
            burst,
            allowance: i128::from(burst) * NANOS,
            last: now,
        }
    }

    /// Changes the rate, as a stream starts or ends. Time already passed
    /// refills at the old rate. Going from unpaced to paced starts with a
    /// full bucket.
    pub fn set_rate(&mut self, rate: Option<u64>, now: Instant) {
        let rate = rate.filter(|rate| *rate > 0);
        if self.rate.is_none() {
            self.allowance = i128::from(self.burst) * NANOS;
        } else {
            self.refill(now);
        }
        self.rate = rate;
        self.last = now;
    }

    pub fn rate(&self) -> Option<u64> {
        self.rate
    }

    /// Takes `bytes` from the bucket and returns how long the caller waits
    /// before writing them: zero while the allowance covers them.
    pub fn take(&mut self, bytes: u64, now: Instant) -> Duration {
        let Some(rate) = self.rate else {
            self.last = now;
            return Duration::ZERO;
        };
        self.refill(now);
        self.allowance -= i128::from(bytes) * NANOS;
        if self.allowance >= 0 {
            return Duration::ZERO;
        }
        // Whole nanoseconds, rounded up, until the debt is paid back.
        let rate = i128::from(rate);
        let wait = (-self.allowance + rate - 1) / rate;
        Duration::from_nanos(u64::try_from(wait).unwrap_or(u64::MAX))
    }

    fn refill(&mut self, now: Instant) {
        let Some(rate) = self.rate else { return };
        let elapsed = now.saturating_duration_since(self.last).as_nanos();
        let elapsed = i128::try_from(elapsed).unwrap_or(i128::MAX / 2);
        let cap = i128::from(self.burst) * NANOS;
        self.allowance = self
            .allowance
            .saturating_add(elapsed.saturating_mul(i128::from(rate)))
            .min(cap);
        self.last = self.last.max(now);
    }
}

#[cfg(test)]
mod tests;
