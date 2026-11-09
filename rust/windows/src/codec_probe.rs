//! Optional Vulkan codec detection is isolated from the streaming host.
//!
//! An injected overlay can fault on a device that a short capability probe has
//! already destroyed. A failed or stalled probe must not take down AV1/HEVC.
use crate::{
    capture,
    encoder::Encoder,
    ipc::Pipe,
    process::{Process, Target},
};
use anyhow::{Result, bail, ensure};
use butterpollo_core::{config::Config, rtsp::Negotiated};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

const PIPE_PREFIX: &str = r"\\.\pipe\Butterpollo.CodecProbe.";
const TIMEOUT: Duration = Duration::from_secs(15);
/// A packed probe takes about 0.4 s per mode; an AMF call over 2 s has hung.
/// PyroWave's Vulkan probe takes about 2 s when healthy and is not judged.
const HANG: Duration = Duration::from_secs(2);
const FLAGS: u32 = 0x0780_0000;
const STATUS_DLL_INIT_FAILED: u32 = 0xC000_0142;

/// The probe could not run in the signed-in user's session yet: nobody is
/// signed in, or Windows could not initialize the child on the session's
/// desktop (STATUS_DLL_INIT_FAILED while it is still signing the user in).
/// Probing again later can succeed.
#[derive(Debug)]
pub struct SessionNotReady(String);
impl std::fmt::Display for SessionNotReady {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for SessionNotReady {}

/// The probe did not answer within its time limit, as when other work keeps
/// the CPU busy. Probing again once the host is idle can succeed.
#[derive(Debug)]
pub struct TimedOut;
impl std::fmt::Display for TimedOut {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("optional codec probe timed out")
    }
}
impl std::error::Error for TimedOut {}

/// The caller stopped the probe, as when a stream wants the encoder. The
/// worker is dropped, which ends its job and the child with it.
#[derive(Debug)]
pub struct Interrupted;
impl std::fmt::Display for Interrupted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("optional codec probe stopped for a stream")
    }
}
impl std::error::Error for Interrupted {}

impl SessionNotReady {
    /// For a fake probe in tests.
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

fn early_exit(code: u32) -> anyhow::Error {
    let message = format!("optional codec probe exited before replying (0x{code:08x})");
    if code == STATUS_DLL_INIT_FAILED {
        SessionNotReady(message).into()
    } else {
        anyhow::anyhow!(message)
    }
}
/// `requested` is when the worker got a packed request, None for PyroWave
/// or before the request. A worker still running [`HANG`] after it when a
/// stream stops it may hold an AMF call that hung, and the stream opens the
/// encoder at once: that is warned about.
fn hang_watch(packed: Option<&Negotiated>) -> Option<Instant> {
    packed.map(|_| Instant::now())
}
fn check_worker(
    exit_code: Option<u32>,
    now: Instant,
    deadline: Instant,
    stopped: bool,
    requested: Option<Instant>,
) -> Result<()> {
    if stopped {
        if let Some(elapsed) = requested
            .map(|at| now.saturating_duration_since(at))
            .filter(|elapsed| *elapsed >= HANG)
        {
            tracing::warn!(
                elapsed_ms = elapsed.as_millis() as u64,
                "optional codec probe still ran when a stream stopped it; the encoder may have hung"
            );
        }
        return Err(Interrupted.into());
    }
    if let Some(code) = exit_code {
        return Err(early_exit(code));
    }
    if now >= deadline {
        return Err(TimedOut.into());
    }
    Ok(())
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    output: String,
    /// The encoding GPU the settings name, as the stream will use it.
    adapter_name: String,
    adapter_pnp_id: String,
    #[serde(default)]
    packed: Option<Negotiated>,
    #[serde(default)]
    encoder_options: BTreeMap<String, String>,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
enum Reply {
    Done { flags: u32, errors: Vec<String> },
    Error { message: String },
}

/// Probe PyroWave in a child process. `stop` is checked while it runs:
/// true ends the child with [`Interrupted`].
pub fn pyrowave(config: &Config, stop: &dyn Fn() -> bool) -> Result<u32> {
    run(config, None, stop)
}

/// Probe one packed 4:4:4 mode in a child process, stopped as `pyrowave` is.
pub fn packed(config: &Config, stream: &Negotiated, stop: &dyn Fn() -> bool) -> Result<()> {
    stream.validate()?;
    ensure!(
        butterpollo_core::packed444::supported(stream.chroma_packing),
        "invalid packed probe request"
    );
    let flag = u32::from(butterpollo_core::packed444::mode_bit(
        stream.codec,
        stream.ten_bit(),
    ));
    ensure!(
        run(config, Some(stream), stop)? == flag,
        "packed hardware encoder produced no complete probe sequence"
    );
    Ok(())
}

fn run(config: &Config, packed: Option<&Negotiated>, stop: &dyn Fn() -> bool) -> Result<u32> {
    // A stream that started while the caller waited gets the encoder alone.
    if stop() {
        return Err(Interrupted.into());
    }
    // The pipe admits only the signed-in user, so it fails before sign-in.
    let (pipe, name) =
        Pipe::server(PIPE_PREFIX).map_err(|error| SessionNotReady(format!("{error:#}")))?;
    let program = std::env::current_exe()?;
    let worker = Process::spawn(
        &program,
        &[
            "--codec-probe-worker".into(),
            name.into(),
            "--codec-probe-parent".into(),
            std::process::id().to_string().into(),
        ],
        program.parent(),
        Target::User { elevated: false },
        &BTreeMap::new(),
        true,
    )
    .map_err(|error| SessionNotReady(format!("start optional codec probe: {error:#}")))?;
    let deadline = Instant::now() + TIMEOUT;
    let check = |requested: Option<Instant>| -> Result<()> {
        check_worker(
            worker.exit_code()?,
            Instant::now(),
            deadline,
            stop(),
            requested,
        )?;
        std::thread::sleep(Duration::from_millis(5));
        Ok(())
    };
    while !pipe.connected(worker.pid)? {
        check(None)?;
    }
    pipe.send(&Request {
        output: config.get("output_name", "").into(),
        adapter_name: config.get("adapter_name", "").into(),
        adapter_pnp_id: config.get("adapter_pnp_id", "").into(),
        packed: packed.cloned(),
        encoder_options: if packed.is_some() {
            encoder_options(config)
        } else {
            BTreeMap::new()
        },
    })?;
    let requested = hang_watch(packed);
    let reply = loop {
        if let Some(reply) = pipe.receive::<Reply>()? {
            break reply;
        }
        check(requested)?;
    };
    pipe.send(&())?;
    ensure!(
        worker.wait(Duration::from_secs(1))? == 0,
        "optional codec probe failed during shutdown"
    );
    match reply {
        Reply::Done { flags, errors } => {
            let allowed = if packed.is_some() {
                u32::from(butterpollo_core::packed444::ALL_MODES)
            } else {
                FLAGS
            };
            ensure!(flags & !allowed == 0, "invalid optional codec flags");
            if packed.is_some() {
                ensure!(
                    flags != 0,
                    "packed encoder unavailable: {}",
                    errors.join("; ")
                );
                return Ok(flags);
            }
            if flags == 0 {
                tracing::warn!(errors = %errors.join("; "), "PyroWave has no usable codec mode; clients may select H.264, HEVC or AV1 instead. Check the packaged PyroWave DLL and AMD graphics driver, then reconnect");
            }
            for error in errors {
                tracing::debug!(%error, "optional codec mode unavailable");
            }
            Ok(flags)
        }
        Reply::Error { message } => bail!("optional codec probe: {message}"),
    }
}

/// The settings the packed probe's encoder reads: the encoder choice and
/// the AMF or NVENC tuning, so it tests what a stream would open.
fn encoder_options(config: &Config) -> BTreeMap<String, String> {
    config
        .values
        .iter()
        .filter(|(key, _)| {
            key.starts_with("amd_") || key.starts_with("nvenc_") || key.as_str() == "encoder"
        })
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

fn probe(request: &Request) -> Result<Reply> {
    let _com = capture::ComGuard::new()?;
    let image = capture::Image {
        width: 640,
        height: 480,
        stride: 2560,
        bytes: vec![128; 640 * 480 * 4],
        captured: Instant::now(),
        pixel: capture::Pixel::Bgra8,
    };
    let mut flags = 0;
    let mut errors = Vec::new();
    let mut gpu = Config::default();
    for (key, value) in [
        ("adapter_name", &request.adapter_name),
        ("adapter_pnp_id", &request.adapter_pnp_id),
    ] {
        if !value.is_empty() {
            gpu.values.insert(key.into(), value.clone());
        }
    }
    if let Some(mode) = &request.packed {
        mode.validate()?;
        ensure!(
            butterpollo_core::packed444::supported(mode.chroma_packing),
            "invalid packed probe request"
        );
        gpu.values.extend(request.encoder_options.clone());
        let mut encoder = Probe {
            encoder: Encoder::new_options(mode, &request.output, &gpu)?,
            image: &image,
            bitrate_kbps: mode.bitrate_kbps,
        };
        packed_sequence(&mut encoder, PACKED_FRAME_WAIT)?;
        return Ok(Reply::Done {
            flags: u32::from(butterpollo_core::packed444::mode_bit(
                mode.codec,
                mode.ten_bit(),
            )),
            errors,
        });
    }
    for (hdr, yuv444, bit) in [
        (false, false, 0x0080_0000),
        (false, true, 0x0100_0000),
        (true, false, 0x0200_0000),
        (true, true, 0x0400_0000),
    ] {
        let mode = Negotiated {
            width: 640,
            height: 480,
            fps: 30,
            bitrate_kbps: 2000,
            codec: 3,
            hdr,
            yuv444,
            ..Default::default()
        };
        let result = (|| -> Result<bool> {
            let mut encoder = Encoder::new_options(&mode, &request.output, &gpu)?;
            for frame in 0..8 {
                if !encoder
                    .encode(&image, frame == 0, mode.bitrate_kbps)?
                    .is_empty()
                {
                    return Ok(true);
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            Ok(false)
        })();
        match result {
            Ok(true) => flags |= bit,
            Ok(false) => errors.push(format!("HDR={hdr} 4:4:4={yuv444}: no encoded frame")),
            Err(error) => errors.push(
                format!("HDR={hdr} 4:4:4={yuv444}: {error:#}")
                    .chars()
                    .take(600)
                    .collect(),
            ),
        }
    }
    Ok(Reply::Done { flags, errors })
}

/// How long the driver has to return each packed probe frame once it is
/// submitted.
const PACKED_FRAME_WAIT: Duration = Duration::from_secs(3);
/// The packed probe's view of an encoder: submit a frame, then collect what
/// it returns.
trait Sequence {
    /// Submits one frame and returns how many encoded frames came back with it.
    fn submit(&mut self, idr: bool) -> Result<usize>;
    fn pending(&self) -> bool;
    fn collect(&mut self) -> Result<usize>;
}
struct Probe<'a> {
    encoder: Encoder,
    image: &'a capture::Image,
    bitrate_kbps: u32,
}
impl Sequence for Probe<'_> {
    fn submit(&mut self, idr: bool) -> Result<usize> {
        let encoded = self.encoder.encode(self.image, idr, self.bitrate_kbps)?;
        Ok(frames(encoded.iter().map(|encoded| encoded.unit)))
    }
    fn pending(&self) -> bool {
        self.encoder.pending()
    }
    fn collect(&mut self) -> Result<usize> {
        let encoded = self.encoder.poll()?;
        Ok(frames(encoded.iter().map(|encoded| encoded.unit)))
    }
}
/// The frames that `units` finish. With slice delivery a P-frame comes back
/// as its slices, ending with `Last`: one frame, not one per slice. `auto`
/// slices a picture of [`AUTO_SLICE_PIXELS`] or more, and a packed stream's
/// coded picture is twice the height (2560x1600 codes 2560x3200), so counting
/// slices failed packed HEVC above 2560x1208 with "failed to return frame 2"
/// (issue #76).
///
/// [`AUTO_SLICE_PIXELS`]: butterpollo_core::encoder_policy::AUTO_SLICE_PIXELS
fn frames(units: impl Iterator<Item = crate::encoder::Unit>) -> usize {
    units
        .filter(|unit| !matches!(unit, crate::encoder::Unit::Slice { .. }))
        .count()
}
/// Three frames, each returned before the next is submitted. Each frame's
/// wait starts once it is submitted: the first upload and conversion after a
/// driver start can take seconds (4.7 s on an RX 7900 XT), and counting that
/// against the driver failed a codec the stream encodes fine.
fn packed_sequence(encoder: &mut impl Sequence, wait: Duration) -> Result<()> {
    let mut count = 0;
    for frame in 0..3 {
        let submitting = Instant::now();
        count += encoder.submit(frame == 0)?;
        let submitted = Instant::now();
        while encoder.pending() && submitted.elapsed() < wait {
            count += encoder.collect()?;
            std::thread::sleep(Duration::from_millis(1));
        }
        ensure!(
            count == frame + 1,
            "packed encoder failed to return frame {} within {} ms of submitting it (submitting took {} ms)",
            frame + 1,
            wait.as_millis(),
            submitted.duration_since(submitting).as_millis()
        );
    }
    Ok(())
}

pub fn worker(name: &str, parent: u32) -> Result<()> {
    let pipe = Pipe::client(name, parent, PIPE_PREFIX)?;
    let deadline = Instant::now() + TIMEOUT;
    let request = loop {
        if let Some(request) = pipe.receive::<Request>()? {
            break request;
        }
        ensure!(
            Instant::now() < deadline,
            "optional codec request timed out"
        );
        std::thread::sleep(Duration::from_millis(5));
    };
    let reply = probe(&request).unwrap_or_else(|error| Reply::Error {
        message: format!("{error:#}").chars().take(600).collect(),
    });
    pipe.send(&reply)?;
    let deadline = Instant::now() + Duration::from_secs(2);
    while pipe.receive::<()>()?.is_none() {
        ensure!(
            Instant::now() < deadline,
            "optional codec reply was not acknowledged"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
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
        // A crashing driver or overlay stays a plain failure.
        assert!(!early_exit(0xC000_0005).is::<SessionNotReady>());
        assert!(!early_exit(1).is::<SessionNotReady>());
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
        let error =
            packed_sequence(&mut fake(&[], Some(60)), Duration::from_millis(20)).unwrap_err();
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
                let reply = probe(&Request {
                    output: String::new(),
                    adapter_name: String::new(),
                    adapter_pnp_id: String::new(),
                    packed: Some(mode),
                    encoder_options: BTreeMap::new(),
                });
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
            let reply = probe(&Request {
                output: String::new(),
                adapter_name: String::new(),
                adapter_pnp_id: String::new(),
                packed: Some(mode),
                encoder_options: BTreeMap::new(),
            });
            let expected = u32::from(butterpollo_core::packed444::mode_bit(codec, true));
            match reply {
                Ok(Reply::Done { flags, .. }) => assert_eq!(flags, expected, "codec {codec}"),
                Ok(Reply::Error { message }) => panic!("codec {codec}: {message}"),
                Err(error) => panic!("codec {codec}: {error:#}"),
            }
        }
    }
}
