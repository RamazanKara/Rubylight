use super::*;

/// The header the Android app writes, read from its source so the two cannot drift apart.
fn android_header() -> Option<String> {
    let source = include_str!("../../../../app/src/main/java/com/limelight/binding/video/FrameLatencyStats.java");
    let start = source.find("CSV_HEADER =")?;
    let end = start + source[start..].find(';')?;
    let mut header = String::new();
    for line in source[start..end].lines() {
        let line = line.trim();
        if line.starts_with("//") {
            continue;
        }
        let mut quoted = line.split('"');
        quoted.next();
        while let Some(text) = quoted.next() {
            header.push_str(text);
            quoted.next();
        }
    }
    // The source spells the line end as a backslash and an n.
    Some(header.trim_end_matches("\\n").to_owned())
}

fn row() -> Row {
    Row {
        number: 7,
        rtp: 90_000,
        receive_us: 1_000_000,
        enqueue_us: 1_001_000,
        input_us: 1_001_500,
        output_us: 1_004_500,
        render_us: 1_010_000,
        host_latency: 25,
        drops: 3,
        clock: Some((-5_000, 800)),
        early: 1,
        first_block_us: 1_000_900,
        due_us: 0,
        begin_us: 0,
        race: RaceReport::default(),
        scan: ScanReport::default(),
        presented: Presented::default(),
        flip: Flip::default(),
        glass: Glass::default(),
    }
}

#[test]
fn the_header_is_the_androids_byte_for_byte() {
    let android = android_header().expect("Android's CSV_HEADER");
    let ours = header();
    let ours = ours.trim_end();
    assert!(ours.starts_with(android.trim_end()), "Android's columns come first, unchanged");
    assert_eq!(&ours[android.trim_end().len()..], format!(",{}", EXTRA_COLUMNS.join(",")));
    assert_eq!(OWN_COLUMNS.len() + 9 + SCHEDULER_COLUMNS.len(), android.split(',').count());
    assert_eq!(ours.split(',').count(), android.split(',').count() + EXTRA_COLUMNS.len());
}

#[test]
fn shown_fps_counts_the_presents_of_the_last_second() {
    let mut shown = ShownFps::default();
    assert_eq!(shown.push(5_000_000), None, "one present has no rate");
    let mut last = None;
    for i in 1..=120u64 {
        last = shown.push(5_000_000 + i * 16_667);
    }
    assert!((last.unwrap() - 60.0).abs() < 0.1, "{last:?}");
    // A stall: the next present, 2 s later, has only itself in the window.
    assert_eq!(shown.push(5_000_000 + 120 * 16_667 + 2_000_000), None);
}

#[test]
fn a_row_fills_the_columns_the_report_reads() {
    let fields = own_fields(&row(), 2, 1_000_000 + 1_000, Some(59.94));
    let fields: Vec<&str> = fields.split(',').collect();
    assert_eq!(fields.len(), OWN_COLUMNS.len());
    let at = |name: &str| fields[OWN_COLUMNS.iter().position(|c| *c == name).unwrap()];
    assert_eq!(at("frame_number"), "7");
    assert_eq!(at("pts_us"), "1000000", "90000 ticks of 90 kHz is one second");
    assert_eq!(at("receive_ns"), "1000000000");
    assert_eq!(at("decoder_input_ns"), "1001500000");
    assert_eq!(at("decoder_output_ns"), "1004500000");
    assert_eq!(at("render_ns"), "1010000000");
    assert_eq!(at("status"), "rendered");
    assert_eq!(at("input_to_output_ms"), "3.000000");
    assert_eq!(at("receive_to_present_ms"), "10.000000");
    assert_eq!(at("host_processing_ms"), "2.500000");
    assert_eq!(at("decode_interval_ms"), "3.500000");
    assert_eq!(at("csv_rows_lost"), "2");
    assert_eq!(at("present_drops"), "3");
    assert_eq!(at("shown_fps"), "59.94");
    assert_eq!(at("release_ns"), "", "MediaCodec's release time has no counterpart");
}

#[test]
fn the_file_has_the_host_columns_from_the_report_and_the_offset() {
    let dir = std::env::temp_dir().join(format!("nightjar-timing-{}", std::process::id()));
    let path = resolve(&dir);
    assert_eq!(path, dir, "a path that is not a folder yet is the file itself");
    let csv = TimingCsv::create(&path).unwrap();
    let timing = HostTiming { frame: 7, host_us: 900, queue_us: 300, encode_us: 200, send_us: 100 };
    let extra = HostTimingExtra { presented_at_us: 2_000_000, capture_us: 100, convert_us: 150 };
    csv.reports(&[(timing, Some(extra))]);
    csv.row(row());
    // A frame without a report is still written, with empty host stamps.
    csv.row(Row { number: 8, ..row() });
    drop(csv);
    let text = std::fs::read_to_string(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 3, "{text}");
    assert_eq!(format!("{}\n", lines[0]), header());
    let tail = |line: &str| line.split(',').skip(OWN_COLUMNS.len()).collect::<Vec<_>>().join(",");
    assert_eq!(tail(lines[1]), "1000000,2000000,2000100,2000300,2000500,2000800,2000900,-5000,800,,,,,,,,1,1000900000,,,,,,,,,,,,,,,,,,,,,");
    assert_eq!(tail(lines[2]), "1000000,,,,,,,-5000,800,,,,,,,,1,1000900000,,,,,,,,,,,,,,,,,,,,,");
    for line in &lines {
        assert_eq!(line.split(',').count(), header().trim_end().split(',').count());
    }
}

#[test]
fn the_early_columns_say_how_the_frame_was_decoded() {
    assert_eq!(extra_fields(&Row { early: 0, first_block_us: 0, ..row() }), "0,,,,,,,,,,,,,,,,,,,,,,");
    assert_eq!(extra_fields(&Row { early: 2, first_block_us: 0, ..row() }), "2,,,,,,,,,,,,,,,,,,,,,,");
    assert_eq!(extra_fields(&row()), "1,1000900000,,,,,,,,,,,,,,,,,,,,,");
    let timed = Row {
        due_us: 1_008_000,
        begin_us: 1_008_100,
        flip: Flip { present_count: 7, stat_present_count: 6, stat_present_refresh: 100, stat_sync_refresh: 101, stat_sync_us: 1_009_500 },
        ..row()
    };
    assert_eq!(extra_fields(&timed), "1,1000900000,1008000000,1008100000,7,6,100,101,1009500000,,,,,,,,,,,,,,");
    // Rows come back in the order they went in, each with its own columns.
    let path = std::env::temp_dir().join(format!("nightjar-timing-early-{}", std::process::id()));
    let csv = TimingCsv::create(&path).unwrap();
    for (number, early) in [(1, 0u8), (2, 1), (3, 2)] {
        csv.row(Row { number, early, first_block_us: if early == 1 { 1_000_900 } else { 0 }, ..row() });
    }
    drop(csv);
    let text = std::fs::read_to_string(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    let ends: Vec<String> = text.lines().skip(1).map(|l| { let f: Vec<&str> = l.split(',').skip(l.split(',').count() - EXTRA_COLUMNS.len()).collect(); format!("{}|{}", f[1], f[0]) }).collect();
    assert_eq!(ends, ["|0", "1000900000|1", "|2"]);
}

#[test]
fn the_race_columns_say_how_the_slices_went() {
    let raced = |slices, ordered| Row {
        race: RaceReport { slices, first_ready_us: 1_003_000, last_ready_us: 1_004_400, ordered },
        ..row()
    };
    let tail = |row: &Row| extra_fields(row).split(',').skip(EXTRA_COLUMNS.len() - GLASS - 11).take(4).collect::<Vec<_>>().join(",");
    assert_eq!(tail(&raced(4, RaceReport::ORDERED)), "4,1003000000,1004400000,1");
    assert_eq!(tail(&raced(1, RaceReport::UNORDERED)), "1,1003000000,1004400000,0", "a frame that left the row order says so");
    assert_eq!(tail(&row()), ",,,", "the race off leaves the columns empty");
    let names = &EXTRA_COLUMNS[EXTRA_COLUMNS.len() - GLASS - 11..EXTRA_COLUMNS.len() - GLASS - 7];
    assert_eq!(names, ["slices_decoded", "first_slice_ready_ns", "last_slice_ready_ns", "row_ordered"]);
}

#[test]
fn the_scan_columns_say_where_the_beam_was_and_why_racing_is_off() {
    let tail = |row: &Row| extra_fields(row).split(',').skip(EXTRA_COLUMNS.len() - GLASS - 7).take(3).collect::<Vec<_>>().join(",");
    let scan = |line, vblank, reason| Row { scan: ScanReport { line, vblank, reason }, ..row() };
    assert_eq!(tail(&scan(Some(412), Some(false), "on")), "412,0,on");
    assert_eq!(tail(&scan(Some(7), Some(true), "no VRR scan restart")), "7,1,no VRR scan restart");
    assert_eq!(tail(&scan(None, None, "no scanline")), ",,no scanline", "no reading, but the reason stays");
    assert_eq!(tail(&scan(Some(90), Some(true), "")), "90,1,", "no verdict yet");
    assert_eq!(tail(&row()), ",,", "the race off leaves the columns empty");
    let names = &EXTRA_COLUMNS[EXTRA_COLUMNS.len() - GLASS - 7..EXTRA_COLUMNS.len() - GLASS - 4];
    assert_eq!(names, ["scan_line_at_present", "in_vblank_at_present", "race_reason"]);
}

#[test]
fn the_presented_columns_say_how_the_frame_went_to_the_screen() {
    let tail = |row: &Row| extra_fields(row).split(',').skip(EXTRA_COLUMNS.len() - GLASS - 4).take(4).collect::<Vec<_>>().join(",");
    let mut presented = Presented { scan_start_us: 2_000_800, mid_row_age_ns: Some(3_200_000), late_slices: Some(0), ..Default::default() };
    presented.set_presents(&[2_000_000, 2_001_400, 2_002_900, 2_004_100]);
    assert_eq!(tail(&Row { presented, ..row() }), "2000800000,2000000000|2001400000|2002900000|2004100000,3200000,0");
    // A frame shown whole has one present and may have a picture older than the slice was ready.
    let mut whole = Presented { scan_start_us: 2_000_800, mid_row_age_ns: Some(-10), late_slices: None, ..Default::default() };
    whole.set_presents(&[2_000_000]);
    assert_eq!(tail(&Row { presented: whole, ..row() }), "2000800000,2000000000,-10,");
    assert_eq!(tail(&row()), ",,,", "the race off leaves the columns empty");
    let mut many = Presented::default();
    many.set_presents(&[1; 20]);
    assert_eq!(usize::from(many.count), PRESENTS, "a row keeps as many presents as it has room for");
    let names = &EXTRA_COLUMNS[EXTRA_COLUMNS.len() - GLASS - 4..EXTRA_COLUMNS.len() - GLASS];
    assert_eq!(names, ["scan_start_ns", "present_ns", "mid_row_age_ns", "late_slices"]);
}

/// The glass columns at the end.
const GLASS: usize = 3;

#[test]
fn the_glass_columns_carry_the_newest_presentation_answer() {
    let tail = |row: &Row| extra_fields(row).split(',').skip(EXTRA_COLUMNS.len() - GLASS).collect::<Vec<_>>().join(",");
    let glass = Glass { frame: Some(5), to_glass_us: Some(6_250), flags: Some(crate::stream::wl_presentation::VSYNC | crate::stream::wl_presentation::ZERO_COPY) };
    assert_eq!(tail(&Row { glass, ..row() }), "5,6.250000,vsync|zero_copy");
    assert_eq!(tail(&Row { glass: Glass { to_glass_us: None, ..glass }, ..row() }), "5,,vsync|zero_copy", "an untimed answer keeps its flags");
    assert_eq!(tail(&row()), ",,", "no answer leaves the columns empty");
    assert_eq!(&EXTRA_COLUMNS[EXTRA_COLUMNS.len() - GLASS..], ["glass_frame", "present_to_glass_ms", "presentation_flags"]);
}

#[test]
fn a_stalled_writer_never_blocks_the_caller() {
    let dir = std::env::temp_dir().join(format!("nightjar-timing-stall-{}", std::process::id()));
    let csv = TimingCsv::create(&dir).unwrap();
    // The channel holds 4096; the writer drains it, but nothing may ever wait on it.
    let started = std::time::Instant::now();
    for number in 0..20_000 {
        csv.row(Row { number, ..row() });
    }
    assert!(started.elapsed() < Duration::from_secs(5), "queuing 20000 rows took {:?}", started.elapsed());
    let lost = csv.lost();
    drop(csv);
    let rows = std::fs::read_to_string(&dir).unwrap().lines().count() - 1;
    let _ = std::fs::remove_file(&dir);
    assert_eq!(rows as u64 + lost, 20_000, "every row is written or counted as lost");
}

/// Manual: `RUBYLIGHT_TIMING_REPORT=<path to rust/release/timing_report.py> cargo test -p
/// nightjar-desktop timing_report -- --ignored`: Rubylight's report reads what is written.
#[test]
#[ignore = "needs Rubylight's timing_report.py; see the doc comment"]
fn timing_report_accepts_a_file_the_desktop_wrote() {
    let Some(script) = crate::sdl::test_input("RUBYLIGHT_TIMING_REPORT", "Rubylight's timing_report.py") else { return };
    let script = script.to_string_lossy().into_owned();
    let path = std::env::temp_dir().join(format!("nightjar-timing-report-{}.csv", std::process::id()));
    let csv = TimingCsv::create(&path).unwrap();
    for number in 0..40u32 {
        let base = 5_000_000 + u64::from(number) * 16_667;
        let present = 100_000 + u64::from(number) * 16_667;
        let timing = HostTiming { frame: number, host_us: 3_000, queue_us: 1_000, encode_us: 1_500, send_us: 200 };
        let extra = HostTimingExtra { presented_at_us: present, capture_us: 200, convert_us: 300 };
        csv.reports(&[(timing, Some(extra))]);
        csv.row(Row {
            number,
            rtp: (u64::from(number) * 1_500) as u32,
            receive_us: base,
            enqueue_us: base + 300,
            input_us: base + 400,
            output_us: base + 2_400,
            render_us: base + 5_000,
            host_latency: 0,
            drops: 0,
            // client = host + 4.9 s
            clock: Some((4_900_000, 600)),
            early: 1,
            first_block_us: base + 1_000,
            due_us: 0,
            begin_us: 0,
            race: RaceReport::default(),
            scan: ScanReport::default(),
            presented: Presented::default(),
            flip: Flip::default(),
            glass: Glass::default(),
        });
    }
    drop(csv);
    let output = std::process::Command::new("python").args([&script]).arg(&path).output().expect("python");
    let _ = std::fs::remove_file(&path);
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "timing_report.py failed: {}{}", text, String::from_utf8_lossy(&output.stderr));
    println!("{text}");
    assert!(text.contains("clock_sync"), "joined with the clock offset: {text}");
    assert!(text.contains("end_to_end") && text.contains("decode"), "{text}");
}
