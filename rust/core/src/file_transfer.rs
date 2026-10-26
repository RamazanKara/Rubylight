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
    if name.is_empty() {
        return Err(NameError::Empty);
    }
    if name.encode_utf16().count() > MAX_NAME_UNITS {
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
mod tests {
    use super::*;

    #[test]
    fn names_that_leave_the_folder_or_change_on_windows_are_refused() {
        let long = "a".repeat(201);
        let cases: &[(&str, NameError)] = &[
            ("", NameError::Empty),
            (&long, NameError::TooLong),
            ("..", NameError::ParentFolder),
            ("a/b", NameError::PathSeparator),
            ("a\\b", NameError::PathSeparator),
            ("../x", NameError::PathSeparator),
            ("..\\x", NameError::PathSeparator),
            ("/etc", NameError::PathSeparator),
            ("x:y", NameError::StreamSeparator),
            ("C:", NameError::StreamSeparator),
            ("file.txt:stream", NameError::StreamSeparator),
            ("a\0b", NameError::ControlCharacter),
            ("a\nb", NameError::ControlCharacter),
            ("a\tb", NameError::ControlCharacter),
            ("a\u{7f}b", NameError::ControlCharacter),
            ("a\u{85}b", NameError::ControlCharacter),
            ("a?b", NameError::InvalidCharacter),
            ("a*", NameError::InvalidCharacter),
            ("a<b>", NameError::InvalidCharacter),
            ("a|b", NameError::InvalidCharacter),
            ("\"a\"", NameError::InvalidCharacter),
            ("CON", NameError::ReservedDeviceName),
            ("con", NameError::ReservedDeviceName),
            ("CON.txt", NameError::ReservedDeviceName),
            ("con .txt", NameError::ReservedDeviceName),
            ("nul.tar.gz", NameError::ReservedDeviceName),
            ("Prn", NameError::ReservedDeviceName),
            ("aux.log", NameError::ReservedDeviceName),
            ("COM1", NameError::ReservedDeviceName),
            ("com9.bin", NameError::ReservedDeviceName),
            ("LPT1.txt", NameError::ReservedDeviceName),
            ("lpt9", NameError::ReservedDeviceName),
            ("COM¹.txt", NameError::ReservedDeviceName),
            ("CONIN$", NameError::ReservedDeviceName),
            ("a.", NameError::TrailingDotOrSpace),
            ("a ", NameError::TrailingDotOrSpace),
            (".", NameError::TrailingDotOrSpace),
            ("...", NameError::TrailingDotOrSpace),
            ("photo.jpg.", NameError::TrailingDotOrSpace),
            // Shows as `invoice_exe.pdf` but ends in `.exe`.
            ("invoice_\u{202e}fdp.exe", NameError::FormatCharacter),
            ("a\u{202a}b", NameError::FormatCharacter),
            ("a\u{2066}b\u{2069}", NameError::FormatCharacter),
            ("a\u{200e}.txt", NameError::FormatCharacter),
            ("a\u{200f}.txt", NameError::FormatCharacter),
            ("a\u{61c}.txt", NameError::FormatCharacter),
            ("photo\u{200b}.jpg", NameError::FormatCharacter),
            ("photo\u{2060}.jpg", NameError::FormatCharacter),
            ("\u{feff}photo.jpg", NameError::FormatCharacter),
            ("pho\u{ad}to.jpg", NameError::FormatCharacter),
            ("x.part", NameError::PartSuffix),
            ("x.PART", NameError::PartSuffix),
            ("photo.jpg.Part", NameError::PartSuffix),
            (".part", NameError::PartSuffix),
            ("日本.part", NameError::PartSuffix),
        ];
        for (name, error) in cases {
            assert_eq!(check_name(name), Err(*error), "{name:?}");
        }
    }

    #[test]
    fn ordinary_and_unicode_names_are_accepted() {
        let longest = "a".repeat(200);
        for name in [
            "photo.jpg",
            "IMG_2026-10-26 12.00.00.HEIC",
            &longest,
            ".config",
            "a..b",
            "..a",
            "console.txt",
            "CONFIG",
            "COM10",
            "LPT",
            "nul_report.pdf",
            "Größe ändern.png",
            "日本語のファイル.txt",
            "ملف.pdf",
            "emoji 🎮.zip",
            " leading space.txt",
            "tab-free name (1).txt",
            // Joiners stay: a family emoji and a Persian word use them.
            "family 👨\u{200d}👩\u{200d}👧.png",
            "می\u{200c}خواهم.txt",
            "x.parts",
            "partial.txt",
            "part",
            "x.par",
        ] {
            assert_eq!(check_name(name), Ok(()), "{name:?}");
        }
    }

    #[test]
    fn the_length_limit_counts_utf16_units() {
        // 100 astral characters are 200 UTF-16 units, 101 are 202.
        assert_eq!(check_name(&"🎮".repeat(100)), Ok(()));
        assert_eq!(check_name(&"🎮".repeat(101)), Err(NameError::TooLong));
        // 200 two-byte UTF-8 characters are 200 units.
        assert_eq!(check_name(&"ä".repeat(200)), Ok(()));
        assert_eq!(check_name(&"ä".repeat(201)), Err(NameError::TooLong));
    }

    #[test]
    fn collisions_are_numbered_before_the_extension() {
        assert_eq!(collision_name("photo.jpg", 0), "photo.jpg");
        assert_eq!(collision_name("photo.jpg", 1), "photo (1).jpg");
        assert_eq!(collision_name("photo.jpg", 12), "photo (12).jpg");
        assert_eq!(collision_name("archive.tar.gz", 2), "archive.tar (2).gz");
        assert_eq!(collision_name("README", 3), "README (3)");
        assert_eq!(collision_name(".config", 1), ".config (1)");
        assert_eq!(collision_name("..a", 1), ". (1).a");
        assert_eq!(collision_name("日本.txt", 1), "日本 (1).txt");
        assert_eq!(part_name("photo (1).jpg"), "photo (1).jpg.part");
        // A numbered name of the longest accepted name is still a valid
        // name for Windows, with room for `.part`.
        let longest = format!("{}.bin", "a".repeat(196));
        let numbered = part_name(&collision_name(&longest, u32::MAX));
        assert!(numbered.encode_utf16().count() <= 255, "{numbered}");
        for n in 1..3 {
            assert_eq!(check_name(&collision_name("photo.jpg", n)), Ok(()));
        }
    }

    #[test]
    fn an_unpaced_pacer_never_waits() {
        let start = Instant::now();
        let mut pacer = Pacer::new(None, 1000, start);
        assert_eq!(pacer.take(u64::MAX / 2, start), Duration::ZERO);
        assert_eq!(pacer.take(10_000_000, start), Duration::ZERO);
        let mut zero = Pacer::new(Some(0), 1000, start);
        assert_eq!(zero.rate(), None);
        assert_eq!(zero.take(10_000_000, start), Duration::ZERO);
    }

    #[test]
    fn the_pacer_spends_its_burst_then_holds_the_rate() {
        let start = Instant::now();
        // 1 MB/s with a 100 kB burst.
        let mut pacer = Pacer::new(Some(1_000_000), 100_000, start);
        assert_eq!(pacer.take(100_000, start), Duration::ZERO);
        // The next 50 kB wait 50 ms.
        assert_eq!(pacer.take(50_000, start), Duration::from_millis(50));
        // The writer slept those 50 ms; the debt is paid, the bucket empty.
        let t = start + Duration::from_millis(50);
        assert_eq!(pacer.take(10_000, t), Duration::from_millis(10));
        // Writing 64 kB chunks as fast as allowed averages the rate.
        let mut now = t + Duration::from_millis(10);
        let mut written = 0u64;
        let begin = now;
        for _ in 0..100 {
            now += pacer.take(64 * 1024, now);
            written += 64 * 1024;
        }
        // The last chunk's wait ends when its bytes are paid for.
        let elapsed = now - begin;
        assert_eq!(elapsed, Duration::from_nanos(written * 1000));
    }

    #[test]
    fn an_idle_pacer_refills_only_up_to_its_burst() {
        let start = Instant::now();
        let mut pacer = Pacer::new(Some(1_000_000), 100_000, start);
        assert_eq!(pacer.take(100_000, start), Duration::ZERO);
        let later = start + Duration::from_secs(60);
        assert_eq!(pacer.take(100_000, later), Duration::ZERO);
        assert_eq!(pacer.take(1, later), Duration::from_nanos(1000));
        // Waits round up to whole nanoseconds.
        let mut odd = Pacer::new(Some(3), 0, start);
        assert_eq!(odd.take(1, start), Duration::from_nanos(333_333_334));
        // A clock that steps back costs nothing and gains nothing.
        let mut back = Pacer::new(Some(1_000), 0, later);
        assert_eq!(back.take(1, start), Duration::from_millis(1));
        assert_eq!(back.take(1, later), Duration::from_millis(2));
    }

    #[test]
    fn a_stream_starting_or_ending_changes_the_pace() {
        let start = Instant::now();
        let mut pacer = Pacer::new(None, 100_000, start);
        assert_eq!(pacer.take(5_000_000, start), Duration::ZERO);
        // A stream starts: the burst is available, then the rate holds.
        pacer.set_rate(Some(1_000_000), start);
        assert_eq!(pacer.rate(), Some(1_000_000));
        assert_eq!(pacer.take(100_000, start), Duration::ZERO);
        assert_eq!(pacer.take(100_000, start), Duration::from_millis(100));
        // Halfway through that wait the rate doubles: the remaining 50 kB
        // of debt clears at the new rate.
        let half = start + Duration::from_millis(50);
        pacer.set_rate(Some(2_000_000), half);
        assert_eq!(pacer.take(0, half), Duration::from_millis(25));
        // The stream ends: writes go straight through again.
        pacer.set_rate(None, half);
        assert_eq!(pacer.take(10_000_000, half), Duration::ZERO);
        // And the next stream starts with a full bucket, not the old debt.
        pacer.set_rate(Some(1_000_000), half);
        assert_eq!(pacer.take(100_000, half), Duration::ZERO);
    }
}
