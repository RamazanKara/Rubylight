//! A start's decoder checks on a worker thread.
//!
//! The 4:4:4/10-bit check, the plain HEVC and AV1 check and the packed full-chroma check each
//! open a decoder and decode fixtures. A driver reset inside such a call cannot be interrupted:
//! run from `App::launch_ready`, they held the frame that draws the window and answers Cancel and
//! the deadline (nightjar#115's symptom class). Their decodes run here instead. The packed check's
//! draw and readback stay on the renderer's thread (`packed444::draw_decoded`), whose render
//! targets they are; only its decodes moved.
//!
//! The worker holds a `DeviceRef` on the renderer's device for its whole run, so a renderer made
//! again after a driver reset does not free the device under it. A check that does not answer is
//! abandoned after [`PROBE_TIMEOUT`]: its worker ends on its own (a thread inside a driver call
//! cannot be stopped), its device stays held until then, and the start ends with a message that
//! names the graphics driver.

use super::*;

/// How long a start's decoder checks may run before the start ends. The checks decode a few small
/// fixtures (well under a second while the graphics driver answers), so only a decoder or device
/// call that cannot answer, as after a driver reset, reaches it.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(30);

/// Whether decoder checks started at `started` have waited [`PROBE_TIMEOUT`] at `now`.
pub fn probe_overdue(started: Instant, now: Instant) -> bool {
    now.saturating_duration_since(started) >= PROBE_TIMEOUT
}

/// What a worker checks.
pub(crate) enum ProbeWork {
    /// The 4:4:4/10-bit formats and/or the plain HEVC and AV1 ones, each with the key its result
    /// is kept under and the formats to check. `device`: the renderer's device (Direct3D 11 on
    /// Windows, the SDL GPU device elsewhere) for the plane shader check.
    ColorAndBase {
        color: Option<(ProbeKey, u32)>,
        base: Option<(ProbeKey, u32)>,
        device: *mut std::ffi::c_void,
    },
    /// The packed full-chroma fixtures, decoded for `hardware`; the launcher draws them.
    Packed { hardware: Hardware },
}

/// What a worker found.
pub(crate) enum ProbeResult {
    ColorAndBase {
        color: Option<(ProbeKey, (u32, String))>,
        base: Option<(ProbeKey, stream::color_probe::Base)>,
    },
    Packed(Result<stream::packed444::Decodes, String>),
}

// The renderer's device pointer is read on the main thread, and the device is held by a
// `DeviceRef` for the worker's whole run: Direct3D 11 devices are free-threaded, and SDL's GPU
// device is used off the presenting thread for shader creation by `pyrowave_mode::settings_probe`
// too. Nothing else of the renderer crosses.
unsafe impl Send for ProbeWork {}

impl ProbeWork {
    fn run(self) -> ProbeResult {
        match self {
            Self::ColorAndBase { color, base, device } => ProbeResult::ColorAndBase {
                color: color.map(|(key, wanted)| {
                    let found = stream::color_probe::probe(key.0, wanted, |ten_bit| {
                        stream::color_probe::planes_drawable_device(device, ten_bit)
                    });
                    (key, found)
                }),
                base: base.map(|(key, wanted)| (key, stream::color_probe::probe_base(key.0, wanted))),
            },
            Self::Packed { hardware } => ProbeResult::Packed(stream::packed444::decode_all(hardware)),
        }
    }
}

/// What the launcher does with a check this frame.
pub(crate) enum ProbeStep {
    Waiting,
    /// Nothing answered in [`PROBE_TIMEOUT`].
    Overdue,
    Done(ProbeResult),
    /// The worker ended without a result (it panicked).
    Failed,
}

/// A start's decoder checks running on a worker thread.
pub(crate) struct LaunchProbe {
    /// The start these checks belong to, for its failed card.
    launch: (u64, i32, bool, LaunchKind),
    app_name: String,
    /// The decoder the packed fixtures were decoded for (its result's cache key).
    hardware: Hardware,
    result: Receiver<ProbeResult>,
    worker: Option<std::thread::JoinHandle<()>>,
    /// The renderer's device, held for the worker and until the launcher lets go of the check.
    device: media::DeviceRef,
    started: Instant,
}

impl LaunchProbe {
    /// Starts `work` on a worker with a reference on `device`, which must be the renderer's device
    /// just read on the main thread.
    pub(crate) fn start(work: ProbeWork, device: media::DeviceRef, launch: (u64, i32, bool, LaunchKind), app_name: String, hardware: Hardware) -> Self {
        let (sender, result) = channel();
        let worker_device = device.clone();
        let worker = std::thread::Builder::new()
            .name("probe".into())
            .spawn(move || {
                let _held = worker_device;
                let _ = sender.send(work.run());
            })
            .inspect_err(|e| eprintln!("nightjar: the decoder check thread cannot start: {e}"))
            .ok();
        Self { launch, app_name, hardware, result, worker, device, started: Instant::now() }
    }

    pub(crate) fn launch(&self) -> (u64, i32, bool, LaunchKind) {
        self.launch
    }

    pub(crate) fn app_name(&self) -> &str {
        &self.app_name
    }

    pub(crate) fn hardware(&self) -> Hardware {
        self.hardware
    }

    pub(crate) fn device(&self) -> &media::DeviceRef {
        &self.device
    }

    /// The result first, so one that arrives just under [`PROBE_TIMEOUT`] is read and not reported
    /// as no answer.
    pub(crate) fn poll(&mut self) -> ProbeStep {
        match self.result.try_recv() {
            Ok(result) => ProbeStep::Done(result),
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                if probe_overdue(self.started, Instant::now()) { ProbeStep::Overdue } else { ProbeStep::Waiting }
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => ProbeStep::Failed,
        }
    }

    /// Whether the worker has ended (its result may still be waiting to be read).
    pub(crate) fn ended(&mut self) -> bool {
        self.worker.as_ref().is_none_or(|worker| worker.is_finished())
    }

    /// Waits for the worker until `deadline`: true once it ended. The result is not wanted: a
    /// check the launcher let go of does not draw anything.
    pub(crate) fn finish_until(mut self, deadline: Instant) -> bool {
        drop(self.result);
        let Some(worker) = self.worker.take() else { return true };
        while !worker.is_finished() {
            if Instant::now() >= deadline { return false; }
            std::thread::sleep(Duration::from_millis(2));
        }
        if worker.join().is_err() {
            eprintln!("nightjar: the decoder check thread panicked");
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_check_stops_being_waited_for_at_its_deadline() {
        let started = Instant::now();
        assert!(!probe_overdue(started, started + PROBE_TIMEOUT - Duration::from_millis(1)));
        assert!(probe_overdue(started, started + PROBE_TIMEOUT));
    }

    #[test]
    fn a_check_that_answered_before_the_deadline_is_read() {
        let (sender, result) = channel();
        let worker = std::thread::spawn(move || { let _ = sender.send(ProbeResult::Packed(Ok(Vec::new()))); });
        let mut probe = LaunchProbe {
            launch: (1, 9, false, LaunchKind::Stream),
            app_name: "Zelda".into(),
            hardware: Hardware::None,
            result,
            worker: Some(worker),
            device: media::DeviceRef::default(),
            started: Instant::now() - PROBE_TIMEOUT,
        };
        while !probe.ended() { std::thread::sleep(Duration::from_millis(1)); }
        assert!(matches!(probe.poll(), ProbeStep::Done(ProbeResult::Packed(Ok(decoded))) if decoded.is_empty()),
            "the answer is read although the deadline has passed");
    }

    #[test]
    fn a_stuck_check_is_abandoned_at_the_deadline() {
        let (_release, held) = channel::<()>();
        let worker = std::thread::spawn(move || { let _ = held.recv(); });
        let probe = LaunchProbe {
            launch: (1, 9, false, LaunchKind::Stream),
            app_name: "Zelda".into(),
            hardware: Hardware::None,
            result: channel().1,
            worker: Some(worker),
            device: media::DeviceRef::default(),
            started: Instant::now(),
        };
        let started = Instant::now();
        assert!(!probe.finish_until(started + Duration::from_millis(50)), "the wait is bounded");
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}
