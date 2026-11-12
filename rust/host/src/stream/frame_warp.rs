//! Frame Warp on this host (docs/frame-warp.md). The video thread runs the GPU passes over
//! each new capture after its frame went to the encoder and hands their readbacks to a
//! worker, which measures and fits them off the frame path and outside the host's lock
//! ([`butterpollo_core::session::FrameWarpState`]). What a game taught the host is kept for
//! its next session in `frame_warp.json`. Nothing runs until a client asks.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use butterpollo_core::frame_warp::{Mask, Measurer, Model, Prior, Thumbnail};
use butterpollo_windows::capture::GpuImage;
use butterpollo_windows::compute::frame_warp::{FrameWarpGpu, Hud, Pass, hud_grid};
use serde::{Deserialize, Serialize};

use crate::state::{Launch, Session, Shared};

/// How often the HUD mask is read back while the camera turns.
const MASK_EVERY: Duration = Duration::from_millis(250);
/// Readbacks waiting for the worker; past it they are dropped (a thumbnail costs its sample).
const QUEUE: usize = 8;
/// Games whose model is kept; the least recently used goes first.
const GAMES: usize = 256;
/// In the configuration directory.
const FILE: &str = "frame_warp.json";

/// The video thread's part: the stream's size, the GPU passes and their readbacks.
#[derive(Default)]
pub(crate) struct VideoFrameWarp {
    passes: Option<FrameWarpGpu>,
    /// The graphics device the passes last failed on; tried again only on another.
    failed_on: Option<butterpollo_windows::capture::Device>,
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
            let (x, y, w, h) = butterpollo_core::display_policy::letterbox(capture, picture);
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
        if self
            .passes
            .as_ref()
            .is_some_and(|passes| !passes.accepts(image))
        {
            self.passes = None;
        }
        if self.passes.is_none() {
            if self
                .failed_on
                .as_ref()
                .is_some_and(|device| device.device == image.gpu.device)
            {
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

    /// The passes stop on this graphics device, loudly; the stream goes on without them.
    fn fail(&mut self, s: &Session, image: &GpuImage, error: &anyhow::Error) {
        tracing::warn!(
            client = %s.launch.client.name,
            error = %format!("{error:#}"),
            "frame warp stopped measuring the picture on this graphics device"
        );
        self.passes = None;
        self.failed_on = Some(image.gpu.clone());
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
        let store = Store {
            path: h.directory.join(FILE),
            key: game_key(h, &s.launch),
        };
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
        s.frame_warp.with(|host| host.set_prior(prior));
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
                "frame warp model sent"
            );
        }
        let (model, prior) = (learned.model.filter(Model::locked), learned.prior);
        if model.is_some() == locked {
            continue;
        }
        locked = model.is_some();
        log_model(s, model);
        if locked
            && !saved
            && let Some(prior) = prior
        {
            store.save(prior);
            saved = true;
        }
    }
    if let Some(prior) = s.frame_warp.with(|host| host.prior()) {
        store.save(prior);
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
            "frame warp model locked"
        ),
        None => tracing::info!(client = %s.launch.client.name, "frame warp model unlocked"),
    }
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

/// `frame_warp.json`: what each game's streams learned, by [`game_key`].
#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
struct Remembered {
    #[serde(default)]
    games: BTreeMap<String, Game>,
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
struct Game {
    prior: Prior,
    /// Unix seconds of the last save.
    used: u64,
}

impl Remembered {
    /// Keeps `prior` for `key`, dropping the least recently used games past [`GAMES`].
    fn remember(&mut self, key: &str, prior: Prior, now: u64) {
        self.games.insert(key.to_owned(), Game { prior, used: now });
        while self.games.len() > GAMES {
            let Some(oldest) = self
                .games
                .iter()
                .min_by_key(|(_, game)| game.used)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            self.games.remove(&oldest);
        }
    }
}

/// One file for every stream: saves take turns.
static SAVES: Mutex<()> = Mutex::new(());

struct Store {
    path: PathBuf,
    key: Option<String>,
}

impl Store {
    fn load(&self) -> Option<Prior> {
        let key = self.key.as_ref()?;
        read(&self.path).games.get(key).map(|game| game.prior)
    }

    fn save(&self, prior: Prior) {
        let Some(key) = &self.key else {
            return;
        };
        let _turn = SAVES.lock().unwrap_or_else(PoisonError::into_inner);
        let mut remembered = read(&self.path);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_secs());
        remembered.remember(key, prior, now);
        if let Err(error) = butterpollo_core::state::write_json(&self.path, &remembered) {
            tracing::warn!(
                path = %self.path.display(),
                error = %format!("{error:#}"),
                "frame warp could not save what it learned"
            );
        }
    }
}

/// The file, or nothing learned when it is missing or unreadable.
fn read(path: &Path) -> Remembered {
    let Ok(bytes) = std::fs::read(path) else {
        return Remembered::default();
    };
    serde_json::from_slice(&bytes).unwrap_or_else(|error| {
        tracing::warn!(path = %path.display(), %error, "frame warp ignores an unreadable file");
        Remembered::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prior(gain: f32) -> Prior {
        Prior {
            mouse_gain: Some([gain, gain]),
            mouse_lag_us: Some(32_000),
            picture_width: 1920,
            picture_height: 1080,
            ..Prior::default()
        }
    }

    #[test]
    fn the_least_recently_used_game_goes_first() {
        let mut remembered = Remembered::default();
        for n in 0..GAMES as u64 {
            remembered.remember(&format!("uuid:{n}"), prior(1.0), 1_000 + n);
        }
        // Game 0 is used again, so game 1 is the oldest when one more comes.
        remembered.remember("uuid:0", prior(2.0), 5_000);
        remembered.remember("name:Another", prior(3.0), 5_001);
        assert_eq!(remembered.games.len(), GAMES);
        assert!(!remembered.games.contains_key("uuid:1"));
        assert_eq!(remembered.games["uuid:0"].prior, prior(2.0));
        assert_eq!(remembered.games["name:Another"].used, 5_001);
    }

    #[test]
    fn a_saved_prior_loads_for_the_same_game_only() {
        let directory = std::env::temp_dir().join(format!("frame-warp-{}", uuid::Uuid::new_v4()));
        let path = directory.join(FILE);
        let store = |key: Option<&str>| Store {
            path: path.clone(),
            key: key.map(str::to_owned),
        };
        assert_eq!(store(Some("uuid:a")).load(), None);
        store(Some("uuid:a")).save(prior(1.5));
        store(None).save(prior(9.0));
        assert_eq!(store(Some("uuid:a")).load(), Some(prior(1.5)));
        assert_eq!(store(Some("uuid:b")).load(), None);
        assert_eq!(store(None).load(), None);
        let file: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(file["games"].as_object().unwrap().len(), 1);
        // A damaged file reads as nothing learned and is replaced by the next save.
        std::fs::write(&path, b"{not json").unwrap();
        assert_eq!(store(Some("uuid:a")).load(), None);
        store(Some("uuid:b")).save(prior(2.5));
        assert_eq!(store(Some("uuid:b")).load(), Some(prior(2.5)));
        let _ = std::fs::remove_dir_all(&directory);
    }
}
