//! A stream's Frame Warp ([`FrameWarpHost`]) where no hot path waits for it.
//!
//! The control thread decodes input and the packetizer assigns frame numbers;
//! neither may wait, but the frame-warp worker holds the host for every
//! thumbnail, whose fit can take milliseconds. So every event goes into a
//! short inbox, and whoever gets the host next (`try_lock`) applies the inbox
//! in order; a holder applies what came in meanwhile before it lets go. What
//! the host produces goes into an outbox the control loop takes from, and the
//! control loop is woken.
//!
//! Times are microseconds on the session's clock: since `Session::started`.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, TryLockError};
use std::time::Instant;

use crate::frame_warp::{FrameWarpHost, FrameWarpRequest, MODEL_BYTES, Status};

/// Wakes the control loop: something is waiting to be sent.
pub type Wake = Arc<dyn Fn() + Send + Sync>;

/// Events kept for the host at most; past it the oldest go (the history loses them).
const INBOX: usize = 16_384;
/// Tag messages kept for the control loop; older ones go (the next carries their tags).
const OUTBOX_TAGS: usize = 8;
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
    Picture((u16, u16)),
}

#[derive(Default)]
struct Outbox {
    model: Option<[u8; MODEL_BYTES]>,
    mask: Option<Vec<u8>>,
    tags: VecDeque<Vec<u8>>,
}

/// What the video thread asks of the host every capture, as the host last said.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Params {
    /// [`FrameWarpHost::thumbnail_size`].
    pub thumbnail: (usize, usize),
    /// [`FrameWarpHost::mask_width`].
    pub mask_width: u16,
    /// [`FrameWarpHost::camera_moving`] when the host was last held.
    pub moving: bool,
}

/// What the control loop sends, in this order: the model and mask reliably,
/// then the tags unreliably.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Outgoing {
    pub model: Option<[u8; MODEL_BYTES]>,
    pub mask: Option<Vec<u8>>,
    pub tags: Vec<Vec<u8>>,
}
impl Outgoing {
    pub fn is_empty(&self) -> bool {
        self.model.is_none() && self.mask.is_none() && self.tags.is_empty()
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
    wake: Mutex<Option<Wake>>,
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
            moving: false,
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
            wake: Mutex::new(None),
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
    /// malformed request, which changes nothing.
    pub fn on_request(&self, payload: &[u8]) -> Option<bool> {
        let request = FrameWarpRequest::decode(payload)?;
        self.on.store(request.enabled, Ordering::Release);
        if !request.enabled {
            *lock(&self.outbox) = Outbox::default();
            lock(&self.inbox).retain(|event| matches!(event, Event::Picture(_)));
        }
        self.push(Event::Request(payload.to_vec()));
        Some(request.enabled)
    }

    /// A relative mouse move the control thread decoded at `at`.
    pub fn on_mouse(&self, dx: i16, dy: i16, at: Instant) {
        if self.enabled() && (dx != 0 || dy != 0) {
            let at_us = self.us(at);
            self.push(Event::Mouse { dx, dy, at_us });
        }
    }

    /// A controller packet the control thread decoded at `at`, with its right stick.
    pub fn on_controller(&self, number: u16, stick: (i16, i16), at: Instant) {
        if self.enabled() {
            let at_us = self.us(at);
            self.push(Event::Controller {
                number,
                stick,
                at_us,
            });
        }
    }

    /// Tags video frame `frame` as its number is assigned. `captured` is the capture's
    /// present time; a repeated picture has none and shows the last capture tagged.
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
        self.push(Event::Tag { frame, capture_us });
    }

    /// The stream's picture size changed (a reconfigure).
    pub fn set_picture(&self, picture: (u16, u16)) {
        self.push(Event::Picture(picture));
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

    /// Everything waiting to be sent; nothing while Frame Warp is off.
    pub fn take_outgoing(&self) -> Outgoing {
        let mut outbox = lock(&self.outbox);
        if !self.enabled() {
            *outbox = Outbox::default();
            return Outgoing::default();
        }
        Outgoing {
            model: outbox.model.take(),
            mask: outbox.mask.take(),
            tags: outbox.tags.drain(..).collect(),
        }
    }

    fn push(&self, event: Event) {
        {
            let mut inbox = lock(&self.inbox);
            if inbox.len() == INBOX {
                inbox.pop_front();
            }
            inbox.push_back(event);
        }
        if let Some(host) = self.try_host() {
            self.hold(host, |_| ());
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
        mut host: MutexGuard<'_, FrameWarpHost>,
        f: impl FnOnce(&mut FrameWarpHost) -> R,
    ) -> R {
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
        if produced {
            let wake = lock(&self.wake).clone();
            if let Some(wake) = wake {
                wake();
            }
        }
        result
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
                Event::Picture(picture) => host.set_picture(picture),
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
        }
        *lock(&self.params) = Params {
            thumbnail: host.thumbnail_size(),
            mask_width: host.mask_width(),
            moving: host.camera_moving(self.now_us()),
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
mod tests {
    use super::*;
    use crate::frame_warp::{FRAME_WARP_REQUEST_MESSAGE_TYPE, Tags};
    use std::sync::atomic::AtomicUsize;
    use std::time::Duration;

    fn request(enabled: bool, controller: Option<u8>, mask_max_width: u16) -> [u8; 8] {
        FrameWarpRequest {
            enabled,
            controller,
            mask_max_width,
        }
        .encode()
    }

    fn state() -> (Instant, FrameWarpState) {
        let epoch = Instant::now();
        (epoch, FrameWarpState::new((1920, 1080), epoch))
    }

    #[test]
    fn nothing_happens_until_a_client_asks() {
        let (epoch, warp) = state();
        assert_eq!(FRAME_WARP_REQUEST_MESSAGE_TYPE, 0x5543);
        assert!(!warp.enabled());
        warp.on_mouse(5, 5, epoch);
        warp.tag(1, Some(epoch));
        assert!(warp.take_outgoing().is_empty());
        assert!(lock(&warp.inbox).is_empty());
        assert_eq!(warp.on_request(&[1, 2]), None);
        assert!(!warp.enabled());
    }

    #[test]
    fn a_request_turns_it_on_and_input_reaches_the_tags() {
        let (epoch, warp) = state();
        let wakes = Arc::new(AtomicUsize::new(0));
        let counter = wakes.clone();
        warp.set_wake(Arc::new(move || {
            counter.fetch_add(1, Ordering::Relaxed);
        }));
        assert_eq!(warp.on_request(&request(true, Some(0), 1024)), Some(true));
        assert!(warp.enabled());
        // The client counts from its request: it hears a model at once.
        let first = warp.take_outgoing();
        assert!(first.model.is_some());
        assert!(wakes.load(Ordering::Relaxed) >= 1);
        warp.on_mouse(10, -3, epoch + Duration::from_millis(1));
        warp.on_mouse(5, 1, epoch + Duration::from_millis(2));
        warp.on_controller(0, (1000, 0), epoch + Duration::from_millis(3));
        // Captured well after the input and the default lag.
        warp.tag(7, Some(epoch + Duration::from_millis(200)));
        let sent = warp.take_outgoing();
        let [tags] = sent.tags.as_slice() else {
            panic!("one tags message, not {}", sent.tags.len());
        };
        let tags = Tags::decode(tags).unwrap();
        let tag = tags.tags.last().unwrap();
        assert_eq!((tag.frame, tag.mouse_x, tag.mouse_y), (7, 15, -2));
        assert_eq!(tag.controller_seq, 1);
        // A repeated picture shows the last capture's input.
        warp.on_mouse(100, 0, epoch + Duration::from_millis(300));
        warp.tag(8, None);
        let sent = warp.take_outgoing();
        let tags = Tags::decode(sent.tags.last().unwrap()).unwrap();
        let tag = tags.tags.last().unwrap();
        assert_eq!((tag.frame, tag.mouse_x), (8, 15));
        // The picture halved until it fits the client's 1024.
        assert_eq!(warp.params().mask_width, 960);
    }

    #[test]
    fn events_wait_for_a_busy_host_and_keep_their_order() {
        let (epoch, warp) = state();
        warp.on_request(&request(true, None, 0));
        let _ = warp.take_outgoing();
        // While the worker holds the host, input and tags only queue.
        warp.with(|_| {
            warp.on_mouse(4, 0, epoch + Duration::from_millis(1));
            warp.tag(3, Some(epoch + Duration::from_millis(500)));
            assert_eq!(lock(&warp.inbox).len(), 2);
        });
        assert!(lock(&warp.inbox).is_empty());
        let sent = warp.take_outgoing();
        let tags = Tags::decode(sent.tags.last().unwrap()).unwrap();
        let tag = tags.tags.last().unwrap();
        assert_eq!((tag.frame, tag.mouse_x), (3, 4));
    }

    #[test]
    fn turning_it_off_drops_what_was_waiting() {
        let (epoch, warp) = state();
        warp.on_request(&request(true, None, 0));
        warp.tag(1, Some(epoch));
        assert_eq!(warp.on_request(&request(false, None, 0)), Some(false));
        assert!(!warp.enabled());
        assert!(warp.take_outgoing().is_empty());
        warp.on_mouse(1, 1, epoch);
        warp.tag(2, Some(epoch));
        assert!(warp.take_outgoing().is_empty());
        assert!(!warp.status().enabled);
    }

    #[test]
    fn the_prior_is_claimed_once() {
        let (_, warp) = state();
        assert!(warp.claim_prior());
        assert!(!warp.claim_prior());
    }

    #[test]
    fn the_clock_counts_from_the_epoch() {
        let (epoch, warp) = state();
        assert_eq!(warp.us(epoch), 0);
        assert_eq!(warp.us(epoch + Duration::from_micros(1500)), 1500);
        if let Some(before) = epoch.checked_sub(Duration::from_millis(1)) {
            assert_eq!(warp.us(before), 0);
        }
    }
}
