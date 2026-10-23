//! The row order of the Rubylight host (`x-ss-video[0].pyrowaveRowOrder:1`): which picture band
//! each block record belongs to, and a tracker that says, as the records of a frame complete,
//! which bands can be decoded exactly.
//!
//! With the order on, the host sends the coarse level first and then the fine records by picture
//! band: block row `r` of wavelet level `L` is needed by picture band `k` (64 rows, one block row
//! of level 0) when `r * 2^L - 1 <= k`, because the inverse wavelet's four lifting steps of +-1
//! reach under one block row at every level. [`row_steps`] gives that band, the record's step,
//! for every block index; it matches the host's `row_steps` in `butterpollo-core`
//! (`rust/core/src/pyrowave.rs`) value for value, which `tests` pins with vectors taken from it.
//!
//! Every record with a step up to `k` having arrived is exactly what band `k` needs, so a decoder
//! that treats the missing blocks as zero reproduces the full decode on those rows.

use alloc::vec;
use alloc::vec::Vec;

/// Rows of one picture band: one block row of level 0 (a 32 row block of a half size band).
pub const BAND_ROWS: u32 = 64;

/// The picture band after which each block of the index space is sent in row order, indexed by
/// block index. Level 4 (the coarse level, the host's critical FEC block) is all step 0.
///
/// The index space runs level 4 to 0, per level the components, per component the bands (four at
/// level 4, three below), per band the 32x32 blocks in rows. 4:2:0 chroma has no level 0.
pub fn row_steps(width: u32, height: u32, chroma444: bool) -> Vec<u16> {
    let stride = width.div_ceil(32).max(4) * 32;
    let rows = height.div_ceil(32).max(4) * 32;
    let bands = rows.div_ceil(BAND_ROWS);
    let mut steps = Vec::new();
    for level in (0..5u32).rev() {
        for component in 0..3 {
            if level == 0 && component != 0 && !chroma444 {
                continue;
            }
            let (across, down) = ((stride >> (level + 1)).div_ceil(32), (rows >> (level + 1)).div_ceil(32));
            for _band in 0..if level == 4 { 4 } else { 3 } {
                for row in 0..down {
                    let step = if level == 4 { 0 } else { (row << level).saturating_sub(1).min(bands - 1) };
                    steps.extend((0..across).map(|_| step as u16));
                }
            }
        }
    }
    steps
}

/// The bands of a picture of this height (`ceil(rows / 64)` of the padded height).
pub fn band_count(height: u32) -> u32 {
    (height.div_ceil(32).max(4) * 32).div_ceil(BAND_ROWS)
}

/// How a frame's records arrived, against the row order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RowReport {
    /// Records seen.
    pub records: u32,
    /// Records that arrived after a record of a later step by more than one packet. A host that
    /// sends the row order has none, or the odd one from packing; a host that does not (or a
    /// frame it sent in index order) has many.
    pub violations: u32,
}

impl RowReport {
    /// The frame arrived in row order: at most one record out of place, as one reordered packet
    /// can cause (the host's first-fit packing delays a record that does not fit by one packet).
    pub fn ordered(&self) -> bool {
        self.records > 0 && self.violations < 2
    }
}

/// Follows a frame's records as they complete, packet by packet, and says how many picture bands
/// are exact.
///
/// The host's packing fills the room left in a packet with any later record that fits, so a
/// packet can carry records of far later steps than the ones it is there to send. What follows
/// the row order is the lowest step in each packet (its cursor): it never goes back by more than
/// a packet. A step is complete once the cursor passed it in an earlier packet. The last steps
/// (the ones no later step follows) complete with the frame: [`RowTracker::finish`].
#[derive(Clone, Debug)]
pub struct RowTracker {
    steps: Vec<u16>,
    bands: u32,
    /// Per step: the packet whose lowest step was the first above it, `u32::MAX` for none yet.
    crossed_in: Vec<u32>,
    /// The highest of the packets' lowest steps so far.
    highest: Option<u16>,
    /// The lowest step among the records the current packet completed.
    lowest_here: Option<u16>,
    packet: u32,
    complete: u32,
    report: RowReport,
    finished: bool,
}

impl RowTracker {
    pub fn new(width: u32, height: u32, chroma444: bool) -> Self {
        let bands = band_count(height);
        Self {
            steps: row_steps(width, height, chroma444),
            bands,
            crossed_in: vec![u32::MAX; bands as usize],
            highest: None,
            lowest_here: None,
            packet: 0,
            complete: 0,
            report: RowReport::default(),
            finished: false,
        }
    }

    /// Starts a frame.
    pub fn begin_frame(&mut self) {
        self.crossed_in.fill(u32::MAX);
        self.highest = None;
        self.lowest_here = None;
        self.packet = 0;
        self.complete = 0;
        self.report = RowReport::default();
        self.finished = false;
    }

    /// A packet of the frame was fed; the records it completes follow with [`Self::record`].
    pub fn packet(&mut self) {
        self.close_packet();
        self.packet += 1;
    }

    /// The packet that was fed last is over: its lowest step moves the cursor.
    fn close_packet(&mut self) {
        let Some(step) = self.lowest_here.take() else { return };
        if self.highest.map_or(true, |highest| step > highest) {
            let from = self.highest.map_or(0, usize::from);
            for s in from..usize::from(step) {
                if self.crossed_in[s] == u32::MAX {
                    self.crossed_in[s] = self.packet;
                }
            }
            self.highest = Some(step);
        }
    }

    /// A record of block `index` completed in the current packet. An index outside the picture
    /// counts as a violation.
    pub fn record(&mut self, index: u32) {
        self.report.records += 1;
        let Some(&step) = self.steps.get(index as usize) else {
            self.report.violations += 1;
            return;
        };
        self.lowest_here = Some(self.lowest_here.map_or(step, |lowest| lowest.min(step)));
        if self.highest.is_some_and(|highest| step < highest)
            && self.packet > self.crossed_in[usize::from(step)].saturating_add(1)
        {
            // Later than the cursor passing its step by more than the next packet.
            self.report.violations += 1;
        }
    }

    /// The frame's last packet has been fed: every band is complete.
    pub fn finish(&mut self) {
        self.close_packet();
        self.finished = true;
    }

    /// How many bands from the top are exact now: band `k` is when every record of a step up to
    /// `k` has arrived. Does not count a violation's late records, which a decoder repairs with
    /// the final decode.
    pub fn complete_bands(&mut self) -> u32 {
        if self.finished {
            return self.bands;
        }
        // Step s is complete when the cursor crossed it in a packet before the current one.
        while self.complete < self.bands
            && self.crossed_in.get(self.complete as usize).is_some_and(|&p| p < self.packet)
        {
            self.complete += 1;
        }
        self.complete
    }

    pub fn bands(&self) -> u32 {
        self.bands
    }

    pub fn report(&self) -> RowReport {
        self.report
    }
}

/// Where the `slices` slices of a picture of `bands` bands end: slice `i` holds bands up to (not
/// including) the `i`th value. The last is `bands`. With fewer bands than slices the slices
/// shrink to one band each.
pub fn slice_ends(bands: u32, slices: u32) -> Vec<u32> {
    let slices = slices.clamp(1, bands.max(1));
    (1..=slices).map(|i| (bands * i).div_ceil(slices)).collect()
}

/// Picture rows of bands `first..end`, clipped to the picture.
pub fn band_rows(height: u32, first: u32, end: u32) -> (u32, u32) {
    ((first * BAND_ROWS).min(height), (end * BAND_ROWS).min(height))
}

#[cfg(test)]
mod tests;
