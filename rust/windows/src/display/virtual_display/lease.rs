//! Leasing: one display per id shared by every stream on it, joining one
//! created at another mode, raising its refresh, and turning it with a stream.
use super::*;

pub(in crate::display) type DisplayLease = std::sync::Arc<std::sync::Mutex<VirtualDisplay>>;
/// The displays streams lease, by id; a shared display has one entry for all.
pub(super) type Leases<D> =
    std::collections::BTreeMap<String, std::sync::Weak<std::sync::Mutex<D>>>;
static DISPLAYS: std::sync::Mutex<Leases<VirtualDisplay>> =
    std::sync::Mutex::new(std::collections::BTreeMap::new());
/// The display `id` for a stream at this mode, and the refresh (millihertz)
/// it still has to be raised to when the stream joins a slower shared one:
/// [`joined_monitor`] does that once the display is on.
pub(in crate::display) fn display_lease(
    id: &str,
    width: u32,
    height: u32,
    fps: u32,
    options: &VirtualOptions,
) -> Result<(DisplayLease, Option<u32>)> {
    lease(
        &mut DISPLAYS.lock().unwrap(),
        id,
        (width, height, fps),
        || VirtualDisplay::create_options(id, width, height, fps, options.clone()),
    )
}
/// What leasing, joining and raising a virtual display need of it, so that
/// tests can stand in for the driver and Windows.
pub(in crate::display) trait SharedDisplay {
    /// The mode it was created with, and the one the host sets on it now.
    fn modes(&self) -> (Timing, Timing);
    /// The mode the host sets on it from now on.
    fn set_mode(&mut self, mode: Timing);
    /// Have the driver offer that mode; false, logged, when the driver refused.
    fn offer_mode(&self) -> bool;
    /// The modes the driver lists for it: the one it was created with, and
    /// those it offered since (its list is a union).
    fn offered(&mut self) -> &mut Vec<Timing>;
    /// The refreshes (whole hertz) Windows lists now at that mode's size.
    fn listed_rates(&self) -> Vec<u32>;
    /// Set that mode in Windows; whether Windows shows it.
    fn apply(&self, stage: &str) -> bool;
    /// That mode's size, when Windows showed it after it was last set.
    fn applied_size(&self) -> Option<(u32, u32)>;
    /// Its monitor, switched back on when Windows left it off.
    fn switch_on(&mut self) -> Result<Monitor>;
}
impl SharedDisplay for VirtualDisplay {
    fn modes(&self) -> (Timing, Timing) {
        (self.created, self.mode)
    }
    fn set_mode(&mut self, mode: Timing) {
        self.mode = mode;
    }
    fn offer_mode(&self) -> bool {
        match self.driver.update_modes(self.id, self.mode) {
            Ok(()) => true,
            Err(error) => {
                tracing::warn!(output = %self.name, error = %format!("{error:#}"), mode = %format_timing(self.mode), "the display driver did not offer the mode");
                false
            }
        }
    }
    fn offered(&mut self) -> &mut Vec<Timing> {
        &mut self.offered
    }
    fn listed_rates(&self) -> Vec<u32> {
        offered_rates(&supported_modes(&self.name), self.mode.0, self.mode.1)
    }
    fn apply(&self, stage: &str) -> bool {
        self.apply_mode(stage)
    }
    fn applied_size(&self) -> Option<(u32, u32)> {
        VirtualDisplay::applied_size(self)
    }
    fn switch_on(&mut self) -> Result<Monitor> {
        let monitor = self.current_monitor()?;
        take_name(&mut self.name, &mut self.generation, &monitor.display_name);
        Ok(monitor)
    }
}
/// Take the name Windows gave a display it switched back on. A new name is a
/// new generation, as after a heartbeat switched it on: the guards of the
/// streams on it, a paused one too, reapply their layout and capture there.
/// A guard made after this reads the new generation, so it recovers nothing.
pub(super) fn take_name(name: &mut String, generation: &mut u64, listed: &str) {
    if name != listed {
        *name = listed.into();
        *generation = generation.wrapping_add(1);
    }
}
pub(super) fn lease<D: SharedDisplay>(
    displays: &mut Leases<D>,
    id: &str,
    requested: Timing,
    create: impl FnOnce() -> Result<D>,
) -> Result<(std::sync::Arc<std::sync::Mutex<D>>, Option<u32>)> {
    displays.retain(|_, lease| lease.strong_count() != 0);
    if let Some(display) = displays.get(id).and_then(std::sync::Weak::upgrade) {
        let (created, current) = display.lock().unwrap().modes();
        let needed = join(created, current, requested)?;
        return Ok((display, needed));
    }
    let display = std::sync::Arc::new(std::sync::Mutex::new(create()?));
    displays.insert(id.into(), std::sync::Arc::downgrade(&display));
    Ok((display, None))
}
/// The monitor of a leased display, switched back on first (a paused shared
/// display can be off), then raised to `needed`, the refresh a joining stream
/// asks for above the display's ([`display_lease`]). A raise Windows refuses
/// fails with why; the display keeps its mode for the streams on it.
pub(in crate::display) fn joined_monitor(
    display: &mut impl SharedDisplay,
    needed: Option<u32>,
) -> Result<Monitor> {
    let monitor = display.switch_on()?;
    if let Some(rate) = needed {
        let (_, before) = display.modes();
        if raise(display, rate).is_none() {
            bail!(
                "shared virtual display already uses a different mode: it runs at {:.3} Hz, and Windows refused the {:.3} Hz this stream asks for",
                f64::from(before.2) / 1000.,
                f64::from(rate) / 1000.
            );
        }
        tracing::info!(
            from = %format_timing(before),
            to_millihz = rate,
            "a joining stream raised the shared virtual display's refresh"
        );
    }
    Ok(monitor)
}
/// Raise a leased display's refresh in place for a stream that asks for more
/// than its launch did, keeping the display and what runs on it ([`raise`]).
/// A shared display too: a faster refresh serves the streams on it.
pub(in crate::display) fn raise_lease(
    display: &std::sync::Mutex<impl SharedDisplay>,
    rate: u32,
) -> Option<u32> {
    raise(&mut *display.lock().unwrap(), rate)
}
/// Raise the refresh in place to at least `rate`: the refresh (millihertz)
/// the display has now, or None when Windows refused it. A refusal puts the
/// previous mode back, since Windows can refuse the rate yet leave another
/// rate or size behind for the streams already on it. A rate Windows does not
/// list yet is not set: it would refuse it as a bad mode.
fn raise(display: &mut impl SharedDisplay, rate: u32) -> Option<u32> {
    let (_, previous) = display.modes();
    if rate <= previous.2 {
        return Some(previous.2);
    }
    display.set_mode((previous.0, previous.1, rate));
    if offer_listed(display, None) && display.apply("stream request") {
        return Some(rate);
    }
    display.set_mode(previous);
    display.apply("stream raise refused");
    None
}
/// Give the display another size at its refresh in place, for a stream that
/// turned between portrait and landscape: whether Windows shows that size,
/// at that refresh or a slower one it lists ([`fallback_rate`]). Windows can
/// list the size already (the driver's standard sizes, or an earlier turn) at
/// other refreshes, so the turn waits for its refresh or for the list Windows
/// shows at that size to change: the driver lists a size's refreshes at once.
/// Either way it is set then, and a slower listed refresh serves it. When
/// Windows does not show the size, the display goes back to its previous mode.
pub(super) fn reshape(display: &mut impl SharedDisplay, width: u32, height: u32) -> bool {
    let (_, previous) = display.modes();
    display.set_mode((width, height, previous.2));
    let before = display.listed_rates();
    offer_listed(display, Some(&before));
    display.apply("stream turned");
    if display.applied_size() == Some((width, height)) {
        return true;
    }
    display.set_mode(previous);
    display.apply("stream turn refused");
    false
}
/// Have the driver offer the display's mode on the live display, and wait
/// until Windows lists it at its refresh, or for a turn until the refreshes
/// Windows lists at its size differ from `turn_from`: the driver publishes
/// the new list after it answers (16 ms later in the S1a run), and Windows
/// refuses a mode it does not list yet (DISP_CHANGE_BADMODE). A mode already
/// listed costs one look. A driver that refused, or that offered the mode
/// before ([`SharedDisplay::offered`]), is not waited for: Windows' list does
/// not change then, and a turn back to a size Windows filters that refresh at
/// would stall every time. False, with why, when Windows does not list it.
fn offer_listed(display: &mut impl SharedDisplay, turn_from: Option<&[u32]>) -> bool {
    let (_, mode) = display.modes();
    let repeated = display.offered().contains(&mode);
    let accepted = display.offer_mode();
    if accepted && !repeated {
        display.offered().push(mode);
    }
    let display = &*display;
    let turn = turn_from.is_some();
    let listed = || {
        let rates = display.listed_rates();
        lists_rate(&rates, mode.2)
            || turn_from.is_some_and(|before| !rates.is_empty() && rates != before)
    };
    if !accepted || repeated {
        return listed();
    }
    let started = Instant::now();
    match wait_listed(listed, MODE_LIST_WAIT) {
        Some(1) => true,
        Some(polls) => {
            tracing::info!(
                mode = %format_timing(mode),
                turn,
                polls,
                waited_ms = started.elapsed().as_millis() as u64,
                "Windows listed the mode the display driver offers"
            );
            true
        }
        None => {
            let listed_hz = display.listed_rates();
            let message = if turn {
                "Windows does not list the mode the display driver was asked to offer; the turn takes a refresh Windows lists at that size"
            } else {
                "Windows does not list the mode the display driver was asked to offer; the display keeps its mode"
            };
            tracing::warn!(
                mode = %format_timing(mode),
                turn,
                waited_ms = started.elapsed().as_millis() as u64,
                ?listed_hz,
                "{message}"
            );
            false
        }
    }
}
/// A stream joins a shared virtual display that was created at the size it
/// asks for, or that shows that size now: a turn or a size the user picked in
/// Windows moves `current`, and the encoder scales as it does on a shared
/// physical display. Any refresh at least the stream's serves it, since
/// capture and encode are paced per stream: a VRR stream's 1000 Hz display
/// serves a fixed 60 fps one. A stream that asks for more gets the refresh
/// the display has to be raised to (for the streams on it, too, a faster
/// refresh serves them). Another size fails with why.
pub(super) fn join(created: Timing, current: Timing, requested: Timing) -> Result<Option<u32>> {
    let (width, height, rate) = requested;
    if (width, height) != (created.0, created.1) && (width, height) != (current.0, current.1) {
        bail!(
            "shared virtual display already uses a different mode: it is {}x{}, and this stream asks for {width}x{height}",
            current.0,
            current.1
        );
    }
    Ok((rate > current.2).then_some(rate))
}
