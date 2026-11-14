//! What a whole picture under VRR needs besides the beam (`VrrSync::Scan`, Windows): the present
//! must be the only thing left between the picture and the screen.
//!
//! `Streaming::wait_out_scan` waits for the beam to leave the picture's last row and then presents
//! with tearing, into the blanking. Read at the present alone, the beam says too little:
//!
//! - **The flip waits for the picture's drawing.** A present goes to the screen once the GPU has
//!   done the work queued ahead of it. A stream's log from a handheld's integrated GPU (HEVC 4:4:4
//!   at 1080p and 120 fps) put the scan a present starts a median 4.6 ms after the call, up to
//!   19.8 ms. So a picture that has to wait has its drawing finished meanwhile (`finish_drawing`),
//!   and the flip is what remains when it goes out. One that meets an idle display goes out at
//!   once: a late flip starts that display's scan late, and tears nothing.
//! - **A flip on its way starts a scan.** While the present before this one has not reached the
//!   screen, a variable refresh display idles and its beam reads as past the last row. A second
//!   present sent then lands in the scan the first one starts, and tears with the display read as
//!   idle. It happens when pictures come back to back: a stream at the display's top rate leaves
//!   the display 0.3 ms between scans at 120 Hz, and a burst from a Wi-Fi link does the rest. So a
//!   present waits for the one before it to start its scan (`on_its_way`), and then for that scan.
//!
//! The counts and times go to the log at the stream's end (`Tally::lines`).

/// How long a present waits for the one before it to start its scan (ns), counted from that
/// present: `scanline::RESTART_WINDOW_NS`, the longest a flip took on the test monitor plus 1 ms.
/// A flip later than this is not waited for, and neither is one that tore into a scan and so
/// started none.
pub const LANDING_LIMIT_NS: u64 = super::scanline::RESTART_WINDOW_NS;

/// How long the drawing of a picture is waited for (ns) before its present goes out regardless:
/// two pictures of a 60 fps stream.
pub const DRAWING_LIMIT_NS: u64 = 33_000_000;

/// Presents in a row whose predecessor started no scan in time before the wait for it rests:
/// the display does not scan on this window's presents, and each wait would cost up to
/// `LANDING_LIMIT_NS` for nothing.
pub const GIVE_UPS_IN_A_ROW: u32 = 3;

/// Presents the wait rests for after that, before it is tried again.
pub const REST_PRESENTS: u32 = 300;

/// How far past the first reading's estimate the wait for a scan under way may run (ns) before
/// the present goes out regardless: a reading can be wrong (a mode switch, a wrapped counter),
/// and a sleep can overshoot. The wait is at most one scan plus this.
pub const SCAN_WAIT_SLACK_NS: u64 = 1_000_000;

/// Waits for a scan under way longer than this sleep most of it (ns)...
const SLEEP_FROM_NS: u64 = 500_000;
/// ...and spin the last of it, reading the beam, since a sleep cannot be that exact.
const SPIN_LAST_NS: u64 = 250_000;

/// What the wait for a scan under way does next (`Streaming::wait_out_scan`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScanStep {
    /// The beam has left the picture's last row: present.
    Go,
    /// Sleep this long (ns), then read the beam again.
    Sleep(u64),
    /// Read the beam again at once.
    Spin,
    /// The wait reached its limit with the beam still in the picture's rows, or the scan line
    /// could not be read: present anyway, counted (`Tally::cut_short`).
    CutShort,
}

/// The next step of a wait for a scan under way, from the latest reading (`left_ns`: how long the
/// beam takes to leave the last row, None once it has; `Beam::scan_left_ns`), `read` false when
/// the scan line gave no reading, the time waited so far and the wait's limit. The beam is read
/// again after every sleep, so a wait follows the scan to its end instead of trusting one reading.
/// Nightjar 5.4.1 capped this wait at 2 ms, which let a picture that met a scan under way on a
/// 120 Hz panel (8 ms a scan) go out into it and tear (Nightjar issue 111).
pub fn scan_step(left_ns: Option<u64>, read: bool, waited_ns: u64, limit_ns: u64) -> ScanStep {
    if !read { return ScanStep::CutShort; }
    let Some(left) = left_ns else { return ScanStep::Go };
    if waited_ns >= limit_ns { return ScanStep::CutShort; }
    if left > SLEEP_FROM_NS { ScanStep::Sleep((left - SPIN_LAST_NS).min(limit_ns - waited_ns)) } else { ScanStep::Spin }
}

/// Whether the present issued `since_present_ns` ago has not started a scan yet, from a reading
/// of an idling display whose last scan began `since_scan_ns` ago (`Beam::since_scan_ns`; the
/// counter keeps walking while the display idles). A present is issued when the beam has left the
/// last row, a whole `scan_ns` after its scan began, so the scan seen is the old one when it began
/// that much before the present and the new one when it began after it: half a scan tells them
/// apart with room on both sides.
pub fn on_its_way(since_scan_ns: u64, since_present_ns: u64, scan_ns: u64) -> bool {
    since_scan_ns > since_present_ns.saturating_add(scan_ns / 2)
}

/// How long the present issued `since_present_ns` ago took to start the scan that began
/// `since_scan_ns` ago (ns). None when that scan is not the present's: it began before the
/// present, or so long after it that the display refreshed by itself in between.
pub fn landing_ns(since_scan_ns: u64, since_present_ns: u64) -> Option<u64> {
    let took = since_present_ns.checked_sub(since_scan_ns)?;
    (took <= LANDING_LIMIT_NS).then_some(took)
}

/// Times in 0.1 ms steps up to 20 ms, for a median and a "19 of 20" without keeping every one.
#[derive(Clone, Debug)]
pub struct Times {
    steps: [u32; Self::STEPS],
    count: u32,
}

impl Default for Times {
    fn default() -> Self { Self { steps: [0; Self::STEPS], count: 0 } }
}

impl Times {
    const STEPS: usize = 200;
    const STEP_NS: u64 = 100_000;

    pub fn push(&mut self, ns: u64) {
        let step = ((ns / Self::STEP_NS) as usize).min(Self::STEPS - 1);
        self.steps[step] += 1;
        self.count += 1;
    }

    pub fn count(&self) -> u32 { self.count }

    /// The time `share` of them stayed within (ms), to the step: the step's upper edge, so a time
    /// is never shown shorter than it was. 0 when there are none.
    pub fn within_ms(&self, share: f64) -> f64 {
        if self.count == 0 { return 0.0; }
        let wanted = (f64::from(self.count) * share).ceil().max(1.0) as u32;
        let mut seen = 0;
        for (step, count) in self.steps.iter().enumerate() {
            seen += count;
            if seen >= wanted { return (step + 1) as f64 * Self::STEP_NS as f64 / 1e6; }
        }
        Self::STEPS as f64 * Self::STEP_NS as f64 / 1e6
    }

    fn line(&self) -> String {
        format!("{:.1} ms at the median, 19 of 20 within {:.1} ms", self.within_ms(0.5), self.within_ms(0.95))
    }
}

/// What the presents of a `VrrSync::Scan` stream met, for the log.
#[derive(Clone, Debug, Default)]
pub struct Tally {
    /// How long the drawing of each picture took to finish after it was asked for.
    pub drawing: Times,
    /// Pictures whose drawing was not finished within `DRAWING_LIMIT_NS`, or could not be asked.
    pub undrawn: u32,
    /// How long a present took to start its scan, where the next present could tell.
    pub landing: Times,
    /// Presents that found the one before still on its way.
    pub behind: u32,
    /// Of those, the ones that waited for it, and how long in all (ns).
    pub waited: (u32, u64),
    /// Presents whose predecessor started no scan within `LANDING_LIMIT_NS`.
    pub gave_up: u32,
    /// Those in a row, and the presents the wait still rests for.
    row: u32,
    rest: u32,
    /// Times the wait went to rest.
    pub rests: u32,
    /// Presents that waited for a scan under way and went out before it ended (`ScanStep::CutShort`).
    pub cut_short: u32,
}

impl Tally {
    /// Whether this present waits for the one before it: not while the wait rests. Counts the
    /// present off the rest.
    pub fn waits(&mut self) -> bool {
        if self.rest == 0 { return true; }
        self.rest -= 1;
        false
    }

    /// A present waited `ns` for the one before it, which then started its scan (`landed`) or did
    /// not in time. Returns true when the wait goes to rest with this one.
    pub fn waited_for(&mut self, ns: u64, landed: bool) -> bool {
        self.waited.0 += 1;
        self.waited.1 += ns;
        if landed {
            self.row = 0;
            return false;
        }
        self.gave_up += 1;
        self.row += 1;
        if self.row < GIVE_UPS_IN_A_ROW { return false; }
        self.row = 0;
        self.rest = REST_PRESENTS;
        self.rests += 1;
        true
    }

    /// A present found the one before it on the screen: the give-ups were not in a row.
    pub fn found_landed(&mut self) { self.row = 0; }

    /// The log's lines at the stream's end, none for a stream that never got this far.
    pub fn lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        if self.cut_short > 0 {
            lines.push(format!("VRR: {} presents went out before the scan under way had ended (the wait reached its limit or the scan line could not be read)", self.cut_short));
        }
        if self.drawing.count() > 0 {
            lines.push(format!("VRR: a picture's drawing was finished before its present, after {} ({} not waited for)",
                self.drawing.line(), self.undrawn));
        } else if self.undrawn > 0 {
            lines.push(format!("VRR: the drawing of {} pictures could not be waited for before their presents", self.undrawn));
        }
        if self.landing.count() > 0 {
            lines.push(format!("VRR: a present started its scan after {} ({} timed)", self.landing.line(), self.landing.count()));
        }
        if self.behind > 0 {
            let each = if self.waited.0 > 0 { self.waited.1 as f64 / f64::from(self.waited.0) / 1e6 } else { 0.0 };
            lines.push(format!(
                "VRR: {} presents found the one before still on its way to the screen; {} waited for it, {each:.2} ms each, {} of them for nothing ({} rests of {REST_PRESENTS} presents)",
                self.behind, self.waited.0, self.gave_up, self.rests));
        }
        lines
    }
}

/// Waits for the GPU to finish what this thread has drawn (a D3D11 event query behind the draws),
/// so the present that follows is all that is left before the picture can be shown. Returns how
/// long that took, None when it could not be asked or took longer than `limit`.
///
/// # Safety
/// `renderer` is the live renderer of this thread.
#[cfg(windows)]
pub unsafe fn finish_drawing(renderer: *mut sdl3_sys::everything::SDL_Renderer, limit: std::time::Duration) -> Option<std::time::Duration> {
    use sdl3_sys::everything::{SDL_FlushRenderer, SDL_PROP_RENDERER_D3D11_DEVICE_POINTER};
    use windows::core::Interface;
    use windows::Win32::Graphics::Direct3D11::{ID3D11Device, D3D11_QUERY_DESC, D3D11_QUERY_EVENT};
    let device = crate::sdl::renderer_pointer(renderer, SDL_PROP_RENDERER_D3D11_DEVICE_POINTER);
    let device = ID3D11Device::from_raw_borrowed(&device)?;
    // SDL's own draws reach the context first.
    if !SDL_FlushRenderer(renderer) { return None; }
    let mut query = None;
    device.CreateQuery(&D3D11_QUERY_DESC { Query: D3D11_QUERY_EVENT, MiscFlags: 0 }, Some(&mut query)).ok()?;
    let query = query?;
    let context = device.GetImmediateContext().ok()?;
    let begun = std::time::Instant::now();
    context.End(&query);
    context.Flush();
    // An event query's data is a BOOL, true once everything before it is done; until then the
    // call succeeds (S_FALSE) and leaves it alone.
    let mut done = 0i32;
    loop {
        if context.GetData(&query, Some(std::ptr::from_mut(&mut done).cast()), std::mem::size_of::<i32>() as u32, 0).is_err() { return None; }
        if done != 0 { return Some(begun.elapsed()); }
        if begun.elapsed() >= limit { return None; }
        std::hint::spin_loop();
    }
}

/// Only Windows reads the scan line, so only there is the drawing waited for.
///
/// # Safety
/// Nothing to uphold here; the signature is the Windows one's.
#[cfg(not(windows))]
pub unsafe fn finish_drawing(_renderer: *mut sdl3_sys::everything::SDL_Renderer, _limit: std::time::Duration) -> Option<std::time::Duration> { None }

#[cfg(test)]
mod tests;
