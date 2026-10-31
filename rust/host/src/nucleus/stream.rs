//! A co-op player's stream: its slot, its input and feedback, its game window
//! and audio, and the watch that ends it with its session.
use super::*;

/// A co-op launch's slot, shared by every copy of the launch
/// (`Launch::held`): the last copy dropped gives it back with
/// [`Registry::leave`]. That is an expired, replaced or failed launch at
/// once, a started one with its stream's session.
pub struct Held {
    host: Weak<Host>,
    seat: Seat,
    launch: String,
}
impl Held {
    pub fn new(host: &Shared, seat: Seat, launch: &str) -> Self {
        Self {
            host: Arc::downgrade(host),
            seat,
            launch: launch.into(),
        }
    }
}
impl Drop for Held {
    fn drop(&mut self) {
        if let Some(h) = self.host.upgrade() {
            h.nucleus.leave(self.seat, &self.launch);
        }
    }
}

/// End the streams of co-op session `session`, only `device`'s when given:
/// pending launches are removed, running streams stop. The removed launches
/// are dropped after the sessions lock, so their slots are given back
/// outside it.
pub fn end_streams(h: &Host, session: u32, device: Option<&str>) {
    let canceled: Vec<_> = {
        let mut sessions = h.sessions.lock().unwrap();
        let matches = |launch: &Launch| {
            launch.nucleus.is_some_and(|seat| seat.session == session)
                && device.is_none_or(|device| launch.client.uuid == device)
        };
        let ids: Vec<String> = sessions
            .pending
            .values()
            .filter(|launch| matches(launch))
            .chain(
                sessions
                    .active
                    .values()
                    .map(|s| &s.launch)
                    .filter(|launch| matches(launch)),
            )
            .map(|launch| launch.id.clone())
            .collect();
        ids.iter()
            .flat_map(|id| sessions.request_stop(Some(id)))
            .collect()
    };
    if !canceled.is_empty() {
        tracing::info!(
            session,
            launches = canceled.len(),
            "Nucleus Co-op: pending launches canceled"
        );
    }
    drop(canceled);
}

/// What a player's stream says when its session ended.
fn ended_text(session: &coop::Session, end: &End) -> String {
    let (title, what) = (
        &session.title,
        match session.kind {
            Kind::Coop => "co-op session",
            Kind::Background => "background stream",
        },
    );
    match end {
        End::Ended => format!("The {title} {what} was ended on the PC"),
        end => format!("The {title} {what} ended: {end}"),
    }
}

/// A co-op player's input and feedback. Its first controller drives its
/// slot's pad and nothing else: no event reaches the PC's desktop or a pad
/// of its own.
pub struct Player {
    pub(super) instance: Arc<dyn Instance>,
    pub(super) seat: Seat,
    pub(super) device: String,
    /// The rumble setting of the player's stream.
    pub(super) rumble: bool,
    /// The kinds of dropped input the player was told about.
    pub(super) told: Vec<Dropped>,
}
impl Player {
    /// The player `launch` streams; None for any other launch, or once its
    /// session is gone.
    pub fn of(h: &Host, launch: &Launch, config: &Config) -> Option<Self> {
        let seat = launch.nucleus?;
        Some(Self {
            instance: h.nucleus.instance(seat.session)?,
            seat,
            device: launch.client.uuid.clone(),
            rumble: Policy::resolve(config).is_ok_and(|policy| policy.forward_rumble),
            told: Vec::new(),
        })
    }
    /// Drive the slot's pad with `event`. What the pad cannot take is
    /// dropped, and the stream says so once per kind.
    pub fn input(&mut self, event: Input, warnings: &Warnings) {
        let Err(dropped) = self.instance.input(&self.device, event) else {
            return;
        };
        let Some(limit) = dropped.limit() else {
            return;
        };
        if !self.told.contains(&dropped) {
            self.told.push(dropped);
            tracing::info!(
                session = self.seat.session,
                slot = self.seat.slot,
                "Nucleus Co-op: {limit}"
            );
            warnings.set(dropped_code(dropped), limit);
        }
    }
    /// The slot's rumble and other feedback, for the player's controller 0.
    pub fn feedback(&self, h: &Host) -> Vec<PadReport> {
        h.nucleus.feedback(self.seat)
    }
    /// Whether feedback of `kind` goes to the device: rumble only where
    /// the stream's settings forward it.
    pub fn feedback_allowed(&self, kind: u16) -> bool {
        !matches!(kind, 0x010b | 0x5500 | 0x5503 | 0x5601) || self.rumble
    }
}
fn dropped_code(dropped: Dropped) -> &'static str {
    match dropped {
        Dropped::KeyboardMouse => "coop_keyboard_mouse",
        _ => "coop_second_controller",
    }
}

/// Ends a co-op player's stream once its session ends: a crash, a failed
/// start, the console's End or every player gone. It runs beside the
/// stream, never on it, and only for co-op launches.
pub struct Watch {
    stop: Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Watch {
    /// None for a launch that is not a co-op player's.
    pub fn start(h: &Shared, s: &Arc<crate::state::Session>) -> Result<Option<Self>> {
        use std::sync::atomic::{AtomicBool, Ordering};
        let Some(seat) = s.launch.nucleus else {
            return Ok(None);
        };
        let stop = Arc::new(AtomicBool::new(false));
        let thread = std::thread::Builder::new().name("nucleus stream".into()).spawn({
            let (h, s, stop) = (h.clone(), s.clone(), stop.clone());
            move || {
                while !stop.load(Ordering::Acquire) && !s.stopping() {
                    let end = match h.nucleus.instance(seat.session) {
                        Some(instance) => instance
                            .end()
                            .map(|end| (instance.session(|s| ended_text(s, &end)), end)),
                        None => Some(("The Nucleus Co-op session is gone".into(), End::Ended)),
                    };
                    if let Some((text, end)) = end {
                        tracing::warn!(session = seat.session, slot = seat.slot, client = %s.launch.client.name, "Nucleus Co-op: {text}");
                        s.launch.warnings.set("coop_ended", &text);
                        match end {
                            End::Crashed(_) | End::Failed(_) => s.fail(),
                            End::Ended | End::Empty => s.stop(),
                        }
                        return;
                    }
                    std::thread::park_timeout(WATCH);
                }
            }
        })?;
        Ok(Some(Self {
            stop,
            thread: Some(thread),
        }))
    }
}
impl Drop for Watch {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Release);
        if let Some(thread) = self.thread.take() {
            thread.thread().unpark();
            if thread.join().is_err() {
                tracing::error!("the Nucleus Co-op stream watch panicked");
            }
        }
    }
}

/// The stream of a co-op launch: its player's game window at the stream's
/// size, once Nucleus Co-op shows it within `wait`. No display is prepared,
/// changed or kept. The capture follows a window Nucleus replaces; a window
/// that is gone is said loudly and never shown as another window or a
/// display.
pub fn prepare(
    h: &Shared,
    launch: &Launch,
    stream: &butterpollo_core::rtsp::Negotiated,
    config: &Config,
    wait: Duration,
) -> Result<crate::display_session::StreamPreparation> {
    let seat = launch
        .nucleus
        .context("the launch has no Nucleus Co-op slot")?;
    let instance = h
        .nucleus
        .instance(seat.session)
        .ok_or_else(|| Refusal::unavailable("The Nucleus Co-op session ended: launch it again"))?;
    instance.resize(seat.slot, (stream.width, stream.height))?;
    // The display only picks the GPU the window is captured on.
    let display = butterpollo_windows::capture::primary_display()
        .context("no display is on the desktop to capture the game window on")?;
    let first = first_window(instance.as_ref(), seat.slot, &display, wait)?;
    tracing::info!(session = seat.session, slot = seat.slot, target = %first, "Nucleus Co-op: streaming the player's game window");
    warn_audio(
        butterpollo_windows::display::windows_build(),
        &launch.warnings,
    );
    crate::display_session::Ready::window(
        h,
        stream,
        config,
        first,
        Box::new(move || instance.capture_target(seat.slot, &display)),
        launch.warnings.clone(),
    )
}

/// The game audio a co-op player hears: its own slot's instance.
pub fn audio(h: &Host, seat: Seat) -> Result<AudioProcess> {
    h.nucleus
        .instance(seat.session)
        .context("the co-op session ended")?
        .audio(seat.slot)
}

/// The player's window follows the stream's new size.
pub fn resize(h: &Host, seat: Seat, size: (u32, u32)) -> Result<()> {
    h.nucleus
        .instance(seat.session)
        .context("the co-op session ended")?
        .resize(seat.slot, size)
}

/// Below the build with process loopback capture, the stream says it has
/// no audio of the player's own game.
pub(super) fn warn_audio(build: Option<u32>, warnings: &Warnings) {
    if let Some(limit) = butterpollo_windows::nucleus::session::audio_limit(build) {
        warnings.set("coop_audio", limit);
    }
}

/// The setting [`isolate`] marks a co-op player's stream with.
pub const PLAYER: &str = "nucleus_player";
/// A co-op player shares the PC with its user: the stream drives no
/// keyboard or mouse of the desktop, captures only its game's audio, moves
/// no window into focus and leaves the clipboard alone ([`player`]).
pub fn isolate(config: &mut Config) {
    config.values.insert(PLAYER.into(), "true".into());
    for (key, value) in butterpollo_core::background::FORCED {
        config.values.insert(key.into(), value.into());
    }
}
pub fn player(config: &Config) -> bool {
    config.boolean(PLAYER, false)
}

/// Slot `slot`'s first capture target, waiting up to `wait` for its window.
pub(super) fn first_window(
    instance: &dyn Instance,
    slot: usize,
    display: &str,
    wait: Duration,
) -> Result<String> {
    let deadline = Instant::now() + wait;
    loop {
        if let Some(end) = instance.end() {
            let text = instance.session(|s| ended_text(s, &end));
            return Err(Refusal::unavailable(text).into());
        }
        match instance.capture_target(slot, display) {
            Ok(target) => return Ok(target),
            Err(error) if Instant::now() >= deadline => {
                return Err(Refusal::unavailable(format!(
                    "{error:#}: Nucleus Co-op showed no window for Player {} within {} s",
                    slot + 1,
                    wait.as_secs()
                ))
                .into());
            }
            Err(_) => std::thread::sleep(WATCH),
        }
    }
}
