//! Frame Warp on this host (docs/frame-warp.md). The video thread runs the GPU passes over
//! each new capture after its frame went to the encoder and hands their readbacks to a
//! worker, which measures and fits them off the frame path and outside the host's lock
//! ([`butterpollo_session::session::FrameWarpState`]). What a game taught the host is kept for
//! its next session in `frame_warp.json` ([`super::frame_warp_prior`]). Nothing runs until a
//! client asks.
//!
//! Frame Warp 2 adds no GPU pass: its camera confidence, scene cuts and HUD steadiness come
//! from the thumbnails and HUD masks these passes already read back, measured on the CPU
//! (`butterpollo_media_policy::frame_warp::confidence`), so there is no
//! `compute_frame_warp_v2.rs` and no second pass over the capture. Whether the host's pointer
//! shows comes with each capture ([`GpuImage::pointer`], Desktop Duplication only).

use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::time::{Duration, Instant};

use butterpollo_media_policy::frame_warp::{Mask, Measurer, Model, Thumbnail};
use butterpollo_win_graphics::capture::GpuImage;
use butterpollo_win_graphics::compute::frame_warp::{FrameWarpGpu, Hud, Pass, hud_grid};

use super::frame_warp_prior::Store;
use crate::state::{Launch, Session, Shared};

/// How often the HUD mask is read back while the camera turns.
const MASK_EVERY: Duration = Duration::from_millis(250);
/// Readbacks waiting for the worker; past it they are dropped (a thumbnail costs its sample).
const QUEUE: usize = 8;

/// The video thread's part: the stream's size, the GPU passes and their readbacks.
#[derive(Default)]
pub(crate) struct VideoFrameWarp {
    passes: Option<FrameWarpGpu>,
    /// Identity (`Device::key`) of the graphics device the passes last failed on; tried again only on another.
    failed_on: Option<usize>,
    worker: Option<Worker>,
    /// The worker could not start: no Frame Warp for the rest of this video thread.
    stopped: bool,
    /// The stream's picture and the capture's size in it, as last given to the host.
    picture: Option<((u32, u32), (u32, u32))>,
    mask_due: Option<Instant>,
}

impl VideoFrameWarp {
    /// After a capture's frame went out: notes the stream's size, hands finished readbacks to
    /// the worker and, for a new capture, submits the passes over it. While no client asked,
    /// only the size is compared.
    pub(crate) fn frame(
        &mut self,
        h: &Shared,
        s: &Arc<Session>,
        image: &GpuImage,
        fresh: bool,
        picture: (u32, u32),
    ) {
        let capture = (image.width, image.height);
        if self.picture != Some((picture, capture)) {
            self.picture = Some((picture, capture));
            // A capture of another shape is letterboxed into the stream, as the converter does.
            let (x, y, w, h) =
                butterpollo_media_policy::display_policy::letterbox(capture, picture);
            s.frame_warp.set_picture(
                (edge(picture.0), edge(picture.1)),
                (edge(x), edge(y), edge(w), edge(h)),
            );
        }
        if !s.frame_warp.enabled() || self.stopped {
            // Turned off: the passes' buffers go; the worker stays to save at the end.
            self.passes = None;
            return;
        }
        if self.worker.is_none() {
            match Worker::spawn(h, s) {
                Ok(worker) => self.worker = Some(worker),
                Err(error) => {
                    tracing::warn!(
                        client = %s.launch.client.name,
                        %error,
                        "frame warp could not start its worker"
                    );
                    self.stopped = true;
                    return;
                }
            }
        }
        match self.passes.as_mut().map(FrameWarpGpu::poll) {
            Some(Ok((thumbnails, mask))) => {
                if let Some(worker) = &self.worker {
                    for thumbnail in thumbnails {
                        worker.send(Work::Thumbnail(Thumbnail {
                            width: thumbnail.width,
                            height: thumbnail.height,
                            luma: thumbnail.luma,
                            at_us: thumbnail.at,
                        }));
                    }
                    if let Some(mask) = mask {
                        worker.send(Work::Hud(Mask {
                            generation: 0,
                            width: edge(mask.width),
                            height: edge(mask.height),
                            words: mask.words,
                        }));
                    }
                }
            }
            Some(Err(error)) => {
                self.fail(s, image, &error);
                return;
            }
            None => {}
        }
        if !fresh {
            return;
        }
        // A pointer on screen is a menu or a pause, not the camera turning.
        s.frame_warp.on_cursor(image.pointer().is_some());
        if self
            .passes
            .as_ref()
            .is_some_and(|passes| !passes.accepts(image))
        {
            self.passes = None;
        }
        if self.passes.is_none() {
            if self.failed_on == Some(image.gpu.key()) {
                return;
            }
            match FrameWarpGpu::for_image(image) {
                Ok(passes) => {
                    self.passes = Some(passes);
                    self.failed_on = None;
                }
                Err(error) => {
                    self.fail(s, image, &error);
                    return;
                }
            }
        }
        let Some(passes) = self.passes.as_mut() else {
            return;
        };
        let params = s.frame_warp.params();
        let now = Instant::now();
        let grid = params.hud_every_us.and_then(|every_us| {
            Some((
                hud_grid(params.mask_width, (image.width, image.height))?,
                every_us,
            ))
        });
        let pass = Pass {
            thumbnail: (
                u32::try_from(params.thumbnail.0).unwrap_or(u32::MAX),
                u32::try_from(params.thumbnail.1).unwrap_or(u32::MAX),
            ),
            hud: grid.map(|(grid, every_us)| Hud {
                grid,
                read: self.mask_due.is_none_or(|due| now >= due),
                every_us,
            }),
        };
        match passes.submit(image, &pass, s.frame_warp.us(image.captured)) {
            Ok(Some(true)) => self.mask_due = Some(now + MASK_EVERY),
            Ok(_) => {}
            Err(error) => self.fail(s, image, &error),
        }
    }

    /// Releases GPU passes on capture generation reset so no dead
    /// `ID3D11Device` or capture texture survives `release_generation`,
    /// while preserving `failed_on` and the off-thread learning worker.
    pub(crate) fn reset(&mut self) {
        self.passes = None;
        self.mask_due = None;
    }

    /// The passes stop on this graphics device, loudly; the stream goes on without them.
    fn fail(&mut self, s: &Session, image: &GpuImage, error: &anyhow::Error) {
        tracing::warn!(
            client = %s.launch.client.name,
            error = %format!("{error:#}"),
            "frame warp stopped measuring the picture on this graphics device"
        );
        self.passes = None;
        self.failed_on = Some(image.gpu.key());
    }
}

fn edge(value: u32) -> u16 {
    u16::try_from(value).unwrap_or(u16::MAX)
}

enum Work {
    Thumbnail(Thumbnail),
    Hud(Mask),
}

/// Measures and fits off the frame path, one thread per stream that uses Frame Warp.
struct Worker {
    sender: Option<SyncSender<Work>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Worker {
    fn spawn(h: &Shared, s: &Arc<Session>) -> std::io::Result<Self> {
        let (sender, receiver) = mpsc::sync_channel(QUEUE);
        let store = Store::new(&h.directory, game_key(h, &s.launch));
        let s = s.clone();
        let thread = std::thread::Builder::new()
            .name("frame-warp".into())
            .spawn(move || work(&s, &store, &receiver))?;
        Ok(Self {
            sender: Some(sender),
            thread: Some(thread),
        })
    }

    fn send(&self, work: Work) {
        if let Some(sender) = &self.sender {
            let _ = sender.try_send(work);
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        // Closing the channel ends the worker, which saves what it learned.
        self.sender = None;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn work(s: &Session, store: &Store, receiver: &Receiver<Work>) {
    if s.frame_warp.claim_prior()
        && let Some(prior) = store.load()
    {
        s.frame_warp.with(|host| host.set_prior_v2(prior));
        tracing::info!(
            client = %s.launch.client.name,
            "frame warp starts from what it learned for this game before"
        );
    }
    // The previous thumbnail and the HUD's pixels: measured against here, outside the host's
    // lock, which the control thread takes for the input and the tags.
    let mut measurer = Measurer::default();
    let mut locked = false;
    let mut saved = false;
    let mut generation = None;
    while let Ok(item) = receiver.recv() {
        let now_us = s.frame_warp.now_us();
        let learned = match item {
            Work::Thumbnail(thumbnail) => {
                s.frame_warp.on_thumbnail(&mut measurer, thumbnail, now_us)
            }
            Work::Hud(mask) => s.frame_warp.on_hud(&mut measurer, mask, now_us),
        };
        if let Some(sent) = learned.model
            && generation.replace(sent.generation) != Some(sent.generation)
        {
            tracing::debug!(
                client = %s.launch.client.name,
                generation = sent.generation,
                mouse_gain = ?sent.mouse_gain,
                stick_gain = ?sent.stick_gain,
                lag_ms = sent.lag_us as f32 / 1000.0,
                fit = sent.confidence,
                field_of_view = field_of_view(&sent),
                "frame warp model sent"
            );
        }
        let (model, prior) = (learned.model.filter(Model::locked), learned.prior);
        // A player who aims only by the gyro locks the stream too.
        let now_locked = model.is_some() || learned.gyro_locked;
        if now_locked == locked {
            continue;
        }
        locked = now_locked;
        log_model(s, model);
        if locked
            && !saved
            && let Some(prior) = prior
        {
            store.save(prior, true);
            saved = true;
        }
    }
    if let Some(prior) = s.frame_warp.with(|host| host.prior_v2()) {
        store.save(prior, locked);
    }
}

fn log_model(s: &Session, model: Option<Model>) {
    match model {
        Some(model) => tracing::info!(
            client = %s.launch.client.name,
            mouse = model.mouse_locked,
            stick = model.stick_locked,
            mouse_gain = ?model.mouse_gain,
            stick_gain = ?model.stick_gain,
            lag_ms = model.lag_us as f32 / 1000.0,
            fit = model.confidence,
            field_of_view = field_of_view(&model),
            "frame warp model locked"
        ),
        None => tracing::info!(client = %s.launch.client.name, "frame warp model unlocked"),
    }
}

/// The horizontal field of view of the model's lens in degrees, for the log; 0 for a picture
/// that moves alike everywhere.
fn field_of_view(model: &Model) -> f32 {
    model.lens().map_or(0.0, |focal| {
        2.0 * (f32::from(model.picture_width) * 0.5 / focal)
            .atan()
            .to_degrees()
    })
}

/// The game a launch streams, as `frame_warp.json` names it: its uuid, else its name.
fn game_key(h: &Shared, launch: &Launch) -> Option<String> {
    let app = crate::stream::launch_app(h, launch)?;
    match app
        .extra
        .get("uuid")
        .and_then(serde_json::Value::as_str)
        .filter(|uuid| !uuid.is_empty())
    {
        Some(uuid) => Some(format!("uuid:{uuid}")),
        None => (!app.name.is_empty()).then(|| format!("name:{}", app.name)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reset_clears_gpu_state_and_mask_timer_while_keeping_session_flags() {
        let mut warp = VideoFrameWarp {
            failed_on: Some(0x1234),
            mask_due: Some(Instant::now()),
            picture: Some(((1920, 1080), (1920, 1080))),
            stopped: true,
            ..VideoFrameWarp::default()
        };
        warp.reset();
        assert!(warp.passes.is_none());
        assert_eq!(warp.failed_on, Some(0x1234));
        assert!(warp.mask_due.is_none());
        assert!(warp.stopped);
        assert_eq!(warp.picture, Some(((1920, 1080), (1920, 1080))));
    }
}
