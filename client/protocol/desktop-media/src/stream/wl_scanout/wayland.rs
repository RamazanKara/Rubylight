//! Nightjar's own HDR description of the window: [`HdrDescription::prepare`] reads the compositor's
//! preferred description and makes the matching one before the PQ renderer, whose swapchain is
//! then made pass-through; [`HdrDescription::attach`] gives it to the surface before the first
//! present, and [`install`] keeps it while that renderer lives. Every renderer
//! made for the window drops it first ([`uninstall`]), before its swapchain could bring Mesa's.

use std::ffi::c_void;
use std::ptr::{null, null_mut};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use sdl3_sys::everything::*;

use crate::stream::wl_presentation::wayland::Proxy;
use super::cm::INTENT_PERCEPTUAL;
use super::{
    brightness, content_peak, describe_label, env_on, hdr_label, set_label, Brightness,
    DescriptionInfo, HdrLabel, Label, Seen, GAMMA22_PRESENT_ENV, PQ_DEFAULT_REFERENCE_NITS,
};

mod protocol;
use protocol::{
    color_feedback, create_pq, manager, preferred, ChangedData, CmData, Conn, DmabufData,
    ImageData, InfoData, Object, CM_DESTROY, CM_GET_SURFACE, CM_SURFACE_IF, DMABUF_DESTROY,
    DMABUF_FEEDBACK_DESTROY, DMABUF_FEEDBACK_IF, DMABUF_FEEDBACK_LISTENER,
    DMABUF_GET_SURFACE_FEEDBACK, DMABUF_IF, DMABUF_LISTENER, FEEDBACK_DESTROY,
    FEEDBACK_GET_PREFERRED, IMAGE_DESTROY, IMAGE_GET_INFORMATION, IMAGE_IF, IMAGE_LISTENER,
    INFO_IF, INFO_LISTENER, SURFACE_DESTROY, SURFACE_SET_IMAGE_DESCRIPTION,
};

/// Nightjar's description of a pass-through HDR10 swapchain's surface (see the module above).
pub struct HdrDescription {
    window: usize,
    surface_object: *mut Proxy,
    image: Option<Object<ImageData>>,
    feedback: Option<Object<ChangedData>>,
    manager: Option<Object<CmData>>,
    label: Option<HdrLabel>,
    preferred: DescriptionInfo,
    /// What the label was made for: the brightness, the gamma 2.2 switch and the content's peak.
    made_for: Option<(Brightness, bool, Option<u32>)>,
    refreshed: Instant,
    pumps: u32,
    /// A refresh whose answers the compositor has yet to send; the present goes on meanwhile.
    inflight: Option<Inflight>,
    conn: Conn,
}

/// A refresh under way: the compositor's answers arrive between frames, one step at a time
/// (`HdrDescription::advance`), so no round trip ever holds the present thread.
struct Inflight {
    started: Instant,
    /// What the new description is made for: the state at the start.
    made_for: (Brightness, bool, Option<u32>),
    step: Step,
}

enum Step {
    /// The compositor's preferred description was asked for; waiting for its ready.
    Preferred(Object<ImageData>),
    /// Its information was asked for; waiting for the done.
    Info { image: Object<ImageData>, info: Object<InfoData> },
    /// Ours was created from `info`'s luminances; waiting for its ready.
    Created { image: Object<ImageData>, label: HdrLabel, info: DescriptionInfo },
}

/// A refresh the compositor leaves unanswered this long is given up.
const REFRESH_TIMEOUT: Duration = Duration::from_secs(1);
/// Round trips the set-up waits for a refresh's answers at most (the steps need one each).
const SETUP_ROUND_TRIPS: usize = 12;

// SAFETY: libwayland's proxies may be used from any thread; the description is used from one
// at a time (behind INSTALLED's lock).
unsafe impl Send for HdrDescription {}

/// How often the compositor's preferred description is read again after it said it changed
/// (KWin says so on every step of a brightness animation).
const REFRESH_EVERY: Duration = Duration::from_millis(500);

/// What the description is made for now: the coming stream's brightness, the gamma 2.2 switch
/// and the host's content peak.
fn wanted() -> (Brightness, bool, Option<u32>) {
    (brightness(), env_on(GAMMA22_PRESENT_ENV), content_peak())
}

impl HdrDescription {
    /// Reads the compositor's preferred description for `window` and makes the matching one,
    /// ready to [`attach`](Self::attach); or why not.
    pub fn prepare(window: *mut SDL_Window) -> Result<Self, String> {
        let conn = Conn::open(window)?;
        let mut description = HdrDescription {
            window: window as usize,
            surface_object: null_mut(),
            image: None,
            feedback: None,
            manager: None,
            label: None,
            preferred: DescriptionInfo::default(),
            made_for: None,
            refreshed: Instant::now(),
            pumps: 0,
            inflight: None,
            conn,
        };
        // SAFETY: objects of our connection; a failure drops what was made (Drop).
        unsafe {
            description.manager = Some(manager(&description.conn)?);
            description.feedback = Some(color_feedback(&description.conn, description.manager.as_ref().unwrap())?);
            description.refresh_now()?;
        }
        Ok(description)
    }

    /// The first refresh, before the stream draws: waits for the compositor's answers.
    unsafe fn refresh_now(&mut self) -> Result<bool, String> {
        // SAFETY: objects of our connection.
        unsafe { self.begin() }?;
        for _ in 0..SETUP_ROUND_TRIPS {
            // SAFETY: as above.
            if let Some(done) = unsafe { self.advance() } {
                return done;
            }
            if !self.conn.roundtrip() {
                self.give_up();
                return Err("the Wayland round trip failed".into());
            }
        }
        Err(self.give_up())
    }

    /// Asks for the compositor's preferred description: the first step of a refresh, which
    /// `advance` carries on as the answers arrive.
    unsafe fn begin(&mut self) -> Result<(), String> {
        let Some(feedback) = &self.feedback else { return Err("no colour manager".into()) };
        // SAFETY: a live feedback object; get_preferred's "n".
        let proxy = unsafe { (self.conn.api.marshal_flags)(feedback.proxy, FEEDBACK_GET_PREFERRED, &IMAGE_IF.0, self.conn.version(feedback.proxy), 0, null_mut::<c_void>()) };
        let image = unsafe { Object::new(&self.conn, proxy, &IMAGE_LISTENER) }.ok_or("no image description")?;
        self.inflight = Some(Inflight { started: Instant::now(), made_for: wanted(), step: Step::Preferred(image) });
        self.conn.flush();
        Ok(())
    }

    /// Gives up the refresh under way: its objects go, and the reason is returned.
    fn give_up(&mut self) -> String {
        let Some(Inflight { step, .. }) = self.inflight.take() else { return "no refresh under way".into() };
        match step {
            Step::Preferred(image) => {
                self.conn.destroy(image.proxy, IMAGE_DESTROY);
                "the compositor did not answer for the image description".into()
            }
            Step::Info { image, info } => {
                // SAFETY: the info proxy of ours, destroyed once (its done ends it).
                unsafe { (self.conn.api.destroy)(info.proxy) };
                self.conn.destroy(image.proxy, IMAGE_DESTROY);
                "the compositor did not describe its preferred description".into()
            }
            Step::Created { image, .. } => {
                self.conn.destroy(image.proxy, IMAGE_DESTROY);
                "the compositor did not answer for the image description".into()
            }
        }
    }

    /// Takes the refresh under way one step on with what the compositor has answered so far;
    /// never waits. None while it is still under way, else how it ended: whether the
    /// description changed. A refresh that failed leaves the state it was made for in
    /// `made_for` (not again for it: the next output change tries again).
    unsafe fn advance(&mut self) -> Option<Result<bool, String>> {
        let inflight = self.inflight.as_ref()?;
        let made_for = inflight.made_for;
        if inflight.started.elapsed() > REFRESH_TIMEOUT {
            self.made_for = Some(made_for);
            return Some(Err(self.give_up()));
        }
        let Inflight { started, step, .. } = self.inflight.take()?;
        let wait = |step| Some(Inflight { started, made_for, step });
        let fail = |this: &mut Self, why: String| {
            this.made_for = Some(made_for);
            Some(Err(why))
        };
        match step {
            Step::Preferred(image) => match image.data.state.clone() {
                None => {
                    self.inflight = wait(Step::Preferred(image));
                    None
                }
                Some(Err(why)) => {
                    self.conn.destroy(image.proxy, IMAGE_DESTROY);
                    fail(self, why)
                }
                Some(Ok(())) => {
                    // SAFETY: a ready description; get_information's "n".
                    let proxy = unsafe { (self.conn.api.marshal_flags)(image.proxy, IMAGE_GET_INFORMATION, &INFO_IF.0, self.conn.version(image.proxy), 0, null_mut::<c_void>()) };
                    let Some(info) = (unsafe { Object::<InfoData>::new(&self.conn, proxy, &INFO_LISTENER) }) else {
                        self.conn.destroy(image.proxy, IMAGE_DESTROY);
                        return fail(self, "no description information".into());
                    };
                    self.conn.flush();
                    self.inflight = wait(Step::Info { image, info });
                    None
                }
            },
            Step::Info { image, info } => {
                if !info.data.done {
                    self.inflight = wait(Step::Info { image, info });
                    return None;
                }
                // The info object ends with its done event; the client only frees its proxy.
                // SAFETY: our proxy, destroyed once.
                unsafe { (self.conn.api.destroy)(info.proxy) };
                let described = info.data.info;
                let Some(manager) = &self.manager else {
                    self.conn.destroy(image.proxy, IMAGE_DESTROY);
                    return fail(self, "no colour manager".into());
                };
                let (brightness, gamma22, content_peak) = made_for;
                let label = match hdr_label(&manager.data.support, &described, brightness, gamma22, content_peak) {
                    Ok(label) => label,
                    Err(why) => {
                        self.conn.destroy(image.proxy, IMAGE_DESTROY);
                        return fail(self, why);
                    }
                };
                match label {
                    HdrLabel::Preferred | HdrLabel::Blending { .. } => Some(self.apply(image, label, described, made_for)),
                    HdrLabel::Parametric(params) | HdrLabel::Desktop(params) => {
                        self.conn.destroy(image.proxy, IMAGE_DESTROY);
                        if self.label == Some(label) && self.image.is_some() {
                            self.preferred = described;
                            self.made_for = Some(made_for);
                            return Some(Ok(false));
                        }
                        // SAFETY: a live manager whose features `hdr_label` checked.
                        match unsafe { create_pq(&self.conn, manager, params) } {
                            Ok(created) => {
                                self.conn.flush();
                                self.inflight = wait(Step::Created { image: created, label, info: described });
                                None
                            }
                            Err(why) => fail(self, why),
                        }
                    }
                }
            }
            Step::Created { image, label, info } => match image.data.state.clone() {
                None => {
                    self.inflight = wait(Step::Created { image, label, info });
                    None
                }
                Some(Err(why)) => {
                    self.conn.destroy(image.proxy, IMAGE_DESTROY);
                    fail(self, why)
                }
                Some(Ok(())) => Some(self.apply(image, label, info, made_for)),
            },
        }
    }

    /// Makes the ready description `image` ours, set on the surface when attached, with the
    /// present's encoding to match. Whether it changed (it did).
    fn apply(&mut self, image: Object<ImageData>, label: HdrLabel, info: DescriptionInfo, made_for: (Brightness, bool, Option<u32>)) -> Result<bool, String> {
        if !self.surface_object.is_null() {
            // The present's encoding and the description change together, from the next frame.
            self.encode(label);
            // SAFETY: our colour surface and a ready description; "ou".
            unsafe { (self.conn.api.marshal_flags)(self.surface_object, SURFACE_SET_IMAGE_DESCRIPTION, null(), self.conn.version(self.surface_object), 0, image.proxy, INTENT_PERCEPTUAL) };
            self.conn.flush();
        }
        if let Some(old) = self.image.replace(image) {
            self.conn.destroy(old.proxy, IMAGE_DESTROY);
        }
        self.label = Some(label);
        self.preferred = info;
        self.made_for = Some(made_for);
        if !self.surface_object.is_null() {
            set_label(Some(self.log_label()));
        }
        Ok(true)
    }

    /// Hands the window's renderer the encoding `label` asks of its present.
    fn encode(&self, label: HdrLabel) {
        // SAFETY: SDL's window lookup; the renderer's properties take any thread.
        let renderer = unsafe { SDL_GetRenderer(self.window as *mut SDL_Window) };
        crate::sdl::set_present_encoding(renderer, label.encoding());
    }

    /// Gives the surface the description, from its next commit (the swapchain's first
    /// present), and the window's renderer the encoding to match. Only on a pass-through
    /// swapchain: Mesa then has no colour surface on it.
    pub fn attach(&mut self) {
        let (Some(manager), Some(image), Some(label)) = (&self.manager, &self.image, self.label) else { return };
        if !self.surface_object.is_null() {
            return;
        }
        self.encode(label);
        // SAFETY: a live manager, SDL's surface and a ready description.
        unsafe {
            let version = self.conn.version(manager.proxy);
            self.surface_object = (self.conn.api.marshal_flags)(manager.proxy, CM_GET_SURFACE, &CM_SURFACE_IF.0, version, 0, null_mut::<c_void>(), self.conn.surface);
            if !self.surface_object.is_null() {
                (self.conn.api.marshal_flags)(self.surface_object, SURFACE_SET_IMAGE_DESCRIPTION, null(), version, 0, image.proxy, INTENT_PERCEPTUAL);
            }
        }
        self.conn.flush();
    }

    /// Follows the compositor's preferred description (another output, its brightness) and the
    /// stream's brightness; a line for the log when ours changed or could not.
    fn pump(&mut self) -> Option<String> {
        self.pumps = self.pumps.wrapping_add(1);
        // While a refresh waits for the compositor its answers are looked for on every call
        // (a read that finds nothing is cheap); otherwise only on every 8th.
        if self.inflight.is_none() && !self.pumps.is_multiple_of(8) {
            return None;
        }
        self.conn.pump();
        if self.inflight.is_some() {
            // SAFETY: objects of our connection.
            let ended = unsafe { self.advance() }?;
            return self.report(ended);
        }
        let feedback = self.feedback.as_mut()?;
        let changed = feedback.data();
        // SAFETY: the box, read between dispatches.
        let output_changed = unsafe { (*changed).changed } && self.refreshed.elapsed() >= REFRESH_EVERY;
        if !output_changed && self.made_for == Some(wanted()) {
            return None;
        }
        // SAFETY: as above.
        unsafe { (*changed).changed = false };
        self.refreshed = Instant::now();
        // SAFETY: objects of our connection.
        match unsafe { self.begin() } {
            Ok(()) => None,
            Err(why) => {
                // Not again for this state: the next output change tries again.
                self.made_for = Some(wanted());
                self.report(Err(why))
            }
        }
    }

    /// The log line for a refresh that ended.
    fn report(&self, ended: Result<bool, String>) -> Option<String> {
        match ended {
            Ok(true) => Some(format!("HDR description follows the output: {}", self.describe())),
            Ok(false) => None,
            Err(why) => Some(format!("HDR description stays {} ({why})", self.describe())),
        }
    }

    pub fn describe(&self) -> String {
        self.label.map_or_else(|| "none".into(), |label| describe_label(label, &self.preferred))
    }

    /// The description for the end-of-stream line.
    pub fn log_label(&self) -> Label {
        Label::Ours {
            text: self.describe(),
            blending: matches!(self.label, Some(HdrLabel::Blending { .. })),
            reference: self.label.map_or(PQ_DEFAULT_REFERENCE_NITS as u32, |label| label.reference(&self.preferred)),
        }
    }

    /// The reference white the description declares, in nits.
    fn reference(&self) -> Option<f32> {
        self.label.map(|label| label.reference(&self.preferred) as f32)
    }
}

impl Drop for HdrDescription {
    fn drop(&mut self) {
        // Destroying the colour surface unsets the description from the surface's next commit.
        self.conn.destroy(self.surface_object, SURFACE_DESTROY);
        self.give_up();
        if let Some(image) = self.image.take() {
            self.conn.destroy(image.proxy, IMAGE_DESTROY);
        }
        if let Some(feedback) = self.feedback.take() {
            self.conn.destroy(feedback.proxy, FEEDBACK_DESTROY);
        }
        if let Some(manager) = self.manager.take() {
            self.conn.destroy(manager.proxy, CM_DESTROY);
        }
        // `conn` flushes and goes after this.
    }
}

static INSTALLED: Mutex<Option<HdrDescription>> = Mutex::new(None);

/// Keeps `description` while its renderer lives.
pub fn install(description: HdrDescription) {
    *INSTALLED.lock().unwrap_or_else(|e| e.into_inner()) = Some(description);
}

/// Drops the description of `window`, if it has one: before any renderer is made for it.
pub fn uninstall(window: *mut SDL_Window) -> bool {
    let mut installed = INSTALLED.lock().unwrap_or_else(|e| e.into_inner());
    if installed.as_ref().is_some_and(|description| description.window == window as usize) {
        *installed = None;
        true
    } else {
        false
    }
}

/// The reference white Nightjar's description of `window` declares (nits), Some(None) without
/// one. Never waits for the drawing thread: None while it holds the description.
pub fn installed_reference(window: *mut SDL_Window) -> Option<Option<f32>> {
    let installed = INSTALLED.try_lock().ok()?;
    Some(installed.as_ref().filter(|description| description.window == window as usize).and_then(HdrDescription::reference))
}

/// Lets the installed description follow the output, from whichever thread draws (never
/// waiting for the other).
pub fn pump_installed() {
    let Ok(mut installed) = INSTALLED.try_lock() else { return };
    if let Some(line) = installed.as_mut().and_then(HdrDescription::pump) {
        eprintln!("nightjar: {line}");
    }
}

/// The stream's look at the compositor: its globals, the preferred description, and the
/// surface's dmabuf feedback over the stream.
pub struct Probe {
    seen: Seen,
    dmabuf: *mut Proxy,
    feedback: Option<Object<DmabufData>>,
    polls: u32,
    conn: Conn,
}

// SAFETY: as for HdrDescription; used from the presenting thread.
unsafe impl Send for Probe {}

impl Probe {
    pub fn open(window: *mut SDL_Window) -> Result<Self, String> {
        let conn = Conn::open(window)?;
        let mut probe = Probe { seen: Seen::default(), dmabuf: null_mut(), feedback: None, polls: 0, conn };
        for (_, interface, version) in probe.conn.globals() {
            probe.seen.globals.note(interface, *version);
        }
        // SAFETY: objects of our connection, destroyed in Drop.
        unsafe {
            probe.seen.preferred = probe.read_preferred();
            if probe.seen.globals.dmabuf >= 4
                && let Some(dmabuf) = probe.conn.bind(&DMABUF_IF.0, 4)
            {
                probe.dmabuf = dmabuf;
                probe.conn.listen(dmabuf, &DMABUF_LISTENER, null_mut());
                let proxy = (probe.conn.api.marshal_flags)(dmabuf, DMABUF_GET_SURFACE_FEEDBACK, &DMABUF_FEEDBACK_IF.0, 4, 0, null_mut::<c_void>(), probe.conn.surface);
                probe.feedback = Object::new(&probe.conn, proxy, &DMABUF_FEEDBACK_LISTENER);
                probe.conn.roundtrip();
            }
        }
        probe.sync();
        Ok(probe)
    }

    /// The preferred description, through objects made and dropped here.
    unsafe fn read_preferred(&self) -> Result<DescriptionInfo, String> {
        let manager = unsafe { manager(&self.conn) }?;
        let read = unsafe { color_feedback(&self.conn, &manager) }.and_then(|feedback| {
            let answer = unsafe { preferred(&self.conn, &feedback) };
            self.conn.destroy(feedback.proxy, FEEDBACK_DESTROY);
            answer.map(|(image, info)| {
                self.conn.destroy(image.proxy, IMAGE_DESTROY);
                info
            })
        });
        self.conn.destroy(manager.proxy, CM_DESTROY);
        read
    }

    fn sync(&mut self) {
        if let Some(feedback) = &self.feedback {
            self.seen.dmabuf = feedback.data.log.clone();
        }
    }

    /// Reads what arrived, every 16th call (a poll per call would be wasted).
    pub fn poll(&mut self) {
        self.polls = self.polls.wrapping_add(1);
        if self.polls.is_multiple_of(16) {
            self.conn.pump();
            self.sync();
        }
    }

    /// What was seen, up to now.
    pub fn seen(&mut self) -> &Seen {
        self.conn.pump();
        self.sync();
        &self.seen
    }
}

impl Drop for Probe {
    fn drop(&mut self) {
        if let Some(feedback) = self.feedback.take() {
            self.conn.destroy(feedback.proxy, DMABUF_FEEDBACK_DESTROY);
        }
        self.conn.destroy(self.dmabuf, DMABUF_DESTROY);
    }
}
