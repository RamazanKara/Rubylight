//! A stream's Frame Warp ([`FrameWarpHost`]) where no hot path waits for it.
//!
//! The control thread decodes input and the packetizer assigns frame numbers;
//! neither may wait. The frame-warp worker measures each thumbnail and runs
//! each fit outside the host's lock ([`Measurer`], [`FitJob`]), holding the
//! host only to hand over the results, so holds are short. Still, every event
//! goes into a short inbox, and whoever gets the host next (`try_lock`)
//! applies the inbox in order; a holder applies what came in meanwhile before
//! it lets go. A frame's tag only goes into the inbox: the control loop, woken
//! for it, makes the tags message off the frame path. What the host produces
//! goes into an outbox the control loop takes from, and the control loop is
//! woken unless a wake is already on its way.
//!
//! The host runs Frame Warp 2 only: a request that is not version 2 (an older
//! Nightjar's) changes nothing and gets nothing back. The gyro's motion packets
//! and the pointer's visibility reach the host the same way ([`v2`]), and every
//! tagged frame's meta (0x5549) goes out with its tags.
//!
//! [`FitJob`]: butterpollo_media_policy::frame_warp::FitJob
//!
//! Times are microseconds on the session's clock: since `Session::started`.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, TryLockError};
use std::time::Instant;

use butterpollo_media_policy::frame_warp::{
    FrameWarpHost, Learned, META_BYTES, MODEL_BYTES, Mask, Measurer, RequestV2, Status, Thumbnail,
    hud_pixels, shape,
};

mod v2;

/// Wakes the control loop: something is waiting to be sent.
pub type Wake = Arc<dyn Fn() + Send + Sync>;

/// Events kept for the host at most; past it the oldest go (the history loses them).
const INBOX: usize = 16_384;
/// Tag messages kept for the control loop; older ones go (the next carries their tags).
const OUTBOX_TAGS: usize = 8;
/// Metas kept for the control loop; older ones go (unreliable anyway).
const OUTBOX_METAS: usize = 8;
/// No capture time yet.
const NONE: u64 = u64::MAX;

enum Event {
    Request(Vec<u8>),
    Mouse {
        dx: i16,
        dy: i16,
        at_us: u64,
    },
    Controller {
        number: u16,
        stick: (i16, i16),
        at_us: u64,
    },
    Tag {
        frame: u32,
        capture_us: u64,
    },
    Picture((u16, u16), (u16, u16, u16, u16)),
    /// A gyro reading of the pad whose gyro turns the camera.
    Motion {
        number: u8,
        kind: u8,
        xyz: [f32; 3],
        at_us: u64,
    },
    /// The host's pointer showed or went.
    Cursor(bool),
}

#[derive(Default)]
struct Outbox {
    model: Option<[u8; MODEL_BYTES]>,
    mask: Option<Vec<u8>>,
    tags: VecDeque<Vec<u8>>,
    metas: VecDeque<[u8; META_BYTES]>,
}

/// What the video thread asks of the host every capture, as the host last said.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Params {
    /// [`FrameWarpHost::thumbnail_size`].
    pub thumbnail: (usize, usize),
    /// [`FrameWarpHost::mask_width`].
    pub mask_width: u16,
    /// [`FrameWarpHost::hud_every_us`] when the host was last held: `None` while the camera
    /// stands still.
    pub hud_every_us: Option<u64>,
}

/// What the control loop sends, in this order: the model and mask reliably,
/// then the tags and the frames' metas unreliably.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Outgoing {
    pub model: Option<[u8; MODEL_BYTES]>,
    pub mask: Option<Vec<u8>>,
    pub tags: Vec<Vec<u8>>,
    pub metas: Vec<[u8; META_BYTES]>,
}
impl Outgoing {
    pub fn is_empty(&self) -> bool {
        self.model.is_none() && self.mask.is_none() && self.tags.is_empty() && self.metas.is_empty()
    }
}

pub struct FrameWarpState {
    epoch: Instant,
    /// Mirrors the last request, so the control thread and the video paths
    /// skip all work while no client asked.
    on: AtomicBool,
    host: Mutex<FrameWarpHost>,
    inbox: Mutex<VecDeque<Event>>,
    outbox: Mutex<Outbox>,
    params: Mutex<Params>,
    /// The newest capture time a frame was tagged with: a repeated frame shows it.
    last_capture_us: AtomicU64,
    /// What an earlier session learned for the game was looked up.
    prior_claimed: AtomicBool,
    /// The pad whose gyro turns the camera, as the last request said ([`v2::NO_PAD`]).
    gyro_pad: AtomicU8,
    /// Whether the host's pointer showed when last told ([`v2::CURSOR_UNKNOWN`]).
    cursor: AtomicU8,
    /// A request that is not version 2 was ignored (logged once a stream).
    ignored: AtomicBool,
    wake: Mutex<Option<Wake>>,
    /// A wake is on its way and the control loop has not taken the outbox since: another
    /// wake would only cost a syscall.
    woken: AtomicBool,
}

impl std::fmt::Debug for FrameWarpState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FrameWarpState")
            .field("on", &self.enabled())
            .finish_non_exhaustive()
    }
}

impl FrameWarpState {
    /// Frame Warp for a stream of `picture` pixels whose clock starts at `epoch`; off until a
    /// client asks.
    pub fn new(picture: (u16, u16), epoch: Instant) -> Self {
        let host = FrameWarpHost::new(picture);
        let params = Params {
            thumbnail: host.thumbnail_size(),
            mask_width: 0,
            hud_every_us: None,
        };
        Self {
            epoch,
            on: AtomicBool::new(false),
            host: Mutex::new(host),
            inbox: Mutex::new(VecDeque::new()),
            outbox: Mutex::new(Outbox::default()),
            params: Mutex::new(params),
            last_capture_us: AtomicU64::new(NONE),
            prior_claimed: AtomicBool::new(false),
            gyro_pad: AtomicU8::new(v2::NO_PAD),
            cursor: AtomicU8::new(v2::CURSOR_UNKNOWN),
            ignored: AtomicBool::new(false),
            wake: Mutex::new(None),
            woken: AtomicBool::new(false),
        }
    }

    /// `at` on the session's clock.
    pub fn us(&self, at: Instant) -> u64 {
        at.saturating_duration_since(self.epoch)
            .as_micros()
            .min(u128::from(u64::MAX - 1)) as u64
    }

    pub fn now_us(&self) -> u64 {
        self.us(Instant::now())
    }

    /// Whether a client asked for Frame Warp and has not turned it off.
    pub fn enabled(&self) -> bool {
        self.on.load(Ordering::Acquire)
    }

    /// What wakes the control loop once something is waiting to be sent.
    pub fn set_wake(&self, wake: Wake) {
        *lock(&self.wake) = Some(wake);
    }

    /// A client's request (0x5543). Returns whether Frame Warp is now on, or `None` for a
    /// request that is not version 2 (an older Nightjar's) or is malformed, which changes
    /// nothing ([`Self::first_ignored`]).
    pub fn on_request(&self, payload: &[u8]) -> Option<bool> {
        let full = RequestV2::decode(payload)?;
        let request = full.base;
        self.set_gyro(&full);
        self.on.store(request.enabled, Ordering::Release);
        // While off, the control loop takes nothing and so never clears a wake's mark.
        self.woken.store(false, Ordering::Release);
        if !request.enabled {
            *lock(&self.outbox) = Outbox::default();
            lock(&self.inbox).retain(|event| matches!(event, Event::Picture(..)));
        }
        self.push(Event::Request(payload.to_vec()));
        Some(request.enabled)
    }

    /// A relative mouse move the control thread decoded at `at`.
    pub fn on_mouse(&self, dx: i16, dy: i16, at: Instant) {
        if self.enabled() && (dx != 0 || dy != 0) {
            let at_us = self.us(at);
            self.push_quietly(Event::Mouse { dx, dy, at_us });
        }
    }

    /// A controller packet the control thread decoded at `at`, with its right stick.
    pub fn on_controller(&self, number: u16, stick: (i16, i16), at: Instant) {
        if self.enabled() {
            let at_us = self.us(at);
            self.push_quietly(Event::Controller {
                number,
                stick,
                at_us,
            });
        }
    }

    /// Tags video frame `frame` as its number is assigned. `captured` is the capture's
    /// present time; a repeated picture has none and shows the last capture tagged. Only
    /// queues the tag and wakes the control loop, which makes the message
    /// ([`Self::take_outgoing`]).
    pub fn tag(&self, frame: u32, captured: Option<Instant>) {
        if !self.enabled() {
            return;
        }
        let capture_us = match captured {
            Some(at) => {
                let us = self.us(at);
                self.last_capture_us.store(us, Ordering::Relaxed);
                us
            }
            None => match self.last_capture_us.load(Ordering::Relaxed) {
                NONE => self.now_us(),
                us => us,
            },
        };
        self.queue(Event::Tag { frame, capture_us });
        self.wake();
    }

    /// The stream's picture size changed (a reconfigure), or where the capture sits in it
    /// ([`FrameWarpHost::set_picture`]).
    pub fn set_picture(&self, picture: (u16, u16), content: (u16, u16, u16, u16)) {
        self.push(Event::Picture(picture, content));
    }

    /// Runs `f` on the host, waiting for it: for the frame-warp worker only, never on a
    /// path that must not wait.
    pub fn with<R>(&self, f: impl FnOnce(&mut FrameWarpHost) -> R) -> R {
        let host = match self.host.lock() {
            Ok(host) => host,
            Err(poisoned) => poisoned.into_inner(),
        };
        self.hold(host, f)
    }

    /// Runs `f` on the host unless someone holds it.
    pub fn try_with<R>(&self, f: impl FnOnce(&mut FrameWarpHost) -> R) -> Option<R> {
        let host = self.try_host()?;
        Some(self.hold(host, f))
    }

    /// Whether the caller is the first to ask: the game's prior is looked up once a stream.
    pub fn claim_prior(&self) -> bool {
        !self.prior_claimed.swap(true, Ordering::AcqRel)
    }

    /// What the video thread reads every capture, as of the last time the host was held.
    pub fn params(&self) -> Params {
        *lock(&self.params)
    }

    /// The host's statistics, waiting for it.
    pub fn status(&self) -> Status {
        self.with(|host| host.status())
    }

    /// The worker's: a capture's luma thumbnail. The motion estimate runs before the host is
    /// held and a due fit after it, on copies, so neither holds up the frame path or the
    /// input.
    pub fn on_thumbnail(
        &self,
        measurer: &mut Measurer,
        thumbnail: Thumbnail,
        now_us: u64,
    ) -> Learned {
        let measured = measurer.measure(&thumbnail);
        let (keep, resets, job, learned) = self.with(|host| {
            let keep = host.on_measured(&measured, now_us);
            (keep, host.resets(), host.fit_job(now_us), host.learned())
        });
        measurer.keep(thumbnail, keep, resets);
        let Some(job) = job else {
            return learned;
        };
        let fitted = job.run();
        self.with(|host| {
            host.publish_fit(fitted);
            host.learned()
        })
    }

    /// The worker's: the GPU's HUD bits, shaped ([`shape`]), laid over the picture
    /// ([`butterpollo_media_policy::frame_warp::Placement::place`]) and over the thumbnail outside the lock.
    pub fn on_hud(&self, measurer: &mut Measurer, mask: Mask, now_us: u64) -> Learned {
        let shaped = shape(&mask);
        let placement = self.with(|host| host.placement());
        let placed = placement.place(&shaped);
        let (size, resets, learned) = self.with(|host| {
            let size = host.on_hud(&placed, placement, now_us);
            (size, host.resets(), host.learned())
        });
        match size {
            Some(size) => measurer.set_hud(hud_pixels(&shaped, size), resets),
            None => measurer.catch_up(resets),
        }
        learned
    }

    /// Everything waiting to be sent; nothing while Frame Warp is off. Makes the tags of the
    /// frames queued since, unless the worker holds the host (it makes them and wakes the
    /// control loop again).
    pub fn take_outgoing(&self) -> Outgoing {
        // From here on, whatever is produced wakes the control loop again.
        self.woken.store(false, Ordering::Release);
        let queued = !lock(&self.inbox).is_empty();
        if queued
            && self.enabled()
            && let Some(host) = self.try_host()
        {
            // Taken just below: no wake.
            self.hold_quietly(host, |_| ());
        }
        let mut outbox = lock(&self.outbox);
        if !self.enabled() {
            *outbox = Outbox::default();
            return Outgoing::default();
        }
        Outgoing {
            model: outbox.model.take(),
            mask: outbox.mask.take(),
            tags: outbox.tags.drain(..).collect(),
            metas: outbox.metas.drain(..).collect(),
        }
    }

    /// Queues `event` and applies it unless someone holds the host.
    fn push(&self, event: Event) {
        self.queue(event);
        if let Some(host) = self.try_host() {
            self.hold(host, |_| ());
        }
    }

    /// Queues `event` and applies it unless someone holds the host, without waking the
    /// control loop (for input decoded on the control thread itself, which calls
    /// [`Self::take_outgoing`] later in the same pass).
    fn push_quietly(&self, event: Event) {
        self.queue(event);
        if let Some(host) = self.try_host() {
            self.hold_quietly(host, |_| ());
        }
    }

    fn queue(&self, event: Event) {
        let mut inbox = lock(&self.inbox);
        if inbox.len() == INBOX {
            inbox.pop_front();
        }
        inbox.push_back(event);
    }

    /// Wakes the control loop, unless a wake is on its way: the control loop takes everything
    /// when it comes ([`Self::take_outgoing`]).
    fn wake(&self) {
        if self.woken.swap(true, Ordering::AcqRel) {
            return;
        }
        let wake = lock(&self.wake).clone();
        match wake {
            Some(wake) => wake(),
            None => self.woken.store(false, Ordering::Release),
        }
    }

    fn try_host(&self) -> Option<MutexGuard<'_, FrameWarpHost>> {
        match self.host.try_lock() {
            Ok(host) => Some(host),
            Err(TryLockError::Poisoned(poisoned)) => Some(poisoned.into_inner()),
            Err(TryLockError::WouldBlock) => None,
        }
    }

    /// Applies the inbox, runs `f`, applies what came in meanwhile and collects what the
    /// host produced; after letting go, takes the host again for events that came in
    /// after the last look, unless someone else has it, who then applies them.
    fn hold<R>(
        &self,
        host: MutexGuard<'_, FrameWarpHost>,
        f: impl FnOnce(&mut FrameWarpHost) -> R,
    ) -> R {
        let (result, produced) = self.hold_quietly(host, f);
        if produced {
            self.wake();
        }
        result
    }

    /// [`Self::hold`] without the wake; also whether the host produced anything to send.
    fn hold_quietly<R>(
        &self,
        mut host: MutexGuard<'_, FrameWarpHost>,
        f: impl FnOnce(&mut FrameWarpHost) -> R,
    ) -> (R, bool) {
        self.apply(&mut host);
        let result = f(&mut host);
        let mut produced = self.apply_and_collect(&mut host);
        drop(host);
        while !lock(&self.inbox).is_empty() {
            let Some(mut host) = self.try_host() else {
                break;
            };
            produced |= self.apply_and_collect(&mut host);
        }
        (result, produced)
    }

    fn apply_and_collect(&self, host: &mut FrameWarpHost) -> bool {
        self.apply(host);
        self.collect(host)
    }

    fn apply(&self, host: &mut FrameWarpHost) {
        let events: Vec<Event> = lock(&self.inbox).drain(..).collect();
        for event in events {
            match event {
                Event::Request(payload) => {
                    let _ = host.on_request(&payload);
                }
                Event::Mouse { dx, dy, at_us } => host.on_mouse(dx, dy, at_us),
                Event::Controller {
                    number,
                    stick,
                    at_us,
                } => host.on_controller(number, stick, at_us),
                Event::Tag { frame, capture_us } => host.tag(frame, capture_us),
                Event::Picture(picture, content) => host.set_picture(picture, content),
                Event::Motion {
                    number,
                    kind,
                    xyz,
                    at_us,
                } => host.on_motion(number, kind, xyz, at_us),
                Event::Cursor(visible) => host.on_cursor(visible),
            }
        }
    }

    /// Moves what the host produced to the outbox; whether there was anything.
    fn collect(&self, host: &mut FrameWarpHost) -> bool {
        let mut produced = false;
        {
            let mut outbox = lock(&self.outbox);
            if let Some(model) = host.take_model() {
                outbox.model = Some(model);
                produced = true;
            }
            if let Some(mask) = host.take_mask() {
                outbox.mask = Some(mask);
                produced = true;
            }
            while let Some(tags) = host.take_tags() {
                if outbox.tags.len() == OUTBOX_TAGS {
                    outbox.tags.pop_front();
                }
                outbox.tags.push_back(tags);
                produced = true;
            }
            while let Some(meta) = host.take_meta() {
                if outbox.metas.len() == OUTBOX_METAS {
                    outbox.metas.pop_front();
                }
                outbox.metas.push_back(meta);
                produced = true;
            }
        }
        *lock(&self.params) = Params {
            thumbnail: host.thumbnail_size(),
            mask_width: host.mask_width(),
            hud_every_us: host.hud_every_us(self.now_us()),
        };
        produced && self.enabled()
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

#[cfg(test)]
mod tests;
