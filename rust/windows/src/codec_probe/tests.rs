use super::*;
#[test]
fn the_packed_probe_gets_the_encoder_settings_of_either_vendor() {
    let config = Config::parse(
        "encoder = nvenc\nnvenc_preset = 4\namd_quality = quality\nadapter_name = GPU\nport = 48123\n",
    )
    .unwrap();
    let options = encoder_options(&config);
    assert_eq!(
        options.keys().map(String::as_str).collect::<Vec<_>>(),
        ["amd_quality", "encoder", "nvenc_preset"]
    );
    assert_eq!(options["nvenc_preset"], "4");
}
#[test]
fn optional_codec_worker_exit_and_stall_end_the_probe_with_an_error() {
    let start = Instant::now();
    let deadline = start + TIMEOUT;
    assert!(check_worker(None, start, deadline, false, None).is_ok());
    for code in [0, 1, 0xc0000005] {
        let error = check_worker(Some(code), start, deadline, false, None).unwrap_err();
        assert!(error.to_string().contains("exited before replying"));
        assert!(!error.is::<SessionNotReady>());
    }
    for now in [deadline, deadline + Duration::from_secs(1)] {
        let error = check_worker(None, now, deadline, false, Some(start)).unwrap_err();
        assert_eq!(error.to_string(), "optional codec probe timed out");
        // A stall under load is probed again once the host is idle.
        assert!(error.is::<TimedOut>());
        assert!(!error.is::<SessionNotReady>());
    }
}
/// A stream that starts while a re-probe runs ends the probe at its next
/// check instead of sharing the encoder for up to 15 s.
#[test]
fn a_stopped_probe_ends_at_once_before_any_other_verdict() {
    let start = Instant::now();
    let deadline = start + TIMEOUT;
    for (exit_code, now) in [(None, start), (Some(1), start), (None, deadline)] {
        let error = check_worker(exit_code, now, deadline, true, None).unwrap_err();
        assert!(error.is::<Interrupted>());
        assert_eq!(
            error.to_string(),
            "optional codec probe stopped for a stream"
        );
    }
}
/// A stream that started while the caller waited for the probe's turn
/// starts no worker at all.
#[test]
fn a_probe_stopped_before_it_starts_starts_no_worker() {
    let error = pyrowave(&Config::default(), &|| true).unwrap_err();
    assert!(error.is::<Interrupted>());
}
fn warnings(check: impl FnOnce()) -> String {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("probe.log");
    let file = std::fs::File::create(&path).unwrap();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_target(false)
        .with_max_level(tracing::Level::WARN)
        .with_writer(move || file.try_clone().unwrap())
        .finish();
    tracing::subscriber::with_default(subscriber, check);
    std::fs::read_to_string(path).unwrap()
}
/// A worker a stream stops more than 2 s after its request, as when an
/// AMF call hangs, is a possible encoder hang: the stream opens the
/// encoder at once, so the log must say so.
#[test]
fn a_probe_stopped_past_its_normal_time_warns_of_an_encoder_hang() {
    let start = Instant::now();
    let deadline = start + TIMEOUT;
    for now in [start, start + Duration::from_millis(1999)] {
        let log = warnings(|| {
            let error = check_worker(None, now, deadline, true, Some(start)).unwrap_err();
            assert!(error.is::<Interrupted>());
        });
        assert!(log.is_empty(), "{log}");
    }
    let log = warnings(|| {
        let error = check_worker(None, start + HANG, deadline, true, None).unwrap_err();
        assert!(error.is::<Interrupted>());
    });
    assert!(log.is_empty(), "no request, no encoder call: {log}");
    let log = warnings(|| {
        let now = start + Duration::from_millis(2500);
        let error = check_worker(None, now, deadline, true, Some(start)).unwrap_err();
        assert!(error.is::<Interrupted>());
    });
    assert!(log.contains("WARN"), "{log}");
    assert!(log.contains("the encoder may have hung"), "{log}");
    assert!(log.contains("elapsed_ms=2500"), "{log}");
}
/// PyroWave's healthy probe takes about 2 s, so a stream that stops a
/// slow one is no sign of a hung encoder; a packed request is judged.
#[test]
fn only_a_packed_probe_is_judged_for_an_encoder_hang() {
    assert!(hang_watch(Some(&Negotiated::default())).is_some());
    let requested = hang_watch(None);
    assert!(requested.is_none());
    let start = Instant::now();
    let log = warnings(|| {
        let now = start + Duration::from_millis(2500);
        let error = check_worker(None, now, start + TIMEOUT, true, requested).unwrap_err();
        assert!(error.is::<Interrupted>());
    });
    assert!(log.is_empty(), "{log}");
}
#[test]
fn only_a_session_desktop_failure_is_worth_probing_again() {
    let failed = early_exit(0xC000_0142);
    assert!(failed.is::<SessionNotReady>());
    assert_eq!(
        failed.to_string(),
        "optional codec probe exited before replying (0xc0000142)"
    );
    // A crashing driver or overlay, or an error exit, is a crash with its
    // code, which the host probes again a few times (issue #94).
    for code in [0xC000_0005, 1] {
        let crashed = early_exit(code);
        assert!(!crashed.is::<SessionNotReady>());
        assert!(crashed.is::<Crashed>());
    }
    assert_eq!(
        early_exit(0xC000_0005).to_string(),
        "optional codec probe exited before replying (0xc0000005)"
    );
}
/// An encoder whose submissions take `submitting` and whose frames come
/// back `encoding` after they were submitted.
struct Fake {
    submitting: Vec<Duration>,
    encoding: Option<Duration>,
    submitted: Vec<Instant>,
    returned: usize,
}
impl Sequence for Fake {
    fn submit(&mut self, _idr: bool) -> Result<usize> {
        std::thread::sleep(
            self.submitting
                .get(self.submitted.len())
                .copied()
                .unwrap_or_default(),
        );
        self.submitted.push(Instant::now());
        Ok(0)
    }
    fn pending(&self) -> bool {
        self.returned < self.submitted.len()
    }
    fn collect(&mut self) -> Result<usize> {
        let ready = self
            .encoding
            .is_some_and(|encoding| self.submitted[self.returned].elapsed() >= encoding);
        self.returned += usize::from(ready);
        Ok(usize::from(ready))
    }
}
fn fake(submitting: &[u64], encoding: Option<u64>) -> Fake {
    Fake {
        submitting: submitting
            .iter()
            .copied()
            .map(Duration::from_millis)
            .collect(),
        encoding: encoding.map(Duration::from_millis),
        submitted: Vec::new(),
        returned: 0,
    }
}
#[test]
fn a_slow_first_submission_does_not_count_against_the_packed_frame_wait() {
    // The RX 7900 XT took 4.7 s to upload and convert the first packed
    // 1080p frame; later frames came back within milliseconds. The wait
    // used to start before that submission, so 1080p120 was refused.
    let mut encoder = fake(&[120], Some(5));
    packed_sequence(&mut encoder, Duration::from_millis(60)).unwrap();
    assert_eq!(encoder.returned, 3);
}
#[test]
fn a_frame_the_driver_never_returns_still_fails_the_packed_probe() {
    let error = packed_sequence(&mut fake(&[], None), Duration::from_millis(20)).unwrap_err();
    assert!(
        error
            .to_string()
            .starts_with("packed encoder failed to return frame 1 within 20 ms"),
        "{error:#}"
    );
    // Each frame gets its own wait, but a late one still fails.
    let error = packed_sequence(&mut fake(&[], Some(60)), Duration::from_millis(20)).unwrap_err();
    assert!(error.to_string().contains("frame 1"), "{error:#}");
}
#[test]
fn a_sliced_packed_frame_counts_once() {
    use crate::encoder::Unit;
    // Issue #76: packed HEVC failed its probe above 2560x1208 and
    // 1920x1616, where `auto` slices the doubled coded picture, because
    // each slice of frame 2 was counted as a frame.
    for (width, height, coded, sliced) in [
        (2560, 1600, (2560, 3200), true),
        (2560, 1208, (2560, 2416), false),
        (2560, 1210, (2560, 2432), true),
        (1920, 1616, (1920, 3232), false),
        (1920, 1618, (1920, 3248), true),
    ] {
        let mode = Negotiated {
            width,
            height,
            fps: 120,
            bitrate_kbps: 48_588,
            codec: 1,
            chroma_packing: butterpollo_core::packed444::VERSION,
            ..Default::default()
        };
        assert_eq!(mode.coded_size(), coded, "{width}x{height}");
        // What the AMF and NVENC encoders open: the coded picture.
        let mut coded_mode = mode.clone();
        (coded_mode.width, coded_mode.height) = coded;
        coded_mode.chroma_packing = 0;
        let blocks =
            butterpollo_core::encoder_policy::slice_delivery(&Config::default(), &coded_mode);
        assert_eq!(blocks, if sliced { 4 } else { 0 }, "{width}x{height}");
        let units: Vec<Unit> = if sliced {
            (0..u32::from(blocks) - 1)
                .map(|index| Unit::Slice { index })
                .chain([Unit::Last {
                    index: u32::from(blocks) - 1,
                }])
                .collect()
        } else {
            vec![Unit::Whole]
        };
        assert_eq!(frames(units.into_iter()), 1, "{width}x{height}");
    }
    assert_eq!(frames([Unit::Whole, Unit::Whole].into_iter()), 2);
    assert_eq!(frames([Unit::Slice { index: 0 }].into_iter()), 0);
}
#[test]
#[ignore = "requires an AMD GPU with AMF"]
fn packed_probe_accepts_1080p_at_60_and_120_fps() {
    for (codec, sdr_10bit) in [(1, false), (1, true), (2, false), (2, true)] {
        for fps in [60, 120] {
            let mode = Negotiated {
                width: 1920,
                height: 1080,
                fps,
                bitrate_kbps: 20_000,
                codec,
                sdr_10bit,
                chroma_packing: butterpollo_core::packed444::VERSION,
                ..Default::default()
            };
            let reply = probe(
                &Request {
                    output: String::new(),
                    adapter_name: String::new(),
                    adapter_pnp_id: String::new(),
                    packed: Some(mode),
                    encoder_options: BTreeMap::new(),
                },
                &mut Vec::new(),
            );
            let expected = u32::from(butterpollo_core::packed444::mode_bit(codec, sdr_10bit));
            match reply {
                Ok(Reply::Done { flags, .. }) => assert_eq!(flags, expected),
                Ok(Reply::Error { message }) => {
                    panic!("codec {codec} 10-bit {sdr_10bit} {fps} fps: {message}")
                }
                Err(error) => panic!("codec {codec} 10-bit {sdr_10bit} {fps} fps: {error:#}"),
            }
        }
    }
}
#[test]
#[ignore = "requires an AMD GPU with AMF"]
fn packed_probe_accepts_sliced_hevc_at_2560x1600() {
    // #76: coded at 2560x3200, above the automatic slice threshold, so
    // each frame comes back as four slices.
    for sdr_10bit in [false, true] {
        let mode = Negotiated {
            width: 2560,
            height: 1600,
            fps: 60,
            bitrate_kbps: 40_000,
            codec: 1,
            sdr_10bit,
            chroma_packing: butterpollo_core::packed444::VERSION,
            ..Default::default()
        };
        let reply = probe(
            &Request {
                output: String::new(),
                adapter_name: String::new(),
                adapter_pnp_id: String::new(),
                packed: Some(mode),
                encoder_options: BTreeMap::new(),
            },
            &mut Vec::new(),
        );
        match reply {
            Ok(Reply::Done { flags, .. }) => assert_eq!(
                flags,
                u32::from(butterpollo_core::packed444::mode_bit(1, sdr_10bit))
            ),
            Ok(Reply::Error { message }) => panic!("10-bit {sdr_10bit}: {message}"),
            Err(error) => panic!("10-bit {sdr_10bit}: {error:#}"),
        }
    }
}
#[test]
#[ignore = "requires an AMD GPU with AMF"]
fn packed_probe_holds_a_bitrate_above_the_driver_ceiling() {
    // #80: AMF refused Av1TargetBitrate above its ceiling with
    // AMF_OUT_OF_RANGE. The RX 7900 XT's ceiling is 1 Gbps on every codec;
    // 1080p keeps the packed picture within its encoder throughput.
    for codec in [1, 2] {
        let mode = Negotiated {
            width: 1920,
            height: 1080,
            fps: 60,
            bitrate_kbps: 1_500_000,
            codec,
            sdr_10bit: true,
            chroma_packing: butterpollo_core::packed444::VERSION,
            ..Default::default()
        };
        let reply = probe(
            &Request {
                output: String::new(),
                adapter_name: String::new(),
                adapter_pnp_id: String::new(),
                packed: Some(mode),
                encoder_options: BTreeMap::new(),
            },
            &mut Vec::new(),
        );
        let expected = u32::from(butterpollo_core::packed444::mode_bit(codec, true));
        match reply {
            Ok(Reply::Done { flags, .. }) => assert_eq!(flags, expected, "codec {codec}"),
            Ok(Reply::Error { message }) => panic!("codec {codec}: {message}"),
            Err(error) => panic!("codec {codec}: {error:#}"),
        }
    }
}
