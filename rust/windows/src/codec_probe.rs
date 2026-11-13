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
use windows::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};

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

/// The worker ended before it replied, with the exit code in the message: a
/// fault in a graphics driver or an injected overlay (0xc0000005) or an error
/// exit. On one RTX 5070 it came and went between host starts (issue #94),
/// so a few more tries can find the codec.
#[derive(Debug)]
pub struct Crashed(String);
impl std::fmt::Display for Crashed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for Crashed {}

impl SessionNotReady {
    /// For a fake probe in tests.
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}
impl Crashed {
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
        Crashed(message).into()
    }
}
/// A pipe that breaks before the reply means the worker ended. Its exit code
/// says how; the pipe's own error ("The pipe has been ended") said neither,
/// which left issue #94 without the code of the fault.
fn ended(worker: &Process, error: anyhow::Error) -> anyhow::Error {
    match worker.wait(Duration::from_secs(1)) {
        Ok(code) => early_exit(code),
        Err(_) => error,
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
    })
    .map_err(|error| ended(&worker, error))?;
    let requested = hang_watch(packed);
    let reply = loop {
        match pipe.receive::<Reply>() {
            Ok(Some(reply)) => break reply,
            Ok(None) => check(requested)?,
            Err(error) => return Err(ended(&worker, error)),
        }
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
                tracing::warn!(errors = %errors.join("; "), "PyroWave has no usable codec mode; clients may select H.264, HEVC or AV1 instead. Check the packaged PyroWave DLL and the graphics driver, then reconnect");
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

/// Probe what `request` asks for. The PyroWave encoders stay open in `open`
/// for [`worker`] to end the process with.
fn probe(request: &Request, open: &mut Vec<Encoder>) -> Result<Reply> {
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
        let result = Encoder::new_options(&mode, &request.output, &gpu).and_then(|mut encoder| {
            let encoded = first_frame(&mut encoder, &image, mode.bitrate_kbps);
            open.push(encoder);
            encoded
        });
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
/// Whether `encoder` returns a frame within eight tries.
fn first_frame(encoder: &mut Encoder, image: &capture::Image, bitrate_kbps: u32) -> Result<bool> {
    for frame in 0..8 {
        if !encoder.encode(image, frame == 0, bitrate_kbps)?.is_empty() {
            return Ok(true);
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    Ok(false)
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
    let mut open = Vec::new();
    let reply = probe(&request, &mut open).unwrap_or_else(|error| Reply::Error {
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
    if !open.is_empty() {
        end_without_teardown(open);
    }
    Ok(())
}
/// Ends the worker with its PyroWave encoders still open, once the host has
/// the reply. On one RTX 5070 the worker exited before replying on half of
/// its host starts, as long into the probe as a probe that passed, so most
/// likely as the encoders closed (issue #94). A Vulkan driver or an injected
/// overlay can fault on a device that is being destroyed; closing none of
/// them leaves nothing to fault on. Windows frees what the process held, and
/// TerminateProcess runs no DLL detach code either.
fn end_without_teardown(open: Vec<Encoder>) -> ! {
    std::mem::forget(open);
    // SAFETY: Ends this process, whose reply is sent and acknowledged; the
    // pseudo-handle needs no closing.
    unsafe {
        let _ = TerminateProcess(GetCurrentProcess(), 0);
    }
    std::process::exit(0)
}

#[cfg(test)]
mod tests;
