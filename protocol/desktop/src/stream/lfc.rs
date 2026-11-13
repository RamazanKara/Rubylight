//! Low frame rate compensation for a VRR display, done by the stream.
//!
//! The repeats are off unless `NIGHTJAR_VRR_LFC=1`: on screen they spared pictures a scan under
//! way for a steady 60 Hz script only, and the waits for a scan's end cost cadence (the README's
//! "Low frame rates on a VRR display" has the runs). `NIGHTJAR_VRR_LFC=count` only counts.
//!
//! A VRR panel waits for a present only so long. After the longest interval of its range (the
//! floor, 1/48 s on most) it scans the last picture again by itself, and that scan takes as long
//! as one at its top rate. A picture presented while it runs tears, or waits for its end
//! (`Streaming::wait_out_scan`): 7.6 to 12.3% of the presents of a 60 fps stream met a scan on a
//! 48-240 Hz monitor, and one in five at 47 fps (rust/PERFORMANCE.md, "Presenting while the
//! panel still scans"). The graphics driver repeats frames for a game below the floor too, from
//! the game's last frame time alone.
//!
//! [`Keeper`] has the stream present the held picture again itself, before the panel would:
//!
//! - while the game's frames are further apart than the panel waits, evenly between a picture
//!   and the next, so the panel keeps one rate and the repeat's scan is over well before the
//!   next picture. The next picture is expected one frame time after this one, less what this
//!   one was late by: after a late picture the repeats move closer, or are left out;
//! - once more between two pictures while that would have had fewer of the last pictures meet a
//!   scan than leaving it: a 60 fps game whose late frames cross the floor is shown at 120 Hz,
//!   one that keeps its time is left alone ([`Keeper::choose`]);
//! - when a picture is overdue, just before the floor, as late as it can be;
//! - never when a picture that waits is due before the panel's own refresh, and earlier when one
//!   is due while the repeat would still be scanned.
//!
//! The floor starts at [`ASSUMED_FLOOR_US`] and is learned from the scan line: a scan the stream
//! did not start shows how long the panel (or the driver) really waited ([`Keeper::observe`]).
//!
//! Everything here takes times in microseconds on the session clock and is tested on its own;
//! the presenter gives it the presents and the beam.

/// Kept between a repeat and what it must not meet, on top of the scan (µs).
pub const GUARD_US: u64 = 1_000;
/// The longest a panel is taken to wait before it refreshes by itself, until the scan line shows
/// otherwise: 48 Hz, the floor of most FreeSync and HDMI VRR ranges. A panel that waits longer
/// only gets repeats it did not need; one that waits less is learned (`Keeper::observe`).
pub const ASSUMED_FLOOR_US: u64 = 20_833;
/// The game's last frame intervals the expected one is the median of.
const INTERVALS: usize = 5;
/// The last pictures the number of repeats is chosen from: two seconds at 60 fps.
const SHOWN: usize = 128;
/// One scan more for each picture is taken up when at most half as many of those pictures would
/// have met a scan with it, less this many, and given up by the same rule. A panel changes its
/// brightness a little with its rate, so the number does not follow every late frame.
const SWITCH_PICTURES: usize = 2;
/// The scan more is also given up after this many pictures of which none would have met a scan
/// without it: ten seconds at 60 fps.
const CALM_PICTURES: u32 = 600;
/// A gap longer than this between two frames is a pause, not a frame time.
const PAUSE_US: u64 = 250_000;
/// A scan that began this much after the stream's last one is not the stream's (µs): the present
/// call and the GPU work ahead of the flip take up to a few milliseconds.
const FOREIGN_AFTER_US: u64 = 6_000;
/// Scans of the panel's own that have to agree within [`AGREE_US`] before the floor moves to them.
const SIGHTINGS: usize = 3;
const AGREE_US: u64 = 1_500;

/// No panel is taken to wait longer than this (10 Hz).
const LONGEST_FLOOR_US: u64 = 100_000;

/// What [`Keeper::observe`] found.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Observed {
    /// The scan on the display is not one the stream started.
    pub foreign: bool,
    /// The floor moved to this (µs).
    pub floor_us: Option<u64>,
}

/// The display as the scan line shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Panel {
    /// One scan at the top rate, blanking included (µs).
    pub scan_us: u64,
    /// The longest the panel waits for a present before it scans by itself (µs).
    pub floor_us: u64,
}

impl Panel {
    pub fn new(scan_us: u64) -> Self {
        Self { scan_us: scan_us.max(1), floor_us: ASSUMED_FLOOR_US }
    }

    /// A scan and the room after it: what a repeat takes before the display is idle again.
    pub fn clear_us(&self) -> u64 {
        self.scan_us + (self.scan_us / 10).max(GUARD_US)
    }

    /// Whether a repeat fits between two scans the panel would not refresh between by itself:
    /// a range whose floor is less than about 2.3 scans (48-100 Hz, say) has no room for one.
    pub fn has_room(&self) -> bool {
        2 * self.clear_us() + GUARD_US <= self.floor_us
    }
}

/// A picture the number of repeats is chosen from.
#[derive(Clone, Copy, Debug, Default)]
struct Shown {
    /// How long after the picture before it this one went to the display (µs).
    span_us: u32,
    /// How late the picture before it was (µs): this one was expected that much sooner after it.
    late_before_us: i32,
    /// This picture's time on the display less its time on the PC (µs, from an arbitrary start).
    offset_us: i64,
}

/// How long after a picture the next is expected: the game's frame time, less what the picture
/// was late by. Its successor is not late with it, or less so.
fn gap_us(interval_us: u64, late_us: i32) -> u64 {
    (interval_us as i64 - i64::from(late_us)).clamp(0, 2 * interval_us as i64) as u64
}

/// When the held picture goes to the display again. See the module documentation.
#[derive(Clone, Debug)]
pub struct Keeper {
    pub panel: Panel,
    /// When the stream last started a scan: a picture's present, a repeat's or a redraw's.
    last_start_us: Option<u64>,
    /// When the picture on the display was first presented.
    picture_us: Option<u64>,
    intervals: [u64; INTERVALS],
    len: usize,
    next: usize,
    /// The RTP timestamp of the picture on the display, and the ticks counted up to it.
    picture_rtp: Option<u32>,
    ticks: i64,
    shown: [Shown; SHOWN],
    shown_len: usize,
    shown_next: usize,
    /// How much later than the typical one of the last pictures the picture on the display came
    /// (µs, negative for an early one).
    late_us: i32,
    /// How many scans the stream starts for one picture: 1 is the picture alone.
    parts: u64,
    /// Pictures since one that would have met a scan with the fewest scans the game needs.
    calm: u32,
    /// How long after the stream's scans the panel's own began, the last few times.
    sightings: [u64; SIGHTINGS],
    sighted: usize,
    /// Repeats presented, and pictures that found a scan under way when they were due.
    pub repeats: u64,
    pub met_scan: u64,
    pub pictures: u64,
}

impl Keeper {
    pub fn new(scan_us: u64) -> Self {
        Self {
            panel: Panel::new(scan_us),
            last_start_us: None,
            picture_us: None,
            intervals: [0; INTERVALS],
            len: 0,
            next: 0,
            picture_rtp: None,
            ticks: 0,
            shown: [Shown::default(); SHOWN],
            shown_len: 0,
            shown_next: 0,
            late_us: 0,
            parts: 1,
            calm: 0,
            sightings: [0; SIGHTINGS],
            sighted: 0,
            repeats: 0,
            met_scan: 0,
            pictures: 0,
        }
    }

    /// The scan line was calibrated again. A scan that differs by more than a calibration does
    /// from the next (2%) is another monitor or mode: what was learned about the panel starts over.
    pub fn set_scan_us(&mut self, scan_us: u64) {
        let scan_us = scan_us.max(1);
        if scan_us.abs_diff(self.panel.scan_us) * 50 > self.panel.scan_us {
            *self = Self { repeats: self.repeats, met_scan: self.met_scan, pictures: self.pictures, ..Self::new(scan_us) };
        }
    }

    /// A repeat that was due found a scan under way, begun `since_scan_us` ago: the panel's wait
    /// counts from that scan, and the next repeat with it.
    pub fn scan_under_way(&mut self, now_us: u64, since_scan_us: u64) {
        self.last_start_us = Some(now_us.saturating_sub(since_scan_us.min(self.panel.scan_us)));
    }

    /// A new picture went to the display at `at_us`. `rtp` is its RTP timestamp (90 kHz), the
    /// host's stamp of when the game presented it: the game's frame time is its distance from the
    /// stamp of the picture before.
    pub fn picture(&mut self, at_us: u64, rtp: u32) {
        let ticks = self.picture_rtp.replace(rtp).map(|last| i64::from(rtp.wrapping_sub(last) as i32));
        let span = self.picture_us.and_then(|before| at_us.checked_sub(before));
        match (ticks, span) {
            (Some(ticks), Some(span)) if ticks > 0 && ticks * 100 / 9 <= PAUSE_US as i64 && span <= PAUSE_US => {
                self.intervals[self.next] = (ticks * 100 / 9) as u64;
                self.next = (self.next + 1) % INTERVALS;
                self.len = (self.len + 1).min(INTERVALS);
                self.ticks += ticks;
                let offset_us = at_us as i64 - self.ticks * 100 / 9;
                self.shown[self.shown_next] = Shown { span_us: span as u32, late_before_us: self.late_us, offset_us };
                self.shown_next = (self.shown_next + 1) % SHOWN;
                self.shown_len = (self.shown_len + 1).min(SHOWN);
                let limit = PAUSE_US as i64;
                self.late_us = (offset_us - self.typical_offset_us()).clamp(-limit, limit) as i32;
            }
            // The first picture, a pause, or a stream that starts over: the times before it say
            // nothing.
            _ => self.start_over(),
        }
        self.picture_us = Some(at_us);
        self.last_start_us = Some(at_us);
        self.pictures += 1;
        self.choose();
    }

    /// The held picture went to the display again at `at_us`, as a repeat of this keeper's.
    pub fn repeated(&mut self, at_us: u64) {
        self.last_start_us = Some(at_us);
        self.repeats += 1;
    }

    /// Any other present went out at `at_us` (the overlay, the menu, Frame Warp): it starts a
    /// scan as well.
    pub fn redrawn(&mut self, at_us: u64) {
        self.last_start_us = Some(at_us);
    }

    /// The stream stopped presenting to this display for a while (minimized, another pacing).
    pub fn forget(&mut self) {
        self.last_start_us = None;
        self.picture_us = None;
        self.picture_rtp = None;
        self.start_over();
        self.parts = 1;
    }

    fn start_over(&mut self) {
        (self.len, self.next) = (0, 0);
        (self.shown_len, self.shown_next) = (0, 0);
        (self.late_us, self.calm) = (0, 0);
    }

    /// The interval the next picture is expected after: the median of the game's last ones, once
    /// there are three.
    pub fn expected_us(&self) -> Option<u64> {
        if self.len < 3 {
            return None;
        }
        let mut sorted = self.intervals;
        let values = &mut sorted[..self.len];
        values.sort_unstable();
        Some(values[(values.len() - 1) / 2])
    }

    /// The typical time on the display less the time on the PC of the last pictures: the median.
    fn typical_offset_us(&self) -> i64 {
        let mut sorted = [0i64; SHOWN];
        let values = &mut sorted[..self.shown_len];
        for (value, shown) in values.iter_mut().zip(&self.shown) {
            *value = shown.offset_us;
        }
        values.sort_unstable();
        values.get(values.len().saturating_sub(1) / 2).copied().unwrap_or(0)
    }

    /// The scans between a picture and the next, expected `gap_us` after it: how far apart they
    /// start and how many there are, the picture's own included. Every one has room for its scan,
    /// so a picture close to the next gets fewer than `parts`.
    fn plan(&self, gap_us: u64, parts: u64) -> (u64, u64) {
        let parts = parts.min(gap_us / (self.panel.clear_us() + GUARD_US)).max(1);
        (gap_us / parts, parts)
    }

    /// Whether `shown` would have met a repeat's scan, had the stream started `parts` scans for
    /// each picture of a game `interval_us` apart: a repeat between two pictures, or one before
    /// the floor while the picture was overdue ([`Keeper::due_us`]).
    fn meets(&self, shown: &Shown, interval_us: u64, parts: u64) -> bool {
        let (clear, floor) = (self.panel.clear_us(), self.panel.floor_us);
        let span = u64::from(shown.span_us);
        let (step, parts) = self.plan(gap_us(interval_us, shown.late_before_us), parts);
        let (mut repeat, mut scans) = (0, 1);
        loop {
            repeat = if scans < parts { scans * step } else { repeat + floor - GUARD_US };
            // The repeats end for a game that stopped, and a picture ahead of one cancels it.
            if repeat > interval_us + 2 * floor || span + GUARD_US < repeat {
                return false;
            }
            if span < repeat + clear {
                return true;
            }
            scans += 1;
        }
    }

    fn would_meet(&self, interval_us: u64, parts: u64) -> usize {
        self.shown[..self.shown_len].iter().filter(|shown| self.meets(shown, interval_us, parts)).count()
    }

    /// Sets how many scans the stream starts for one picture. A game below the floor needs its
    /// pictures repeated, as many times as keep every part within the floor. One scan more is
    /// started while at most half as many of the last pictures would have met a scan with it
    /// ([`SWITCH_PICTURES`]), and given up by the same rule or after [`CALM_PICTURES`] that had
    /// no need of it.
    fn choose(&mut self) {
        let Some(interval) = self.expected_us().filter(|_| self.panel.has_room()) else {
            (self.parts, self.calm) = (1, 0);
            return;
        };
        let room = (interval / (self.panel.clear_us() + GUARD_US)).max(1);
        let least = interval.div_ceil(self.panel.floor_us - GUARD_US).min(room);
        let more = (least + 1).min(room);
        let newest = self.shown[(self.shown_next + SHOWN - 1) % SHOWN];
        let needed = self.shown_len > 0 && self.meets(&newest, interval, least);
        self.calm = if needed { 0 } else { self.calm.saturating_add(1) };
        let (with_least, with_more) = (self.would_meet(interval, least), self.would_meet(interval, more));
        self.parts = if self.parts.clamp(least, more) == least {
            if 2 * with_more + SWITCH_PICTURES <= with_least { more } else { least }
        } else if self.calm >= CALM_PICTURES || 2 * with_least + SWITCH_PICTURES <= with_more {
            least
        } else {
            more
        };
    }

    /// When the held picture is to be presented again, if at all. `next_picture_us` is when a
    /// picture that already waits is due.
    pub fn due_us(&self, next_picture_us: Option<u64>) -> Option<u64> {
        let (last, shown) = (self.last_start_us?, self.picture_us?);
        if !self.panel.has_room() {
            return None;
        }
        let (clear, floor) = (self.panel.clear_us(), self.panel.floor_us);
        let before_floor = last + floor - GUARD_US;
        let expected = self.expected_us();
        let mut due = match expected {
            Some(interval) => {
                let gap = gap_us(interval, self.late_us);
                let (step, parts) = self.plan(gap, self.parts);
                let planned = last + step;
                // Between two pictures only while the repeat is over before the next is expected;
                // after that the picture is overdue, and the repeat waits as long as it can.
                if parts > 1 && planned + clear + GUARD_US <= shown + gap {
                    planned
                } else {
                    before_floor
                }
            }
            None => before_floor,
        };
        // A game that stopped presenting: the panel refreshes a still picture by itself.
        if due > shown + expected.unwrap_or(0) + 2 * floor {
            return None;
        }
        if let Some(next) = next_picture_us {
            if next <= due + clear {
                // The repeat would still be scanned when the picture is due.
                if next + GUARD_US <= last + floor {
                    // The picture comes before the panel's own refresh: nothing to do.
                    return None;
                }
                // The panel's refresh would meet it: the repeat goes early enough to be over by
                // then. A panel with room for a repeat has it after the last scan as well.
                due = next.saturating_sub(clear);
            }
        }
        Some(due.max(last + clear))
    }

    /// What the scan line says just before a present at `now_us`: the scan on the display began
    /// `since_scan_us` ago, and `scanning` while its picture rows are still going out
    /// (`new_picture`: the present is a picture's, which then meets that scan).
    ///
    /// A scan that began well after the stream's last one is the panel's own (or the driver's):
    /// the panel waited that long, and its wait starts over from that scan. Three such waits that
    /// agree and differ from the floor in use become the floor.
    pub fn observe(&mut self, now_us: u64, since_scan_us: u64, scanning: bool, new_picture: bool) -> Observed {
        self.met_scan += u64::from(new_picture && scanning);
        let mut seen = Observed::default();
        let Some(ours) = self.last_start_us.and_then(|last| now_us.checked_sub(last)) else { return seen };
        if since_scan_us + FOREIGN_AFTER_US >= ours {
            // The stream's own scan, as far as the readings tell; the panel's wait counts from
            // where it began, which is after the present call.
            if since_scan_us < ours {
                self.last_start_us = Some(now_us - since_scan_us);
            }
            return seen;
        }
        seen.foreign = true;
        self.last_start_us = Some(now_us - since_scan_us);
        let waited = ours - since_scan_us;
        let floor = self.panel.floor_us;
        // Sooner than a repeat has room for is the GPU getting to the stream's own flip late;
        // near twice the floor or more, the panel refreshed more than once in between.
        let plausible = waited >= 2 * self.panel.clear_us() + GUARD_US && waited + AGREE_US < 2 * floor && waited <= LONGEST_FLOOR_US;
        if !plausible || waited.abs_diff(floor) <= AGREE_US {
            return seen;
        }
        self.sightings[self.sighted % SIGHTINGS] = waited;
        self.sighted += 1;
        if self.sighted < SIGHTINGS {
            return seen;
        }
        let (low, high) = self.sightings.iter().fold((u64::MAX, 0), |(low, high), s| (low.min(*s), high.max(*s)));
        if high - low <= AGREE_US {
            self.sighted = 0;
            self.panel.floor_us = low;
            seen.floor_us = Some(low);
        }
        seen
    }

    /// The overlay's and the log's words for what the keeper did.
    pub fn line(&self) -> String {
        let met = if self.pictures > 0 { self.met_scan as f64 * 100.0 / self.pictures as f64 } else { 0.0 };
        format!(
            "{} repeats, {met:.1}% of pictures met a scan, panel waits {:.1} ms",
            self.repeats,
            self.panel.floor_us as f64 / 1e3
        )
    }
}

#[cfg(test)]
mod tests;
