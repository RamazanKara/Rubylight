//! What reaches the glass on Wayland: one `wp_presentation` feedback per presented picture tells
//! when the compositor showed it (or that it never did) and how: on a vertical blank (VSYNC) and
//! straight from our buffer without a composition pass (ZERO_COPY). Modelled on Nonary's
//! moonlight-qt fork (`waylandfeedback/wayland.cpp`): SDL's wl_display and wl_surface, a private
//! event queue whose objects are created through a display wrapper (so SDL's event thread never
//! dispatches them), one request before each present (the swap's own commit carries it), and a
//! non-blocking read polled from the render loop. libwayland-client is the one SDL loaded.
//! X11, and compositors without the protocol, get a clean "not offered" and nothing else.

use super::stage_times::percentile;

/// `wp_presentation_feedback.kind` bits.
pub const VSYNC: u32 = 0x1;
pub const HW_CLOCK: u32 = 0x2;
pub const HW_COMPLETION: u32 = 0x4;
pub const ZERO_COPY: u32 = 0x8;

/// A feedback unanswered this long is given up, as Nonary does (a hidden window gets none).
pub const TIMEOUT_NS: u64 = 250_000_000;
/// Requests in flight at most; the oldest is given up beyond it.
pub const MAX_PENDING: usize = 32;

/// How one presented picture went.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// On the glass `to_glass_ns` after the present call (None when the compositor's clock is not
    /// a monotonic one), with the feedback's kind bits.
    Presented { to_glass_ns: Option<u64>, flags: u32 },
    /// Replaced before it was shown.
    Discarded,
    /// No answer within [`TIMEOUT_NS`], or more than [`MAX_PENDING`] waiting.
    Unanswered,
}

/// One answered request: the stream's frame number, when its picture was decoded (session clock,
/// µs), when its present was called (session clock, µs), and the outcome.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shown {
    pub frame: u32,
    pub ready_us: u64,
    pub requested_us: u64,
    pub outcome: Outcome,
}

impl Shown {
    /// When the picture reached the glass on the session clock, if known.
    pub fn glass_us(&self) -> Option<u64> {
        match self.outcome {
            Outcome::Presented { to_glass_ns: Some(ns), .. } => Some(self.requested_us + ns / 1000),
            _ => None,
        }
    }
}

/// The kind bits by name, `|`-separated, for the timing CSV.
pub fn flag_names(flags: u32) -> String {
    let names = [(VSYNC, "vsync"), (HW_CLOCK, "hw_clock"), (HW_COMPLETION, "hw_completion"), (ZERO_COPY, "zero_copy")];
    names.iter().filter(|(bit, _)| flags & bit != 0).map(|(_, name)| *name).collect::<Vec<_>>().join("|")
}

/// Present to glass from the feedback's timestamp and the request's, both in the compositor's
/// clock (ns). A clock other than CLOCK_MONOTONIC or CLOCK_MONOTONIC_RAW can jump, and a
/// timestamp before the request is not this present's: both give None.
pub fn to_glass_ns(monotonic: bool, requested_ns: u64, presented_ns: u64) -> Option<u64> {
    (monotonic && presented_ns >= requested_ns).then(|| presented_ns - requested_ns)
}

/// The 64-bit seconds of a `presented` event and its nanoseconds as one count, None when out of range.
pub fn timestamp_ns(seconds_hi: u32, seconds_lo: u32, nanoseconds: u32) -> Option<u64> {
    let seconds = u64::from(seconds_hi) << 32 | u64::from(seconds_lo);
    if nanoseconds >= 1_000_000_000 { return None; }
    seconds.checked_mul(1_000_000_000)?.checked_add(u64::from(nanoseconds))
}

/// Histogram resolution and range of present to glass (longer counts in the last bucket); the
/// same 10 µs buckets as `stage_times`.
const BUCKET_US: u32 = 10;
const BUCKETS: usize = 10_000;

/// The stream's tally for the end-of-stream line.
pub struct Stats {
    presented: u64,
    discarded: u64,
    unanswered: u64,
    zero_copy: u64,
    vsync: u64,
    glass: Vec<u32>,
    timed: usize,
}

impl Default for Stats {
    fn default() -> Self {
        Self { presented: 0, discarded: 0, unanswered: 0, zero_copy: 0, vsync: 0, glass: vec![0; BUCKETS], timed: 0 }
    }
}

impl Stats {
    pub fn add(&mut self, outcome: Outcome) {
        match outcome {
            Outcome::Presented { to_glass_ns, flags } => {
                self.presented += 1;
                self.zero_copy += u64::from(flags & ZERO_COPY != 0);
                self.vsync += u64::from(flags & VSYNC != 0);
                if let Some(ns) = to_glass_ns {
                    let us = (ns / 1000).min(u64::from(u32::MAX)) as u32;
                    self.glass[((us / BUCKET_US) as usize).min(BUCKETS - 1)] += 1;
                    self.timed += 1;
                }
            }
            Outcome::Discarded => self.discarded += 1,
            Outcome::Unanswered => self.unanswered += 1,
        }
    }

    /// `presentation: zero-copy x%, vsync y%, present-to-glass p50/p95 a/b ms over n presents (...)`.
    pub fn line(&self) -> String {
        let tail = format!("{} discarded, {} unanswered", self.discarded, self.unanswered);
        if self.presented == 0 {
            return format!("presentation: no picture reported shown ({tail})");
        }
        let share = |n: u64| n as f64 * 100.0 / self.presented as f64;
        let glass = if self.timed == 0 {
            "present-to-glass unknown (the compositor's clock is not monotonic)".to_owned()
        } else {
            let at = |p| f64::from(percentile(&self.glass, self.timed, p)) / 1000.0;
            format!("present-to-glass p50/p95 {:.2}/{:.2} ms", at(50), at(95))
        };
        format!("presentation: zero-copy {:.1}%, vsync {:.1}%, {glass} over {} presents ({tail})", share(self.zero_copy), share(self.vsync), self.presented)
    }
}

#[cfg(target_os = "linux")]
pub use wayland::Feedback;

/// A stream's feedback: opened on its first picture, a request per picture, the answers tallied.
#[cfg(target_os = "linux")]
#[derive(Default)]
pub struct Glass {
    feedback: Option<Result<Feedback, String>>,
    /// The picture the coming present shows: its frame number and decode time (session clock, µs).
    next: Option<(u32, u64)>,
    stats: Stats,
    /// When the last picture was reported on the glass with a time (session clock, µs).
    last_timed_us: Option<u64>,
    /// What the compositor says about direct scanout, opened with the feedback; None while
    /// switched off (`NIGHTJAR_WAYLAND_SCANOUT_PROBE=0`) or before the first picture.
    scanout: Option<Result<super::wl_scanout::Probe, String>>,
    /// The swapchain as of the last pictures.
    facts: super::wl_scanout::SwapFacts,
    pictures: u64,
}

#[cfg(target_os = "linux")]
impl Glass {
    /// The coming present shows frame `frame`, decoded at `ready_us`.
    pub fn next_picture(&mut self, frame: u32, ready_us: u64) {
        self.next = Some((frame, ready_us));
    }

    /// Right before the present: asks for its feedback if it shows a picture (`next_picture`).
    pub fn request(&mut self, window: *mut sdl3_sys::everything::SDL_Window, now_us: u64) {
        let Some((frame, ready_us)) = self.next.take() else { return };
        let feedback = self.feedback.get_or_insert_with(|| {
            let opened = Feedback::open(window);
            match &opened {
                Ok(feedback) if feedback.timed() => eprintln!("nightjar: presentation: wp_presentation feedback on every picture"),
                Ok(_) => eprintln!("nightjar: presentation: wp_presentation feedback on every picture, without times (the compositor's clock is not monotonic)"),
                Err(why) => eprintln!("nightjar: presentation: feedback not offered ({why})"),
            }
            opened
        });
        if let Ok(feedback) = feedback {
            feedback.request(frame, ready_us, now_us);
        }
        use super::wl_scanout::{env_on, Probe, SCANOUT_PROBE_ENV};
        if self.pictures == 0 && env_on(SCANOUT_PROBE_ENV) {
            self.scanout = Some(Probe::open(window));
        }
        // The swapchain's format and mode, now and then (a few property reads).
        if self.pictures.is_multiple_of(256) {
            self.facts = crate::sdl::swap_facts(window);
        }
        self.pictures += 1;
    }

    /// The pictures answered since the last call, tallied for the end of the stream.
    pub fn poll(&mut self, now_us: u64) -> Vec<Shown> {
        if let Some(Ok(probe)) = &mut self.scanout {
            probe.poll();
        }
        super::wl_scanout::pump_installed();
        let Some(Ok(feedback)) = &mut self.feedback else { return Vec::new() };
        let shown = feedback.poll(now_us);
        for answer in &shown {
            self.stats.add(answer.outcome);
            if answer.glass_us().is_some() { self.last_timed_us = Some(now_us); }
        }
        shown
    }

    /// Whether the glass times stand in for the present call's return (a timed answer within
    /// the last second), so the doctor's display time runs to the glass.
    pub fn timing(&self, now_us: u64) -> bool {
        self.last_timed_us.is_some_and(|at| now_us.saturating_sub(at) < 1_000_000)
    }

    /// The end-of-stream line, None when no picture was presented.
    pub fn line(&self) -> Option<String> {
        match self.feedback.as_ref()? {
            Ok(_) => Some(self.stats.line()),
            Err(why) => Some(format!("presentation: not offered ({why})")),
        }
    }

    /// The end-of-stream lines: presentation, then what stood for or against direct scanout;
    /// none when no picture was presented.
    pub fn lines(&mut self) -> Vec<String> {
        let Some(presentation) = self.line() else { return Vec::new() };
        let scanout = match &mut self.scanout {
            None => format!("scanout: not looked at ({} is 0)", super::wl_scanout::SCANOUT_PROBE_ENV),
            Some(Err(why)) => format!("scanout: not looked at ({why})"),
            Some(Ok(probe)) => {
                let label = super::wl_scanout::label();
                let kde = std::env::var("XDG_CURRENT_DESKTOP").is_ok_and(|desktop| desktop.split(':').any(|name| name.eq_ignore_ascii_case("KDE")));
                super::wl_scanout::Report { seen: probe.seen(), label: label.as_ref(), facts: self.facts, zero_copy_seen: self.stats.zero_copy > 0, kde }.line()
            }
        };
        vec![presentation, scanout]
    }
}

#[cfg(target_os = "linux")]
pub(crate) mod wayland {
    use std::collections::VecDeque;
    use std::ffi::{c_char, c_int, c_void, CStr};
    use std::ptr::{null, null_mut};

    use sdl3_sys::everything::*;

    use super::{timestamp_ns, to_glass_ns, Outcome, Shown, MAX_PENDING, TIMEOUT_NS};

    #[repr(C)]
    pub(crate) struct Message {
        pub(crate) name: *const c_char,
        pub(crate) signature: *const c_char,
        pub(crate) types: *const *const Interface,
    }

    #[repr(C)]
    pub(crate) struct Interface {
        pub(crate) name: *const c_char,
        pub(crate) version: c_int,
        pub(crate) method_count: c_int,
        pub(crate) methods: *const Message,
        pub(crate) event_count: c_int,
        pub(crate) events: *const Message,
    }

    /// The protocol tables are immutable data shared with libwayland.
    #[repr(transparent)]
    pub(crate) struct Shared<T>(pub(crate) T);
    // SAFETY: never written after initialisation; libwayland only reads them.
    unsafe impl<T> Sync for Shared<T> {}

    // presentation-time.xml, version 1: what wayland-scanner would generate.
    static NO_TYPES: Shared<[*const Interface; 7]> = Shared([null(); 7]);
    static FEEDBACK_REQUEST_TYPES: Shared<[*const Interface; 2]> = Shared([null(), &FEEDBACK_INTERFACE.0]);
    static PRESENTATION_REQUESTS: Shared<[Message; 2]> = Shared([
        Message { name: c"destroy".as_ptr(), signature: c"".as_ptr(), types: NO_TYPES.0.as_ptr() },
        Message { name: c"feedback".as_ptr(), signature: c"on".as_ptr(), types: FEEDBACK_REQUEST_TYPES.0.as_ptr() },
    ]);
    static PRESENTATION_EVENTS: Shared<[Message; 1]> = Shared([Message { name: c"clock_id".as_ptr(), signature: c"u".as_ptr(), types: NO_TYPES.0.as_ptr() }]);
    static PRESENTATION_INTERFACE: Shared<Interface> = Shared(Interface {
        name: c"wp_presentation".as_ptr(),
        version: 1,
        method_count: 2,
        methods: PRESENTATION_REQUESTS.0.as_ptr(),
        event_count: 1,
        events: PRESENTATION_EVENTS.0.as_ptr(),
    });
    static FEEDBACK_EVENTS: Shared<[Message; 3]> = Shared([
        Message { name: c"sync_output".as_ptr(), signature: c"o".as_ptr(), types: NO_TYPES.0.as_ptr() },
        Message { name: c"presented".as_ptr(), signature: c"uuuuuuu".as_ptr(), types: NO_TYPES.0.as_ptr() },
        Message { name: c"discarded".as_ptr(), signature: c"".as_ptr(), types: NO_TYPES.0.as_ptr() },
    ]);
    static FEEDBACK_INTERFACE: Shared<Interface> = Shared(Interface {
        name: c"wp_presentation_feedback".as_ptr(),
        version: 1,
        method_count: 0,
        methods: null(),
        event_count: 3,
        events: FEEDBACK_EVENTS.0.as_ptr(),
    });

    pub(crate) const DISPLAY_GET_REGISTRY: u32 = 1;
    pub(crate) const REGISTRY_BIND: u32 = 0;
    const PRESENTATION_DESTROY: u32 = 0;
    const PRESENTATION_FEEDBACK: u32 = 1;
    pub(crate) const MARSHAL_FLAG_DESTROY: u32 = 1;

    pub(crate) type Proxy = c_void;
    pub(crate) type Queue = c_void;
    pub(crate) type MarshalFlags = unsafe extern "C" fn(*mut Proxy, u32, *const Interface, u32, u32, ...) -> *mut Proxy;

    /// libwayland-client's calls, from the library SDL's Wayland driver already loaded (also
    /// `wl_scanout`'s).
    pub(crate) struct Api {
        pub(crate) marshal_flags: MarshalFlags,
        pub(crate) add_listener: unsafe extern "C" fn(*mut Proxy, *const c_void, *mut c_void) -> c_int,
        pub(crate) destroy: unsafe extern "C" fn(*mut Proxy),
        pub(crate) get_version: unsafe extern "C" fn(*mut Proxy) -> u32,
        pub(crate) create_wrapper: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
        pub(crate) wrapper_destroy: unsafe extern "C" fn(*mut c_void),
        pub(crate) set_queue: unsafe extern "C" fn(*mut Proxy, *mut Queue),
        pub(crate) create_queue: unsafe extern "C" fn(*mut c_void) -> *mut Queue,
        pub(crate) queue_destroy: unsafe extern "C" fn(*mut Queue),
        pub(crate) roundtrip_queue: unsafe extern "C" fn(*mut c_void, *mut Queue) -> c_int,
        pub(crate) dispatch_queue_pending: unsafe extern "C" fn(*mut c_void, *mut Queue) -> c_int,
        pub(crate) prepare_read_queue: unsafe extern "C" fn(*mut c_void, *mut Queue) -> c_int,
        pub(crate) read_events: unsafe extern "C" fn(*mut c_void) -> c_int,
        pub(crate) cancel_read: unsafe extern "C" fn(*mut c_void),
        pub(crate) flush: unsafe extern "C" fn(*mut c_void) -> c_int,
        pub(crate) get_fd: unsafe extern "C" fn(*mut c_void) -> c_int,
        pub(crate) registry_interface: *const Interface,
        pub(crate) _library: libloading::Library,
    }

    impl Api {
        pub(crate) fn load() -> Result<Self, String> {
            // SAFETY: libwayland-client's exported functions with their wayland-client-core.h
            // signatures, and its wl_registry_interface table.
            unsafe {
                let library = libloading::Library::new("libwayland-client.so.0").map_err(|e| format!("cannot load libwayland-client.so.0: {e}"))?;
                macro_rules! get {
                    ($name:literal) => {
                        *library.get(concat!($name, "\0").as_bytes()).map_err(|e| format!("{}: {e}", $name))?
                    };
                }
                let registry: libloading::Symbol<*const Interface> = library.get(b"wl_registry_interface\0").map_err(|e| format!("wl_registry_interface: {e}"))?;
                let registry_interface = *registry;
                Ok(Self {
                    marshal_flags: get!("wl_proxy_marshal_flags"),
                    add_listener: get!("wl_proxy_add_listener"),
                    destroy: get!("wl_proxy_destroy"),
                    get_version: get!("wl_proxy_get_version"),
                    create_wrapper: get!("wl_proxy_create_wrapper"),
                    wrapper_destroy: get!("wl_proxy_wrapper_destroy"),
                    set_queue: get!("wl_proxy_set_queue"),
                    create_queue: get!("wl_display_create_queue"),
                    queue_destroy: get!("wl_event_queue_destroy"),
                    roundtrip_queue: get!("wl_display_roundtrip_queue"),
                    dispatch_queue_pending: get!("wl_display_dispatch_queue_pending"),
                    prepare_read_queue: get!("wl_display_prepare_read_queue"),
                    read_events: get!("wl_display_read_events"),
                    cancel_read: get!("wl_display_cancel_read"),
                    flush: get!("wl_display_flush"),
                    get_fd: get!("wl_display_get_fd"),
                    registry_interface,
                    _library: library,
                })
            }
        }
    }

    /// What the registry and presentation listeners fill in during the set-up round trips.
    struct Globals {
        presentation: *mut Proxy,
        clock: Option<u32>,
        api: *const Api,
    }

    /// One request in flight; its address is the listener's data, so it stays boxed.
    struct Pending {
        proxy: *mut Proxy,
        frame: u32,
        ready_us: u64,
        requested_us: u64,
        requested_ns: u64,
        answer: Option<Answer>,
    }

    #[derive(Clone, Copy)]
    enum Answer {
        Presented { ns: Option<u64>, flags: u32 },
        Discarded,
    }

    /// The feedback of one window's presents.
    pub struct Feedback {
        api: Box<Api>,
        display: *mut c_void,
        surface: *mut Proxy,
        queue: *mut Queue,
        registry: *mut Proxy,
        globals: Box<Globals>,
        clock: libc::clockid_t,
        monotonic: bool,
        pending: VecDeque<Box<Pending>>,
        done: Vec<Shown>,
    }

    // SAFETY: libwayland's proxies and queues may be used from any thread; the feedback is used
    // from one at a time (the stream's presenting thread).
    unsafe impl Send for Feedback {}

    unsafe extern "C" fn global(data: *mut c_void, registry: *mut Proxy, name: u32, interface: *const c_char, version: u32) {
        // SAFETY: `data` is the Globals box the registry was given, alive while it is.
        let globals = unsafe { &mut *data.cast::<Globals>() };
        // SAFETY: the api outlives the registry; `interface` is a string from libwayland.
        unsafe {
            let api = &*globals.api;
            if !globals.presentation.is_null() || CStr::from_ptr(interface) != c"wp_presentation" { return; }
            // Version 1 is all this needs (Nonary binds the same).
            let bound = (api.marshal_flags)(registry, REGISTRY_BIND, &PRESENTATION_INTERFACE.0, 1, 0, name, PRESENTATION_INTERFACE.0.name, version.min(1), null_mut::<c_void>());
            if bound.is_null() { return; }
            globals.presentation = bound;
            (api.add_listener)(bound, PRESENTATION_LISTENER.0.as_ptr().cast(), data);
        }
    }

    unsafe extern "C" fn global_remove(_data: *mut c_void, _registry: *mut Proxy, _name: u32) {}

    unsafe extern "C" fn clock_id(data: *mut c_void, _presentation: *mut Proxy, clock: u32) {
        // SAFETY: the Globals box, alive while the presentation object is.
        unsafe { (*data.cast::<Globals>()).clock = Some(clock) };
    }

    unsafe extern "C" fn sync_output(_data: *mut c_void, _feedback: *mut Proxy, _output: *mut Proxy) {}

    #[allow(clippy::too_many_arguments)]
    unsafe extern "C" fn presented(data: *mut c_void, _feedback: *mut Proxy, hi: u32, lo: u32, ns: u32, _refresh: u32, _seq_hi: u32, _seq_lo: u32, flags: u32) {
        // SAFETY: the Pending box the feedback was given; it is destroyed with the proxy.
        unsafe { (*data.cast::<Pending>()).answer = Some(Answer::Presented { ns: timestamp_ns(hi, lo, ns), flags }) };
    }

    unsafe extern "C" fn discarded(data: *mut c_void, _feedback: *mut Proxy) {
        // SAFETY: as in `presented`.
        unsafe { (*data.cast::<Pending>()).answer = Some(Answer::Discarded) };
    }

    type GlobalFn = unsafe extern "C" fn(*mut c_void, *mut Proxy, u32, *const c_char, u32);
    type RemoveFn = unsafe extern "C" fn(*mut c_void, *mut Proxy, u32);
    type ClockFn = unsafe extern "C" fn(*mut c_void, *mut Proxy, u32);
    type SyncFn = unsafe extern "C" fn(*mut c_void, *mut Proxy, *mut Proxy);
    type PresentedFn = unsafe extern "C" fn(*mut c_void, *mut Proxy, u32, u32, u32, u32, u32, u32, u32);
    type DiscardedFn = unsafe extern "C" fn(*mut c_void, *mut Proxy);

    /// wl_registry_listener and wp_presentation_feedback_listener, laid out as C does.
    #[repr(C)]
    struct RegistryListener {
        global: GlobalFn,
        global_remove: RemoveFn,
    }

    #[repr(C)]
    struct FeedbackListener {
        sync_output: SyncFn,
        presented: PresentedFn,
        discarded: DiscardedFn,
    }

    static REGISTRY_LISTENER: RegistryListener = RegistryListener { global, global_remove };
    static PRESENTATION_LISTENER: Shared<[ClockFn; 1]> = Shared([clock_id]);
    static FEEDBACK_LISTENER: FeedbackListener = FeedbackListener { sync_output, presented, discarded };

    fn clock_ns(clock: libc::clockid_t) -> Option<u64> {
        let mut now = libc::timespec { tv_sec: 0, tv_nsec: 0 };
        // SAFETY: a valid out pointer.
        (unsafe { libc::clock_gettime(clock, &mut now) } == 0).then(|| now.tv_sec as u64 * 1_000_000_000 + now.tv_nsec as u64)
    }

    impl Feedback {
        /// Feedback for `window`'s presents, or why there is none ("not offered").
        pub fn open(window: *mut SDL_Window) -> Result<Self, String> {
            // SAFETY: a live window; the pointers are SDL's own Wayland objects.
            let (display, surface) = unsafe {
                let props = SDL_GetWindowProperties(window);
                (
                    SDL_GetPointerProperty(props, SDL_PROP_WINDOW_WAYLAND_DISPLAY_POINTER, null_mut()),
                    SDL_GetPointerProperty(props, SDL_PROP_WINDOW_WAYLAND_SURFACE_POINTER, null_mut()),
                )
            };
            if display.is_null() || surface.is_null() {
                return Err("the window is not a Wayland one".into());
            }
            let api = Box::new(Api::load()?);
            // SAFETY: SDL's live display; every object made here belongs to the private queue.
            unsafe {
                let queue = (api.create_queue)(display);
                if queue.is_null() { return Err("no Wayland event queue".into()); }
                let mut feedback = Feedback {
                    api,
                    display,
                    surface,
                    queue,
                    registry: null_mut(),
                    globals: Box::new(Globals { presentation: null_mut(), clock: None, api: null() }),
                    clock: libc::CLOCK_MONOTONIC,
                    monotonic: false,
                    pending: VecDeque::new(),
                    done: Vec::new(),
                };
                feedback.globals.api = &*feedback.api;
                // Through a wrapper the registry is on the private queue from its creation, before
                // SDL's thread could read an event for it.
                let wrapper = (feedback.api.create_wrapper)(display);
                if wrapper.is_null() { return Err("no Wayland display wrapper".into()); }
                (feedback.api.set_queue)(wrapper, queue);
                let version = (feedback.api.get_version)(wrapper);
                feedback.registry = (feedback.api.marshal_flags)(wrapper, DISPLAY_GET_REGISTRY, feedback.api.registry_interface, version, 0, null_mut::<c_void>());
                (feedback.api.wrapper_destroy)(wrapper);
                if feedback.registry.is_null() { return Err("no Wayland registry".into()); }
                let globals: *mut Globals = &mut *feedback.globals;
                (feedback.api.add_listener)(feedback.registry, (&REGISTRY_LISTENER as *const RegistryListener).cast(), globals.cast());
                if (feedback.api.roundtrip_queue)(display, queue) < 0 { return Err("the Wayland registry round trip failed".into()); }
                if feedback.globals.presentation.is_null() { return Err("the compositor does not offer wp_presentation".into()); }
                // The clock_id event follows the bind.
                if (feedback.api.roundtrip_queue)(display, queue) < 0 { return Err("the wp_presentation round trip failed".into()); }
                let Some(clock) = feedback.globals.clock else { return Err("the compositor sent no presentation clock".into()) };
                feedback.clock = clock as libc::clockid_t;
                feedback.monotonic = feedback.clock == libc::CLOCK_MONOTONIC || feedback.clock == libc::CLOCK_MONOTONIC_RAW;
                Ok(feedback)
            }
        }

        /// Whether present to glass can be timed (the compositor's clock is monotonic).
        pub fn timed(&self) -> bool { self.monotonic }

        /// Asks for the feedback of the next commit, the coming present's: call right before it.
        /// `frame` and `ready_us` come back with the answer; `now_us` is the session clock.
        pub fn request(&mut self, frame: u32, ready_us: u64, now_us: u64) {
            self.pump(now_us);
            if self.pending.len() >= MAX_PENDING {
                if let Some(oldest) = self.pending.pop_front() { self.finish(&oldest, Some(Outcome::Unanswered)); }
            }
            let Some(requested_ns) = clock_ns(self.clock) else { return };
            let mut pending = Box::new(Pending { proxy: null_mut(), frame, ready_us, requested_us: now_us, requested_ns, answer: None });
            // SAFETY: live objects on the private queue; the box outlives the proxy (`finish`).
            unsafe {
                let presentation = self.globals.presentation;
                let proxy = (self.api.marshal_flags)(presentation, PRESENTATION_FEEDBACK, &FEEDBACK_INTERFACE.0, (self.api.get_version)(presentation), 0, self.surface, null_mut::<c_void>());
                if proxy.is_null() { return; }
                pending.proxy = proxy;
                (self.api.add_listener)(proxy, (&FEEDBACK_LISTENER as *const FeedbackListener).cast(), (&mut *pending as *mut Pending).cast());
            }
            // No commit here: the present's own swap commits the surface and carries the request.
            // The match is by order, not by identity: the feedback belongs to the surface's next
            // commit, so it describes the coming present only while nothing else commits the
            // surface in between. A commit SDL or the driver makes for itself (a resize, a
            // presentation the WSI defers past this call, MAILBOX replacing a picture before the
            // compositor latched it) takes the request instead, and is answered as this frame or
            // as discarded. The statistics carry that error; they never stall a present.
            self.pending.push_back(pending);
        }

        /// Reads what arrived without blocking and returns the requests answered since the last call.
        pub fn poll(&mut self, now_us: u64) -> Vec<Shown> {
            self.pump(now_us);
            std::mem::take(&mut self.done)
        }

        fn pump(&mut self, now_us: u64) {
            // SAFETY: SDL's live display and our queue; the read protocol shared with SDL's thread.
            unsafe {
                if (self.api.prepare_read_queue)(self.display, self.queue) == 0 {
                    (self.api.flush)(self.display);
                    let mut fd = libc::pollfd { fd: (self.api.get_fd)(self.display), events: libc::POLLIN, revents: 0 };
                    if libc::poll(&mut fd, 1, 0) > 0 && fd.revents & libc::POLLIN != 0 {
                        (self.api.read_events)(self.display);
                    } else {
                        (self.api.cancel_read)(self.display);
                    }
                }
                (self.api.dispatch_queue_pending)(self.display, self.queue);
            }
            // One pass over the queue in place: each request leaves from the front and the
            // waiting ones go back to the back, in order, without a queue of its own.
            for _ in 0..self.pending.len() {
                let Some(pending) = self.pending.pop_front() else { break };
                let expired = now_us.saturating_sub(pending.requested_us) * 1000 > TIMEOUT_NS;
                match (pending.answer, expired) {
                    (Some(_), _) => self.finish(&pending, None),
                    (None, true) => self.finish(&pending, Some(Outcome::Unanswered)),
                    (None, false) => self.pending.push_back(pending),
                }
            }
        }

        /// Destroys the request's proxy (its box may go after this) and keeps its answer.
        fn finish(&mut self, pending: &Pending, outcome: Option<Outcome>) {
            // SAFETY: the feedback proxy, destroyed once; later events for it are dropped.
            unsafe { (self.api.destroy)(pending.proxy) };
            let outcome = outcome.unwrap_or(match pending.answer {
                Some(Answer::Presented { ns, flags }) => Outcome::Presented { to_glass_ns: ns.and_then(|ns| to_glass_ns(self.monotonic, pending.requested_ns, ns)), flags },
                Some(Answer::Discarded) => Outcome::Discarded,
                None => Outcome::Unanswered,
            });
            self.done.push(Shown { frame: pending.frame, ready_us: pending.ready_us, requested_us: pending.requested_us, outcome });
        }
    }

    impl Drop for Feedback {
        fn drop(&mut self) {
            // SAFETY: our own objects, destroyed once, children first.
            unsafe {
                for pending in std::mem::take(&mut self.pending) { (self.api.destroy)(pending.proxy); }
                if !self.globals.presentation.is_null() {
                    (self.api.marshal_flags)(self.globals.presentation, PRESENTATION_DESTROY, null(), (self.api.get_version)(self.globals.presentation), MARSHAL_FLAG_DESTROY);
                }
                if !self.registry.is_null() { (self.api.destroy)(self.registry); }
                (self.api.flush)(self.display);
                (self.api.queue_destroy)(self.queue);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stats_give_shares_and_percentiles_of_the_presented() {
        let mut stats = Stats::default();
        for i in 0..100u64 {
            let flags = VSYNC | if i < 75 { ZERO_COPY } else { 0 };
            stats.add(Outcome::Presented { to_glass_ns: Some((1_000 + i * 100) * 1000), flags });
        }
        stats.add(Outcome::Discarded);
        stats.add(Outcome::Unanswered);
        stats.add(Outcome::Unanswered);
        assert_eq!(
            stats.line(),
            "presentation: zero-copy 75.0%, vsync 100.0%, present-to-glass p50/p95 6.00/10.40 ms over 100 presents (1 discarded, 2 unanswered)"
        );
    }

    #[test]
    fn stats_without_answers_or_a_monotonic_clock_say_so() {
        let mut stats = Stats::default();
        stats.add(Outcome::Unanswered);
        assert_eq!(stats.line(), "presentation: no picture reported shown (0 discarded, 1 unanswered)");
        stats.add(Outcome::Presented { to_glass_ns: None, flags: ZERO_COPY });
        assert!(stats.line().starts_with("presentation: zero-copy 100.0%, vsync 0.0%, present-to-glass unknown"), "{}", stats.line());
    }

    #[test]
    fn a_long_present_counts_in_the_last_bucket() {
        let mut stats = Stats::default();
        stats.add(Outcome::Presented { to_glass_ns: Some(5_000_000_000), flags: 0 });
        assert!(stats.line().contains("p50/p95 99.99/99.99 ms"), "{}", stats.line());
    }

    #[test]
    fn glass_time_needs_a_monotonic_clock_and_a_later_timestamp() {
        assert_eq!(to_glass_ns(true, 1_000, 7_000), Some(6_000));
        assert_eq!(to_glass_ns(true, 7_000, 1_000), None);
        assert_eq!(to_glass_ns(false, 1_000, 7_000), None);
        assert_eq!(timestamp_ns(0, 2, 5), Some(2_000_000_005));
        assert_eq!(timestamp_ns(1, 0, 0), Some((1u64 << 32) * 1_000_000_000));
        assert_eq!(timestamp_ns(0, 1, 1_000_000_000), None);
        assert_eq!(timestamp_ns(u32::MAX, u32::MAX, 0), None);
    }

    #[test]
    fn shown_puts_the_glass_on_the_session_clock() {
        let shown = Shown { frame: 7, ready_us: 900, requested_us: 1_000, outcome: Outcome::Presented { to_glass_ns: Some(6_500_000), flags: VSYNC } };
        assert_eq!(shown.glass_us(), Some(7_500));
        assert_eq!(Shown { outcome: Outcome::Discarded, ..shown }.glass_us(), None);
        assert_eq!(Shown { outcome: Outcome::Presented { to_glass_ns: None, flags: 0 }, ..shown }.glass_us(), None);
    }

    /// The binding against a real compositor: SDL's Wayland window, its software renderer's
    /// presents, and the answers through the private queue. Run it under a headless Weston:
    /// `weston --backend=headless --socket=nj-test & WAYLAND_DISPLAY=nj-test cargo test -p
    /// nightjar-desktop -- --ignored stream::wl_presentation`.
    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "needs a Wayland compositor (WAYLAND_DISPLAY)"]
    fn a_compositor_answers_each_present() {
        use nightjar_core::session::now_us;
        use sdl3_sys::everything::*;
        // SAFETY: plain SDL calls on objects made here, on this thread.
        unsafe {
            SDL_SetHint(SDL_HINT_VIDEO_DRIVER, c"wayland".as_ptr());
            assert!(SDL_Init(SDL_INIT_VIDEO), "{}", crate::sdl::error());
            let window = SDL_CreateWindow(c"glass".as_ptr(), 320, 240, SDL_WindowFlags::default());
            assert!(!window.is_null(), "{}", crate::sdl::error());
            let renderer = SDL_CreateRenderer(window, c"software".as_ptr());
            assert!(!renderer.is_null(), "{}", crate::sdl::error());
            let mut feedback = Feedback::open(window).expect("the compositor offers wp_presentation");
            let mut answers = Vec::new();
            for frame in 0..90u32 {
                SDL_PumpEvents();
                SDL_SetRenderDrawColor(renderer, (frame * 2) as u8, 0, 0, 255);
                SDL_RenderClear(renderer);
                feedback.request(frame, now_us(), now_us());
                SDL_RenderPresent(renderer);
                std::thread::sleep(std::time::Duration::from_millis(16));
                answers.extend(feedback.poll(now_us()));
            }
            for _ in 0..20 {
                SDL_PumpEvents();
                std::thread::sleep(std::time::Duration::from_millis(16));
                answers.extend(feedback.poll(now_us()));
            }
            let mut stats = Stats::default();
            answers.iter().for_each(|answer| stats.add(answer.outcome));
            eprintln!("{} (clock monotonic: {})", stats.line(), feedback.timed());
            let presented: Vec<&Shown> = answers.iter().filter(|answer| matches!(answer.outcome, Outcome::Presented { .. })).collect();
            assert!(presented.len() >= 30, "{} of 90 presents answered as shown", presented.len());
            assert!(presented.windows(2).all(|pair| pair[0].frame < pair[1].frame), "answers come back in present order");
            if feedback.timed() {
                assert!(presented.iter().all(|answer| answer.glass_us().is_some()));
            }
            drop(feedback);
            SDL_DestroyRenderer(renderer);
            SDL_DestroyWindow(window);
            SDL_Quit();
        }
    }

    #[test]
    fn flags_read_by_name() {
        assert_eq!(flag_names(0), "");
        assert_eq!(flag_names(VSYNC | ZERO_COPY), "vsync|zero_copy");
        assert_eq!(flag_names(0xf), "vsync|hw_clock|hw_completion|zero_copy");
    }
}
