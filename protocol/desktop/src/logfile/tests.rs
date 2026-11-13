use super::*;
use std::sync::mpsc::sync_channel;
use std::time::Duration;

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("nightjar-log-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn feed(chunks: &[&[u8]]) -> String {
    let mut stamper = Stamper::default();
    let mut out = Vec::new();
    for chunk in chunks {
        stamper.push(chunk, "T ", &mut out);
    }
    String::from_utf8(out).unwrap()
}

#[test]
fn timestamp_is_utc_with_milliseconds() {
    assert_eq!(timestamp(UNIX_EPOCH), "1970-01-01T00:00:00.000Z ");
    let at = UNIX_EPOCH + Duration::new(1_792_936_987, 123_000_000);
    assert_eq!(timestamp(at), "2026-10-25T14:03:07.123Z ");
    assert_eq!(timestamp(UNIX_EPOCH + Duration::from_secs(951_868_800)), "2000-03-01T00:00:00.000Z ");
    // Leap day and the end of a year.
    assert_eq!(timestamp(UNIX_EPOCH + Duration::from_secs(1_709_164_800 + 86_399)), "2024-02-29T23:59:59.000Z ");
    assert_eq!(timestamp(UNIX_EPOCH + Duration::from_secs(1_767_225_599)), "2025-12-31T23:59:59.000Z ");
}

#[test]
fn every_line_gets_one_stamp() {
    assert_eq!(feed(&[b"one\ntwo\n"]), "T one\nT two\n");
}

#[test]
fn a_line_split_over_chunks_is_stamped_once() {
    assert_eq!(feed(&[b"nightj", b"ar: hel", b"lo\nnext"]), "T nightjar: hello\nT next");
    assert_eq!(feed(&[b"a\n", b"", b"b"]), "T a\nT b");
}

#[test]
fn crlf_is_one_line_end_even_across_chunks() {
    assert_eq!(feed(&[b"a\r\nb\r\n"]), "T a\nT b\n");
    assert_eq!(feed(&[b"a\r", b"\nb"]), "T a\nT b");
    // A lone \r inside a line stays.
    assert_eq!(feed(&[b"a\rb\n"]), "T a\rb\n");
    assert_eq!(feed(&[b"a\r", b"b\n"]), "T a\rb\n");
}

#[test]
fn empty_lines_are_not_stamped() {
    assert_eq!(feed(&[b"a\n\n\r\nb\n"]), "T a\n\n\nT b\n");
}

#[test]
fn utf8_split_over_chunks_is_decoded_whole() {
    let bytes = "Größe ä€😀\n".as_bytes();
    // Every split point, including inside the 2, 3 and 4 byte sequences.
    for cut in 0..=bytes.len() {
        let mut carry = Utf8Carry::default();
        let text = carry.push(&bytes[..cut]) + &carry.push(&bytes[cut..]);
        assert_eq!(text, "Größe ä€😀\n", "cut at {cut}");
    }
    // Bad bytes do not stop the decoding or get stuck in the carry.
    let mut carry = Utf8Carry::default();
    assert_eq!(carry.push(b"a\xffb\xc3"), "a\u{FFFD}b");
    assert_eq!(carry.push(b"\xa4c"), "äc");
    assert_eq!(carry.push(b"\xe2\x82"), "");
    assert_eq!(carry.push(b"x"), "\u{FFFD}x");
}

#[test]
fn rotation_moves_current_to_prev_and_replaces_the_old_prev() {
    let dir = temp("rotate");
    assert!(rotate(&dir).unwrap().warning.is_none());
    assert!(dir.join(CURRENT).exists() && !dir.join(PREV).exists());
    std::fs::write(dir.join(CURRENT), "first run").unwrap();
    rotate(&dir).unwrap();
    assert_eq!(std::fs::read_to_string(dir.join(PREV)).unwrap(), "first run");
    assert_eq!(std::fs::read_to_string(dir.join(CURRENT)).unwrap(), "");
    std::fs::write(dir.join(CURRENT), "second run").unwrap();
    rotate(&dir).unwrap();
    assert_eq!(std::fs::read_to_string(dir.join(PREV)).unwrap(), "second run");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn rotation_creates_the_folder() {
    let dir = temp("create").join("deeper").join("logs");
    rotate(&dir).unwrap();
    assert!(dir.join(CURRENT).exists());
    let _ = std::fs::remove_dir_all(dir.parent().unwrap().parent().unwrap());
}

#[test]
fn a_prev_file_that_cannot_be_replaced_does_not_stop_the_log() {
    let dir = temp("stuck");
    // A folder where the prev file goes: removing it as a file fails, as a locked file does.
    std::fs::create_dir_all(dir.join(PREV)).unwrap();
    std::fs::write(dir.join(CURRENT), "old run, long enough to see it truncated").unwrap();
    let mut log = LogFile::open(&dir, CAP).unwrap();
    let warnings = log.take_warnings();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("overwritten"));
    log.write_chunk(b"new run\n", UNIX_EPOCH).unwrap();
    log.flush().unwrap();
    assert_eq!(std::fs::read_to_string(dir.join(CURRENT)).unwrap(), "1970-01-01T00:00:00.000Z new run\n");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn open_keeps_the_last_two_runs() {
    let dir = temp("open");
    for run in ["a", "b", "c"] {
        let mut log = LogFile::open(&dir, CAP).unwrap();
        log.write_chunk(format!("run {run}\n").as_bytes(), UNIX_EPOCH).unwrap();
        log.flush().unwrap();
    }
    assert_eq!(std::fs::read_to_string(dir.join(CURRENT)).unwrap(), "1970-01-01T00:00:00.000Z run c\n");
    assert_eq!(std::fs::read_to_string(dir.join(PREV)).unwrap(), "1970-01-01T00:00:00.000Z run b\n");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_full_file_is_rotated_and_stays_under_the_cap() {
    let dir = temp("cap");
    let cap = 600;
    let mut log = LogFile::open(&dir, cap).unwrap();
    for i in 0..50 {
        log.write_chunk(format!("line number {i:03}\n").as_bytes(), UNIX_EPOCH).unwrap();
    }
    log.flush().unwrap();
    let current = std::fs::read_to_string(dir.join(CURRENT)).unwrap();
    let prev = std::fs::read_to_string(dir.join(PREV)).unwrap();
    assert!(current.len() as u64 <= cap && prev.len() as u64 <= cap, "{} {}", current.len(), prev.len());
    // The newest line is kept, the new file says where the rest went, and lines are whole.
    assert!(current.contains("line number 049\n"));
    assert!(current.lines().next().unwrap().contains("log continued"));
    assert!(current.lines().chain(prev.lines()).all(|l| l.starts_with("1970-01-01T00:00:00.000Z ")));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_failed_rotation_in_a_run_keeps_appending_and_says_so_once_per_try() {
    let dir = temp("cap-stuck");
    let mut log = LogFile::open(&dir, 300).unwrap();
    std::fs::create_dir_all(dir.join(PREV)).unwrap();
    for i in 0..40 {
        log.write_chunk(format!("line number {i:03}\n").as_bytes(), UNIX_EPOCH).unwrap();
    }
    log.flush().unwrap();
    let text = std::fs::read_to_string(dir.join(CURRENT)).unwrap();
    assert_eq!(text.lines().count(), 40, "nothing is lost");
    // About 1500 bytes in 300-byte steps: a handful of tries, not one per line.
    let warnings = log.take_warnings();
    assert!((2..=8).contains(&warnings.len()), "{warnings:?}");
    assert!(warnings[0].contains("keeps growing"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_chunk_cut_mid_line_over_the_cap_starts_the_new_file_with_a_stamp() {
    let dir = temp("cap-mid");
    let mut log = LogFile::open(&dir, 200).unwrap();
    log.write_chunk(b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n", UNIX_EPOCH).unwrap();
    log.write_chunk(b"part one of a line ", UNIX_EPOCH).unwrap();
    log.write_chunk(b"and its end, long enough to push the file over the cap\n", UNIX_EPOCH).unwrap();
    log.flush().unwrap();
    let current = std::fs::read_to_string(dir.join(CURRENT)).unwrap();
    assert!(current.lines().all(|l| l.starts_with("1970-01-01T00:00:00.000Z ")), "{current}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_logs_folder_follows_the_config_folder() {
    // The only test that sets the variable.
    let dir = temp("env");
    std::env::set_var("NIGHTJAR_CONFIG_DIR", &dir);
    let found = logs_dir().unwrap();
    std::env::remove_var("NIGHTJAR_CONFIG_DIR");
    assert_eq!(found, dir.join("logs"));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Feed `reads` through a reader into a writer thread and return what the log file holds.
fn through_pumps(reads: Vec<Vec<u8>>, name: &str) -> (Vec<u8>, String) {
    let dir = temp(name);
    let log = LogFile::open(&dir, CAP).unwrap();
    let backlog = std::sync::Arc::new(Backlog::new(BACKLOG_BYTES));
    let (files, chunks) = sync_channel(8);
    let writer = {
        let backlog = backlog.clone();
        std::thread::spawn(move || pump_writer(chunks, &backlog, log, |_| {}))
    };
    let mut reads = reads.into_iter();
    let mut forwarded = Vec::new();
    pump_reader(
        |buffer| match reads.next() {
            Some(read) => {
                buffer[..read.len()].copy_from_slice(&read);
                read.len()
            }
            None => 0,
        },
        |bytes| forwarded.extend_from_slice(bytes),
        files,
        &backlog,
    );
    writer.join().unwrap();
    let text = std::fs::read_to_string(dir.join(CURRENT)).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    (forwarded, text)
}

#[test]
fn the_pump_forwards_raw_bytes_and_logs_stamped_lines() {
    let (forwarded, text) = through_pumps(vec![b"hello wor".to_vec(), b"ld\r\nbye\n".to_vec()], "pump");
    assert_eq!(forwarded, b"hello world\r\nbye\n");
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 2);
    assert!(lines[0].ends_with("Z hello world") && lines[1].ends_with("Z bye"));
}

#[test]
fn a_burst_of_short_lines_on_a_healthy_disk_loses_nothing() {
    // One short line per read, as eprintln! on many threads produces, far faster than a
    // disk flush per read could keep up with.
    let reads: Vec<Vec<u8>> = (0..20_000).map(|i| format!("nightjar: stream timing line {i:05}\n").into_bytes()).collect();
    let (forwarded, text) = through_pumps(reads, "burst");
    assert_eq!(forwarded.iter().filter(|&&b| b == b'\n').count(), 20_000);
    assert!(!text.contains("dropped"), "nothing may be dropped");
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 20_000);
    for (i, line) in lines.iter().enumerate() {
        assert!(line.ends_with(&format!("Z nightjar: stream timing line {i:05}")), "{line}");
    }
}

#[test]
fn a_reader_holds_back_what_the_writer_has_not_taken_and_sends_it_whole() {
    // Room for one chunk. The writer takes nothing until the third read: the second read is
    // held back, forwarded all the same, and goes out with the third. Nothing is lost.
    let backlog = Backlog::new(BACKLOG_BYTES);
    let (files, chunks) = sync_channel(1);
    let script: [&[u8]; 3] = [b"first\n", b"second\n", b"third\n"];
    let mut reads = 0;
    let mut forwarded = Vec::new();
    let mut taken: Vec<Chunk> = Vec::new();
    pump_reader(
        |buffer| {
            if reads == 2 {
                taken.extend(chunks.try_iter());
            }
            let Some(chunk) = script.get(reads) else { return 0 };
            reads += 1;
            buffer[..chunk.len()].copy_from_slice(chunk);
            chunk.len()
        },
        |bytes| forwarded.extend_from_slice(bytes),
        files,
        &backlog,
    );
    assert_eq!(forwarded, b"first\nsecond\nthird\n");
    taken.extend(chunks.try_iter());
    let seen: Vec<(&[u8], usize)> = taken.iter().map(|c| (&c.bytes[..], c.lost)).collect();
    assert_eq!(seen, vec![(&b"first\n"[..], 0), (&b"second\nthird\n"[..], 0)]);
}

#[test]
fn a_reader_drops_only_above_the_byte_limit_and_counts_it() {
    // The writer is stalled until the third read; the backlog holds 10 bytes.
    let backlog = Backlog::new(10);
    let (files, chunks) = sync_channel(8);
    let script: [&[u8]; 3] = [b"first\n", b"second\n", b"third\n"];
    let mut reads = 0;
    let mut forwarded = Vec::new();
    let mut taken: Vec<Chunk> = Vec::new();
    pump_reader(
        |buffer| {
            if reads == 2 {
                for chunk in chunks.try_iter() {
                    backlog.taken(chunk.bytes.len());
                    taken.push(chunk);
                }
            }
            let Some(chunk) = script.get(reads) else { return 0 };
            reads += 1;
            buffer[..chunk.len()].copy_from_slice(chunk);
            chunk.len()
        },
        |bytes| forwarded.extend_from_slice(bytes),
        files,
        &backlog,
    );
    assert_eq!(forwarded, b"first\nsecond\nthird\n", "forwarding never stops");
    taken.extend(chunks.try_iter());
    let seen: Vec<(&[u8], usize)> = taken.iter().map(|c| (&c.bytes[..], c.lost)).collect();
    assert_eq!(seen, vec![(&b"first\n"[..], 0), (&b"third\n"[..], b"second\n".len())]);
}

fn write_through_writer(chunks: Vec<Chunk>, name: &str) -> String {
    let dir = temp(name);
    let log = LogFile::open(&dir, CAP).unwrap();
    let backlog = Backlog::new(BACKLOG_BYTES);
    let (files, receiver) = sync_channel(chunks.len().max(1));
    for chunk in chunks {
        files.send(chunk).unwrap();
    }
    drop(files);
    pump_writer(receiver, &backlog, log, |_| {});
    let text = std::fs::read_to_string(dir.join(CURRENT)).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    text
}

#[test]
fn the_chunk_after_a_drop_carries_the_count_and_the_log_says_so() {
    let at = UNIX_EPOCH;
    let text = write_through_writer(
        vec![Chunk { at, bytes: b"before\n".to_vec(), lost: 0 }, Chunk { at, bytes: b"after\n".to_vec(), lost: 13 }],
        "pump-lost",
    );
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 3, "{text}");
    assert!(lines[0].ends_with("Z before"));
    assert!(lines[1].ends_with("Z 13 bytes of log output were dropped, the disk was too slow"));
    assert!(lines[2].ends_with("Z after"));
}

#[test]
fn the_drop_note_waits_for_the_end_of_the_line() {
    let at = UNIX_EPOCH;
    let text = write_through_writer(
        vec![
            Chunk { at, bytes: b"half a li".to_vec(), lost: 0 },
            Chunk { at, bytes: b"ne\nnext\n".to_vec(), lost: 5 },
            // Lost bytes before a chunk with no line end in it: the note waits for one.
            Chunk { at, bytes: b"x".to_vec(), lost: 0 },
            Chunk { at, bytes: b"y".to_vec(), lost: 3 },
            Chunk { at, bytes: b"z\nlast".to_vec(), lost: 0 },
        ],
        "pump-lost-line",
    );
    let lines: Vec<&str> = text.lines().map(|l| l.trim_start_matches("1970-01-01T00:00:00.000Z ")).collect();
    assert_eq!(
        lines,
        vec![
            "half a line",
            "5 bytes of log output were dropped, the disk was too slow",
            "next",
            "xyz",
            "3 bytes of log output were dropped, the disk was too slow",
            "last"
        ],
        "{text}"
    );
}

#[test]
fn a_drop_at_the_very_end_is_still_reported() {
    let text = write_through_writer(vec![Chunk { at: UNIX_EPOCH, bytes: b"cut".to_vec(), lost: 0 }, Chunk { at: UNIX_EPOCH, bytes: Vec::new(), lost: 9 }], "pump-lost-end");
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 2, "{text}");
    assert!(lines[1].ends_with("Z 9 bytes of log output were dropped, the disk was too slow"));
}

#[test]
fn lines_are_redacted_whole_before_they_reach_the_file() {
    nightjar_core::report::redact::add_secret("Logfile test PC");
    let dir = temp("redact");
    let mut log = LogFile::open(&dir, CAP).unwrap();
    // A PC name and an address cut by chunk ends, a PIN, and a line still open at the end.
    log.write_chunk(b"nightjar: pairing with Logfile te", UNIX_EPOCH).unwrap();
    log.write_chunk(b"st PC at 192.168.", UNIX_EPOCH).unwrap();
    log.write_chunk(b"1.20, PIN 4711\r\nnightjar: stream ", UNIX_EPOCH).unwrap();
    log.write_chunk(b"to fd01::5 ends", UNIX_EPOCH).unwrap();
    log.finish(UNIX_EPOCH).unwrap();
    log.flush().unwrap();
    // Other tests save a PC named "PC": it does not mark the marks again.
    assert_eq!(
        std::fs::read_to_string(dir.join(CURRENT)).unwrap(),
        "1970-01-01T00:00:00.000Z nightjar: pairing with [PC] at [address], PIN [omitted]\n\
             1970-01-01T00:00:00.000Z nightjar: stream to [address] ends"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// Regression: "H.264 4:4:4 is not offered" was logged as "H.264 [address] is not offered".
#[test]
fn chroma_formats_stay_and_ipv6_addresses_go() {
    let mut out = Vec::new();
    Redactor::default().push(b"H.264 4:4:4 is not offered: HEVC 4:2:0 via 2001:db8:0:0:1:2:3:4 or fe80::1\n", &mut out);
    assert_eq!(String::from_utf8(out).unwrap(), "H.264 4:4:4 is not offered: HEVC 4:2:0 via [address] or [address]\n");
}

#[test]
fn a_line_longer_than_the_limit_is_written_in_parts() {
    let mut redactor = Redactor::default();
    let mut out = Vec::new();
    let mut line = vec![b'z'; OPEN_LINE_LIMIT - 6];
    line.extend_from_slice(b" 192.1");
    redactor.push(&line, &mut out);
    assert!(out.is_empty() && redactor.holds_text());
    // Cut after the last space before the limit: the address stays whole.
    redactor.push(b"68.1.20 ok", &mut out);
    assert_eq!(out, b"[data] ");
    redactor.push(b"\n", &mut out);
    assert_eq!(out, b"[data] [address] ok\n");
    assert!(!redactor.holds_text());

    // Without a space the cut falls between two characters.
    let mut out = Vec::new();
    let text = format!("q{}", "é".repeat(OPEN_LINE_LIMIT / 2));
    redactor.push(text.as_bytes(), &mut out);
    assert!(!out.is_empty() && redactor.holds_text());
    redactor.push(b"\n", &mut out);
    assert_eq!(String::from_utf8(out).unwrap(), format!("{text}\n"));
}

#[test]
fn a_cut_never_parts_a_keyword_from_its_secret_or_splits_a_pc_name() {
    nightjar_core::report::redact::add_secret("Logfile cut PC");
    let mut redactor = Redactor::default();
    // The last space before the limit is right after "PIN": the cut goes before the keyword.
    let mut out = Vec::new();
    let mut line = vec![b'z'; OPEN_LINE_LIMIT - 6];
    line.extend_from_slice(b" PIN 4");
    redactor.push(&line, &mut out);
    redactor.push(b"711 ok", &mut out);
    assert_eq!(out, b"[data] ");
    redactor.push(b"\n", &mut out);
    assert_eq!(out, b"[data] PIN [omitted] ok\n");

    // The last space before the limit is inside a PC's name: the cut goes before the name.
    let mut out = Vec::new();
    let mut line = vec![b'z'; OPEN_LINE_LIMIT - 12];
    line.extend_from_slice(b" Logfile cu");
    redactor.push(&line, &mut out);
    redactor.push(b"t PC here\n", &mut out);
    assert_eq!(out, b"[data] [PC] here\n");
}

#[test]
fn a_log_note_starts_a_line_of_its_own() {
    let dir = temp("note");
    let mut log = LogFile::open(&dir, CAP).unwrap();
    log.write_chunk(b"half a li", UNIX_EPOCH).unwrap();
    log.note("noted", UNIX_EPOCH).unwrap();
    log.flush().unwrap();
    assert_eq!(std::fs::read_to_string(dir.join(CURRENT)).unwrap(), "1970-01-01T00:00:00.000Z half a li\n1970-01-01T00:00:00.000Z noted\n");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_problem_report_holds_both_logs_scrubbed_after_the_header() {
    let dir = temp("report");
    std::fs::write(dir.join(CURRENT), "now: streaming to Report test PC
").unwrap();
    std::fs::write(dir.join(PREV), "before: crashed
").unwrap();
    nightjar_core::report::redact::add_secret("Report test PC");
    let path = write_report(&dir, "Nightjar 9.9.9 / windows x86_64").unwrap();
    assert_eq!(path, dir.join(REPORT));
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.starts_with("Nightjar 9.9.9 / windows x86_64

This run's log (nightjar.log):
now: streaming to "), "{text}");
    assert!(!text.contains("Report test PC"), "{text}");
    assert!(text.ends_with("The run before's log (nightjar.prev.log):
before: crashed
"), "{text}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_problem_report_without_logs_is_its_header() {
    let dir = temp("report-empty");
    let text = std::fs::read_to_string(write_report(&dir, "Nightjar 9.9.9").unwrap()).unwrap();
    assert_eq!(text, "Nightjar 9.9.9");
    let _ = std::fs::remove_dir_all(&dir);
}
