//! The NVIDIA build's image enhancements before encoding: RTX HDR (an SDR
//! capture for an HDR stream) and RTX Video enhancement (NVIDIA's video
//! super resolution at the capture's own size). A build without the
//! `nvidia-proprietary` feature offers neither and passes every picture
//! through unchanged. The per-app enhancement design follows Foundation
//! Sunshine's (AlkaidLab/foundation-sunshine, GPL-3.0) DLSS NR settings.
use crate::state::{Session, Shared};
#[cfg(feature = "nvidia-proprietary")]
use butterpollo_core::nvidia_enhance::Enhancement;
use butterpollo_core::{
    config::Config,
    nvidia_enhance::{self, RTX_HDR, RTX_VIDEO, Source},
};
use butterpollo_windows::capture::{GpuImage, Pixel};
use std::sync::{Arc, atomic::Ordering};

/// The session warning naming an enhancement that was asked for and does
/// not run.
const WARNING: &str = "nvidia_enhancement";

/// Asks the GPU and driver which enhancements they offer, once at startup.
/// NVIDIA's NGX runs in a process of its own (`--nvidia-probe`): a crash or
/// hang inside it costs the enhancements, never the host.
#[cfg(feature = "nvidia-proprietary")]
pub(crate) fn probe(h: &Shared) {
    let started = std::time::Instant::now();
    let (offered, errors) = probe_child(&h.directory);
    h.nvidia_enhancements.store(offered, Ordering::Release);
    for (feature, error) in &errors {
        tracing::warn!(feature, error = %error, "NVIDIA image enhancement unavailable");
    }
    tracing::info!(
        rtx_hdr = offered & RTX_HDR != 0,
        rtx_video = offered & RTX_VIDEO != 0,
        elapsed_ms = started.elapsed().as_millis() as u64,
        "NVIDIA image enhancements checked"
    );
    *h.nvidia_enhancement_errors.lock().unwrap() = errors;
}

/// The `--nvidia-probe` process: NGX's check, printed as one JSON line. A
/// crash in it leaves a report in `profile`'s crashes folder.
#[cfg(feature = "nvidia-proprietary")]
pub(crate) fn probe_main(profile: &std::path::Path) -> i32 {
    // Without crash reports the check still runs; its exit code tells a crash.
    let _ = butterpollo_windows::crash::initialize(profile);
    let (offered, errors) = butterpollo_windows::nvidia_enhance::probe();
    let errors: serde_json::Map<String, serde_json::Value> = errors
        .into_iter()
        .map(|(feature, error)| (feature.to_owned(), error.into()))
        .collect();
    println!(
        "{}",
        serde_json::json!({"offered": offered, "errors": errors})
    );
    0
}

/// Runs NVIDIA's check in its own process, as this host's user.
#[cfg(feature = "nvidia-proprietary")]
fn probe_child(profile: &std::path::Path) -> nvidia_enhance::Offer {
    use nvidia_enhance::{PROBE_TIMEOUT_SECS, ProbeEnd, probe_failed, read_probe};
    use std::{io::BufRead, os::windows::process::CommandExt, process::Stdio, time::Instant};
    let spawned = std::env::current_exe().and_then(|program| {
        std::process::Command::new(program)
            .arg("--nvidia-probe")
            .arg(profile)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .spawn()
    });
    let mut child = match spawned {
        Ok(child) => child,
        Err(error) => return probe_failed(format!("NVIDIA's check could not start: {error}")),
    };
    // The answer is one line. Read on its own thread: the crash reporter the
    // check starts holds the pipe open after the check exits.
    let (sender, lines) = std::sync::mpsc::channel();
    if let Some(stdout) = child.stdout.take() {
        std::thread::spawn(move || {
            if let Some(Ok(line)) = std::io::BufReader::new(stdout).lines().next() {
                let _ = sender.send(line);
            }
        });
    }
    let deadline = Instant::now() + std::time::Duration::from_secs(PROBE_TIMEOUT_SECS);
    let ended = loop {
        match child.try_wait() {
            Ok(Some(status)) => break ProbeEnd::Code(status.code().unwrap_or(-1)),
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(50))
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                break ProbeEnd::TimedOut;
            }
            Err(error) => {
                return probe_failed(format!("NVIDIA's check could not be waited for: {error}"));
            }
        }
    };
    let line = lines.recv_timeout(std::time::Duration::from_secs(2)).ok();
    read_probe(line.as_deref(), ended)
}

/// The console's view: whether this is the NVIDIA build, what it offers and
/// why the rest is missing.
pub(crate) fn status(h: &Shared) -> serde_json::Value {
    let offered = h.nvidia_enhancements.load(Ordering::Acquire);
    let errors: serde_json::Map<String, serde_json::Value> = h
        .nvidia_enhancement_errors
        .lock()
        .unwrap()
        .iter()
        .map(|(feature, error)| ((*feature).to_owned(), error.as_str().into()))
        .collect();
    serde_json::json!({
        "build": cfg!(feature = "nvidia-proprietary"),
        "rtx_hdr": offered & RTX_HDR != 0,
        "rtx_video": offered & RTX_VIDEO != 0,
        "errors": errors,
    })
}

/// What a capture delivers, for the enhancement choice.
pub(crate) fn source(pixel: Pixel) -> Source {
    match pixel {
        Pixel::Bgra8 => Source::Sdr,
        Pixel::RgbaF16 => Source::ScRgb,
        Pixel::Rgba10Pq => Source::Pq10,
    }
}

/// A stream's enhancement: chooses it for each picture, keeps its NGX
/// filter, and reports when the encoder must be rebuilt because the picture
/// it receives changed kind (format, or an unshared filter output instead of
/// the capture's texture).
#[derive(Default)]
pub(crate) struct Enhancer {
    #[cfg(feature = "nvidia-proprietary")]
    inner: active::Inner,
}

impl Enhancer {
    /// The picture to encode for `image`, and whether the encoder must be
    /// rebuilt for it. A failing enhancement is turned off for this capture
    /// with a session warning, and the original picture is encoded.
    #[cfg_attr(not(feature = "nvidia-proprietary"), allow(unused_variables))]
    pub(crate) fn apply(
        &mut self,
        s: &Session,
        config: &Config,
        image: Arc<GpuImage>,
    ) -> (Arc<GpuImage>, bool) {
        let source = source(image.pixel);
        let offered = nvidia_enhance::offered(config);
        match nvidia_enhance::unavailable(config, offered, source) {
            Some(reason) => s.launch.warnings.set(WARNING, reason),
            None => s.launch.warnings.clear(WARNING),
        }
        #[cfg(feature = "nvidia-proprietary")]
        {
            let wanted = nvidia_enhance::select(config, offered, s.config.hdr, source);
            self.inner.apply(s, config, wanted, image)
        }
        #[cfg(not(feature = "nvidia-proprietary"))]
        (image, false)
    }

    /// The encoder's SDR white and linear scale while RTX HDR converts the
    /// picture; `None` leaves the encoder's own values.
    #[cfg_attr(not(feature = "nvidia-proprietary"), allow(unused_variables))]
    pub(crate) fn luminance(&self, config: &Config) -> Option<(f32, f32)> {
        #[cfg(feature = "nvidia-proprietary")]
        {
            (self.inner.selected == Enhancement::RtxHdr)
                .then(|| nvidia_enhance::rtx_luminance(config))
        }
        #[cfg(not(feature = "nvidia-proprietary"))]
        None
    }

    /// RTX HDR's values from the NVIDIA driver profile of the game in front,
    /// where the app or device did not choose them (polled every 250 ms
    /// while RTX HDR runs). Returns the stream's new runtime configuration.
    #[cfg_attr(not(feature = "nvidia-proprietary"), allow(unused_variables))]
    pub(crate) fn tune(&mut self, h: &Shared, image: &GpuImage, base: &Config) -> Option<Config> {
        #[cfg(feature = "nvidia-proprietary")]
        {
            self.inner.tune(h, image, base)
        }
        #[cfg(not(feature = "nvidia-proprietary"))]
        None
    }

    /// Drops the filters: the capture is recreated, possibly on a new device.
    pub(crate) fn reset(&mut self) {
        #[cfg(feature = "nvidia-proprietary")]
        {
            self.inner = active::Inner::default();
        }
    }
}

#[cfg(feature = "nvidia-proprietary")]
mod active {
    use super::*;
    use anyhow::Context as _;
    use butterpollo_windows::{rtx_profiles::Profiles, rtx_video, truehdr};
    use std::{
        sync::Weak,
        time::{Duration, Instant},
    };

    #[derive(Default)]
    pub(super) struct Inner {
        pub(super) selected: Enhancement,
        rtx_hdr: Option<truehdr::Filter>,
        rtx_video: Option<rtx_video::Filter>,
        /// An enhancement that failed on this capture: not retried each frame.
        failed: Option<Enhancement>,
        /// The capture device the filters were made on.
        device: usize,
        /// The last source picture and its enhanced copy: a repeated picture
        /// is encoded again without a second conversion, and the stream's
        /// freshness check sees the same picture.
        last: Option<(Weak<GpuImage>, Arc<GpuImage>)>,
        profiles: Option<Profiles>,
        foreground: Option<butterpollo_windows::foreground::Tracker>,
        profile_due: Option<Instant>,
    }

    impl Inner {
        pub(super) fn apply(
            &mut self,
            s: &Session,
            config: &Config,
            wanted: Enhancement,
            image: Arc<GpuImage>,
        ) -> (Arc<GpuImage>, bool) {
            let device = image.gpu.key();
            if device != self.device {
                *self = Self {
                    selected: self.selected,
                    profiles: self.profiles.take(),
                    foreground: self.foreground.take(),
                    device,
                    ..Self::default()
                };
            }
            let wanted = if self.failed == Some(wanted) {
                Enhancement::None
            } else {
                wanted
            };
            let changed = wanted != self.selected;
            if changed {
                tracing::info!(client = %s.launch.client.name, from = ?self.selected, to = ?wanted, "image enhancement changed; rebuilding the encoder");
                self.selected = wanted;
                self.last = None;
                if wanted != Enhancement::RtxHdr {
                    self.rtx_hdr = None;
                }
                if wanted != Enhancement::RtxVideo {
                    self.rtx_video = None;
                }
            }
            if wanted == Enhancement::None {
                return (image, changed);
            }
            if let Some((source, enhanced)) = &self.last
                && std::ptr::eq(source.as_ptr(), Arc::as_ptr(&image))
            {
                return (enhanced.clone(), changed);
            }
            match self.convert(config, wanted, &image) {
                Ok(enhanced) => {
                    let enhanced = Arc::new(enhanced);
                    self.last = Some((Arc::downgrade(&image), enhanced.clone()));
                    (enhanced, changed)
                }
                Err(error) => {
                    let name = match wanted {
                        Enhancement::RtxHdr => "RTX HDR",
                        _ => "RTX Video enhancement",
                    };
                    tracing::warn!(client = %s.launch.client.name, error = %format!("{error:#}"), "{name} failed; streaming the picture without it");
                    s.launch.warnings.set(
                        "nvidia_enhancement_failed",
                        format!("{name} failed ({error:#}) and is off until the capture restarts; the stream continues without it. Update the NVIDIA driver, or turn {name} off for this app."),
                    );
                    self.failed = Some(wanted);
                    self.selected = Enhancement::None;
                    self.rtx_hdr = None;
                    self.rtx_video = None;
                    self.last = None;
                    // The encoder was made for the enhanced picture.
                    (image, true)
                }
            }
        }

        fn convert(
            &mut self,
            config: &Config,
            wanted: Enhancement,
            image: &GpuImage,
        ) -> anyhow::Result<GpuImage> {
            match wanted {
                Enhancement::RtxHdr => {
                    let parameters = nvidia_enhance::rtx_parameters(config);
                    let filter = match &mut self.rtx_hdr {
                        Some(filter) => filter,
                        none => none.insert(truehdr::Filter::new_gpu(image, parameters)?),
                    };
                    filter.set_parameters(parameters);
                    filter.apply_gpu(image)
                }
                Enhancement::RtxVideo => {
                    let quality = nvidia_enhance::rtx_video_quality(config)
                        .context("RTX Video enhancement has no strength")?;
                    let filter = match &mut self.rtx_video {
                        Some(filter) => filter,
                        none => none.insert(rtx_video::Filter::new_gpu(image, quality)?),
                    };
                    filter.set_quality(quality);
                    filter.apply_gpu(image)
                }
                Enhancement::None => anyhow::bail!("no enhancement"),
            }
        }

        pub(super) fn tune(
            &mut self,
            h: &Shared,
            image: &GpuImage,
            base: &Config,
        ) -> Option<Config> {
            if self.selected != Enhancement::RtxHdr {
                self.profile_due = None;
                return None;
            }
            let now = Instant::now();
            if self.profile_due.is_some_and(|due| now < due) {
                return None;
            }
            self.profile_due = Some(now + Duration::from_millis(250));
            let (running, owned) = {
                let app = h.current_app.lock().unwrap();
                (
                    app.is_some(),
                    app.as_ref()
                        .and_then(|app| app.child.as_ref())
                        .and_then(|child| child.process_ids().ok())
                        .unwrap_or_default(),
                )
            };
            let visible = running
                .then(|| {
                    self.foreground
                        .get_or_insert_with(butterpollo_windows::foreground::Tracker::default)
                        .poll(&owned, &image.gpu.display)
                })
                .flatten();
            let profiles = self.profiles.get_or_insert_with(Profiles::new);
            Some(nvidia_enhance::resolve(
                base,
                visible.is_some(),
                profiles.poll(visible.as_deref()),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_map_to_their_sources() {
        assert_eq!(source(Pixel::Bgra8), Source::Sdr);
        assert_eq!(source(Pixel::RgbaF16), Source::ScRgb);
        assert_eq!(source(Pixel::Rgba10Pq), Source::Pq10);
    }

    #[test]
    fn a_build_without_the_enhancements_reports_nothing_offered() {
        let f = crate::state::test_support::Fixture::new();
        let h = &f.host;
        h.nvidia_enhancement_errors
            .lock()
            .unwrap()
            .push(("rtx_video", "the GPU does not support it".into()));
        let status = status(h);
        assert_eq!(status["build"], cfg!(feature = "nvidia-proprietary"));
        assert_eq!(status["rtx_hdr"], false);
        assert_eq!(status["rtx_video"], false);
        assert_eq!(status["errors"]["rtx_video"], "the GPU does not support it");
        h.nvidia_enhancements.store(RTX_VIDEO, Ordering::Release);
        assert!(status_of(h, "rtx_video"));
    }

    fn status_of(h: &Shared, key: &str) -> bool {
        status(h)[key].as_bool().unwrap()
    }
}
