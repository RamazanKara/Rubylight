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
fn a_numbered_copy_of_the_longest_name_is_a_stored_name() {
    let longest = format!("{}.txt", "n".repeat(MAX_NAME_UNITS - 4));
    let numbered = collision_name(&longest, u32::MAX);
    assert_eq!(numbered.len(), MAX_STORED_NAME_UNITS);
    assert_eq!(check_name(&numbered), Err(NameError::TooLong));
    assert_eq!(check_stored_name(&numbered), Ok(()));
    // With `.part` it still fits a Windows path component.
    assert!(part_name(&numbered).len() <= 255);
    assert_eq!(
        check_stored_name(&format!("{numbered}x")),
        Err(NameError::TooLong)
    );
    // The other rules hold as for uploads.
    assert_eq!(check_stored_name("a:b"), Err(NameError::StreamSeparator));
    assert_eq!(check_stored_name("x.part"), Err(NameError::PartSuffix));
}

#[test]
fn a_shared_folder_path_is_split_into_names_that_stay_in_the_folder() {
    assert_eq!(folder_path(""), Ok(vec![]));
    assert_eq!(folder_path("saves"), Ok(vec!["saves"]));
    assert_eq!(
        folder_path("saves/Slot 1/ä.sav"),
        Ok(vec!["saves", "Slot 1", "ä.sav"])
    );
    // A whole Windows path component, longer than an upload's name.
    let long = "a".repeat(MAX_FOLDER_NAME_UNITS);
    assert_eq!(folder_path(&long), Ok(vec![long.as_str()]));
    let deepest = vec!["d"; MAX_FOLDER_DEPTH].join("/");
    assert_eq!(folder_path(&deepest).map(|n| n.len()), Ok(MAX_FOLDER_DEPTH));
    for (path, error) in [
        ("..", NameError::ParentFolder),
        ("saves/../..", NameError::ParentFolder),
        (".", NameError::TrailingDotOrSpace),
        ("saves/./x", NameError::TrailingDotOrSpace),
        ("/saves", NameError::Empty),
        ("saves/", NameError::Empty),
        ("saves//x", NameError::Empty),
        ("saves\\..\\x", NameError::PathSeparator),
        ("C:/Windows", NameError::StreamSeparator),
        ("x.txt:hidden", NameError::StreamSeparator),
        ("x.txt::$DATA", NameError::StreamSeparator),
        ("saves/CON", NameError::ReservedDeviceName),
        ("nul.txt", NameError::ReservedDeviceName),
        ("Com1 .txt/x", NameError::ReservedDeviceName),
        ("conin$", NameError::ReservedDeviceName),
        ("x.mp4.PART", NameError::PartSuffix),
        ("x ", NameError::TrailingDotOrSpace),
        ("x\u{202e}gpj.exe", NameError::FormatCharacter),
        ("a\u{0}b", NameError::ControlCharacter),
        ("a*b", NameError::InvalidCharacter),
    ] {
        assert_eq!(folder_path(path), Err(error), "{path:?}");
    }
    assert_eq!(
        folder_path(&"a".repeat(MAX_FOLDER_NAME_UNITS + 1)),
        Err(NameError::TooLong)
    );
    let deeper = vec!["d"; MAX_FOLDER_DEPTH + 1].join("/");
    assert_eq!(folder_path(&deeper), Err(NameError::TooLong));
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
