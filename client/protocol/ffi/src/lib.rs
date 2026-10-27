//! C ABI over `rubylight-protocol` for the Android client (see `include/rubylight_protocol.h`).
//!
//! Built as a static library linked into the client's native code. Outside tests it is
//! `no_std`: memory comes from the C allocator and a panic aborts, so the library needs
//! nothing beyond libc.
#![cfg_attr(not(any(test, windows)), no_std)]

extern crate alloc;

use alloc::boxed::Box;
use core::ffi::c_void;
use core::slice;
use rubylight_protocol::control::{
    ClockSyncReply, ClockSyncRequest, DisplayCaps, Reconfigure, CLOCK_SYNC_REQUEST_BYTES, DISPLAY_CAPS_BYTES,
    RECONFIGURE_BYTES,
};
use rubylight_protocol::present_clock::{Grid, PresentClock, Settings};
use rubylight_protocol::fec_report::{FecBlock, FecTally, FEC_REPORT_BYTES};
use rubylight_protocol::phase_lock::{SlackWindow, REPORT_BYTES};
use rubylight_protocol::pyrowave::{self, Records};

/// Receives one decoder packet; returns false to stop and fail the frame.
pub type PushPacket = unsafe extern "C" fn(user: *mut c_void, data: *const u8, length: usize) -> bool;

unsafe fn bytes<'a>(data: *const u8, length: usize) -> &'a [u8] {
    if length == 0 { &[] } else { slice::from_raw_parts(data, length) }
}

fn forward(push: PushPacket, user: *mut c_void) -> impl FnMut(&[u8]) -> bool {
    move |packet| unsafe { push(user, packet.as_ptr(), packet.len()) }
}

/// Creates record reassembly state for a stream of the negotiated geometry; a zero width or
/// height means none was negotiated.
#[no_mangle]
pub extern "C" fn rp_pyrowave_records_new(width: u32, height: u32, chroma444: bool) -> *mut Records {
    let records = if width == 0 || height == 0 { Records::default() } else { Records::with_geometry(width, height, chroma444) };
    Box::into_raw(Box::new(records))
}

/// # Safety
/// `records` comes from `rp_pyrowave_records_new` and is not used afterwards; null is ignored.
#[no_mangle]
pub unsafe extern "C" fn rp_pyrowave_records_free(records: *mut Records) {
    if !records.is_null() {
        drop(Box::from_raw(records));
    }
}

/// Feeds one record-framed frame to `push`, sequence header first.
///
/// # Safety
/// `records` is live, `data` points to `length` readable bytes, and `push` is safe to call
/// with `user`.
#[no_mangle]
pub unsafe extern "C" fn rp_pyrowave_records_push_frame(
    records: *mut Records,
    data: *const u8,
    length: usize,
    push: PushPacket,
    user: *mut c_void,
) -> bool {
    (*records).push_frame(bytes(data, length), forward(push, user))
}

/// Share of the last frame's records that were lost, 0-100.
///
/// # Safety
/// `records` is live.
#[no_mangle]
pub unsafe extern "C" fn rp_pyrowave_records_loss_percent(records: *const Records) -> f32 {
    (*records).loss_percent
}

/// Validates an ordinary (pre-record) container and feeds its packets to `push`.
///
/// # Safety
/// `data` points to `length` readable bytes, and `push` is safe to call with `user`.
#[no_mangle]
pub unsafe extern "C" fn rp_pyrowave_push_container(
    data: *const u8,
    length: usize,
    push: PushPacket,
    user: *mut c_void,
) -> bool {
    pyrowave::push_container(bytes(data, length), forward(push, user))
}

/// Creates an empty phase-lock slack window.
#[no_mangle]
pub extern "C" fn rp_slack_window_new() -> *mut SlackWindow {
    Box::into_raw(Box::default())
}

/// # Safety
/// `window` comes from `rp_slack_window_new` and is not used afterwards; null is ignored.
#[no_mangle]
pub unsafe extern "C" fn rp_slack_window_free(window: *mut SlackWindow) {
    if !window.is_null() {
        drop(Box::from_raw(window));
    }
}

/// Records one shown frame's slack before the latch and the display refresh period.
///
/// # Safety
/// `window` is live.
#[no_mangle]
pub unsafe extern "C" fn rp_slack_window_push(window: *mut SlackWindow, slack_ns: i64, period_ns: i64) {
    (*window).push(slack_ns, period_ns);
}

/// Frames recorded since the last report.
///
/// # Safety
/// `window` is live.
#[no_mangle]
pub unsafe extern "C" fn rp_slack_window_len(window: *const SlackWindow) -> usize {
    (*window).len()
}

/// Summarises and clears the window into the 16-byte report message at `out`. Returns false
/// (and still clears the window) with fewer than `min_frames` frames.
///
/// # Safety
/// `window` is live and `out` points to `RP_PHASE_REPORT_BYTES` writable bytes.
#[no_mangle]
pub unsafe extern "C" fn rp_slack_window_report(window: *mut SlackWindow, min_frames: usize, out: *mut u8) -> bool {
    match (*window).report(min_frames) {
        Some(report) => {
            core::ptr::copy_nonoverlapping(report.encode().as_ptr(), out, REPORT_BYTES);
            true
        }
        None => false,
    }
}

/// Writes the 16-byte display-caps message (0x5531) to `out`. Luminance in nits; 0 for a
/// value the display does not report.
///
/// # Safety
/// `out` points to `RP_DISPLAY_CAPS_BYTES` writable bytes.
#[no_mangle]
pub unsafe extern "C" fn rp_display_caps_encode(hdr: bool, max_nits: f32, max_average_nits: f32, min_nits: f32, out: *mut u8) {
    let scale = |nits: f32, per_nit: f32| if nits.is_finite() && nits > 0.0 { (nits * per_nit + 0.5).min(u32::MAX as f32) as u32 } else { 0 };
    let caps = DisplayCaps {
        hdr,
        max_centinits: scale(max_nits, 100.0),
        max_average_centinits: scale(max_average_nits, 100.0),
        min_decimillinits: scale(min_nits, 10_000.0),
    };
    core::ptr::copy_nonoverlapping(caps.encode().as_ptr(), out, DISPLAY_CAPS_BYTES);
}

/// Creates an empty FEC loss tally for the once-a-second report (0x5539).
#[no_mangle]
pub extern "C" fn rp_fec_tally_new() -> *mut FecTally {
    Box::into_raw(Box::default())
}

/// # Safety
/// `tally` comes from `rp_fec_tally_new` and is not used afterwards; null is ignored.
#[no_mangle]
pub unsafe extern "C" fn rp_fec_tally_free(tally: *mut FecTally) {
    if !tally.is_null() {
        drop(Box::from_raw(tally));
    }
}

/// Counts one finished video FEC block, recovered or not, at `now_ms` on any monotonic clock,
/// from the video queue's counters (`FecBlock`): `arrived_shards` that came and
/// `missing_shards` holes before the last arrival.
///
/// # Safety
/// `tally` is live.
#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn rp_fec_tally_block(
    tally: *mut FecTally,
    frame: u32,
    block_index: u8,
    block_count: u8,
    data_shards: u32,
    parity_shards: u32,
    arrived_shards: u32,
    missing_shards: u32,
    unrecovered: bool,
    now_ms: u64,
) {
    let block = FecBlock {
        frame,
        block_index,
        block_count,
        data_shards,
        parity_shards,
        arrived_shards,
        missing_shards,
        unrecovered,
    };
    (*tally).block(block, now_ms);
}

/// Writes the 24-byte report to `out` once its interval is a second old and returns true; an
/// interval without video gives a report without shards, and one past 5 s or before the first
/// block writes nothing.
///
/// # Safety
/// `tally` is live and `out` points to `RP_FEC_REPORT_BYTES` writable bytes.
#[no_mangle]
pub unsafe extern "C" fn rp_fec_tally_report(tally: *mut FecTally, now_ms: u64, out: *mut u8) -> bool {
    match (*tally).take(now_ms) {
        Some(report) => {
            core::ptr::copy_nonoverlapping(report.encode().as_ptr(), out, FEC_REPORT_BYTES);
            true
        }
        None => false,
    }
}

/// Writes the 12-byte reconfigure message (0x5532) to `out`.
///
/// # Safety
/// `out` points to `RP_RECONFIGURE_BYTES` writable bytes.
#[no_mangle]
pub unsafe extern "C" fn rp_reconfigure_encode(width: u16, height: u16, fps_millihz: u32, out: *mut u8) {
    let request = Reconfigure { width, height, fps_millihz };
    core::ptr::copy_nonoverlapping(request.encode().as_ptr(), out, RECONFIGURE_BYTES);
}

/// Writes the 16-byte clock sync request (0x5537) to `out`.
///
/// # Safety
/// `out` points to `RP_CLOCK_SYNC_REQUEST_BYTES` writable bytes.
#[no_mangle]
pub unsafe extern "C" fn rp_clock_sync_request_encode(sequence: u32, client_us: u64, out: *mut u8) {
    let request = ClockSyncRequest { sequence, client_us };
    core::ptr::copy_nonoverlapping(request.encode().as_ptr(), out, CLOCK_SYNC_REQUEST_BYTES);
}

/// Values of [`PresentClock`]'s settings; see `present_clock::Settings`.
#[repr(C)]
pub struct RpPresentSettings {
    pub window: u32,
    pub percentile: u8,
    pub margin_us: i64,
    pub decay_us: i64,
    pub max_hold_us: i64,
    pub min_interval_us: i64,
}

impl From<&RpPresentSettings> for Settings {
    fn from(s: &RpPresentSettings) -> Self {
        Settings {
            window: s.window as usize,
            percentile: s.percentile,
            margin_us: s.margin_us,
            decay_us: s.decay_us,
            max_hold_us: s.max_hold_us,
            min_interval_us: s.min_interval_us,
        }
    }
}

/// Fills `out` with the default settings.
///
/// # Safety
/// `out` is writable.
#[no_mangle]
pub unsafe extern "C" fn rp_present_settings_default(out: *mut RpPresentSettings) {
    let s = Settings::default();
    out.write(RpPresentSettings {
        window: s.window as u32,
        percentile: s.percentile,
        margin_us: s.margin_us,
        decay_us: s.decay_us,
        max_hold_us: s.max_hold_us,
        min_interval_us: s.min_interval_us,
    });
}

/// # Safety
/// `settings` is readable.
#[no_mangle]
pub unsafe extern "C" fn rp_present_clock_new(settings: *const RpPresentSettings) -> *mut PresentClock {
    Box::into_raw(Box::new(PresentClock::new((&*settings).into())))
}

/// # Safety
/// `clock` comes from `rp_present_clock_new` and is not used afterwards; null is ignored.
#[no_mangle]
pub unsafe extern "C" fn rp_present_clock_free(clock: *mut PresentClock) {
    if !clock.is_null() {
        drop(Box::from_raw(clock));
    }
}

/// Feeds a clock sync reply (0x5537 payload) received at `client_receive_us`. Returns the
/// exchange's round trip in microseconds, or -1 for a reply that is malformed or cannot be right.
///
/// # Safety
/// `clock` is live and `data` points to `length` readable bytes.
#[no_mangle]
pub unsafe extern "C" fn rp_present_clock_sync(
    clock: *mut PresentClock,
    data: *const u8,
    length: usize,
    client_receive_us: u64,
) -> i64 {
    ClockSyncReply::decode(bytes(data, length))
        .and_then(|reply| (*clock).offset.add(&reply, client_receive_us))
        .unwrap_or(-1)
}

/// One frame's schedule; see `present_clock::Schedule`. `latency_us` is -1 without clock sync.
#[repr(C)]
pub struct RpPresentSchedule {
    pub present_us: u64,
    pub hold_us: i64,
    pub latency_us: i64,
    pub late: bool,
}

/// When to present the frame with RTP timestamp `rtp` that was ready at `ready_us` (client
/// microseconds, the clock the clock sync requests use).
///
/// # Safety
/// `clock` is live.
#[no_mangle]
pub unsafe extern "C" fn rp_present_clock_schedule(clock: *mut PresentClock, rtp: u32, ready_us: u64) -> RpPresentSchedule {
    let s = (*clock).schedule(rtp, ready_us);
    RpPresentSchedule {
        present_us: s.present_us,
        hold_us: s.hold_us,
        latency_us: s.latency_us.unwrap_or(-1),
        late: s.late,
    }
}

/// The display's vsync grid; see `present_clock::Grid`.
#[repr(C)]
pub struct RpPresentGrid {
    pub anchor_us: i64,
    pub interval_us: i64,
    pub lead_us: i64,
    pub slack_us: i64,
}

/// One frame's release onto a vsync grid; see `present_clock::GridSchedule`. `latency_us` is -1
/// without clock sync.
#[repr(C)]
pub struct RpPresentGridSchedule {
    pub release_us: u64,
    pub vsync_us: u64,
    pub vsync_index: i64,
    pub late_by_us: i64,
    pub hold_us: i64,
    pub latency_us: i64,
    pub late: bool,
    pub shares_vsync: bool,
}

/// As `rp_present_clock_schedule`, onto a vsync grid: release the frame at `release_us` for the
/// vsync at `vsync_us`.
///
/// # Safety
/// `clock` is live and `grid` is readable.
#[no_mangle]
pub unsafe extern "C" fn rp_present_clock_schedule_on_grid(
    clock: *mut PresentClock,
    rtp: u32,
    ready_us: u64,
    grid: *const RpPresentGrid,
) -> RpPresentGridSchedule {
    let g = &*grid;
    let s = (*clock).schedule_on_grid(
        rtp,
        ready_us,
        Grid { anchor_us: g.anchor_us, interval_us: g.interval_us, lead_us: g.lead_us, slack_us: g.slack_us },
    );
    RpPresentGridSchedule {
        release_us: s.release_us,
        vsync_us: s.vsync_us,
        vsync_index: s.vsync_index,
        late_by_us: s.late_by_us,
        hold_us: s.hold_us,
        latency_us: s.latency_us.unwrap_or(-1),
        late: s.late,
        shares_vsync: s.shares_vsync,
    }
}

/// Notes a frame with RTP timestamp `rtp` for the content rate; call for every frame in order.
///
/// # Safety
/// `clock` is live.
#[no_mangle]
pub unsafe extern "C" fn rp_present_clock_observe(clock: *mut PresentClock, rtp: u32) {
    (*clock).observe(rtp);
}

/// The content rate in millihertz once it held steady for two seconds, else 0.
///
/// # Safety
/// `clock` is live.
#[no_mangle]
pub unsafe extern "C" fn rp_present_clock_content_rate(clock: *const PresentClock) -> u32 {
    (*clock).content_rate_mhz().unwrap_or(0)
}

/// Forgets the frames seen, for a new stream size or a decoder reset; clock sync stays.
///
/// # Safety
/// `clock` is live.
#[no_mangle]
pub unsafe extern "C" fn rp_present_clock_reset(clock: *mut PresentClock) {
    (*clock).reset();
}

/// Frames scheduled, frames shown late, and the clock sync round trip (-1 without one).
///
/// # Safety
/// `clock` is live; the out pointers are writable or null.
#[no_mangle]
pub unsafe extern "C" fn rp_present_clock_stats(clock: *const PresentClock, frames: *mut u64, late: *mut u64, rtt_us: *mut i64) {
    let clock = &*clock;
    if !frames.is_null() {
        frames.write(clock.frames);
    }
    if !late.is_null() {
        late.write(clock.late_frames);
    }
    if !rtt_us.is_null() {
        rtt_us.write(clock.offset.rtt_us().unwrap_or(-1));
    }
}

/// The offset from the host's video clock to the client's (`client = host + offset`) at client
/// time `local_us`, and the round trip of the exchange it comes from, both in microseconds.
/// Returns false, writing nothing, before the first clock sync exchange.
///
/// # Safety
/// `clock` is live; the out pointers are writable or null.
#[no_mangle]
pub unsafe extern "C" fn rp_present_clock_offset(clock: *const PresentClock, local_us: u64, offset_us: *mut i64, rtt_us: *mut i64) -> bool {
    let clock = &*clock;
    let (Some(offset), Some(rtt)) = (clock.offset.offset_at(local_us as i64), clock.offset.rtt_us()) else {
        return false;
    };
    if !offset_us.is_null() {
        offset_us.write(offset);
    }
    if !rtt_us.is_null() {
        rtt_us.write(rtt);
    }
    true
}

// Windows clients link the standard library (the MSVC CRT has no posix_memalign).
#[cfg(not(any(test, windows)))]
mod runtime {
    use core::alloc::{GlobalAlloc, Layout};
    use core::ffi::c_void;

    extern "C" {
        fn malloc(size: usize) -> *mut c_void;
        fn free(pointer: *mut c_void);
        fn realloc(pointer: *mut c_void, size: usize) -> *mut c_void;
        fn posix_memalign(pointer: *mut *mut c_void, alignment: usize, size: usize) -> i32;
        fn abort() -> !;
    }

    /// malloc already aligns to 16 bytes on every Android ABI.
    const MALLOC_ALIGN: usize = 16;

    struct CAllocator;

    unsafe impl GlobalAlloc for CAllocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            if layout.align() <= MALLOC_ALIGN {
                return malloc(layout.size()) as *mut u8;
            }
            let mut pointer = core::ptr::null_mut();
            let alignment = layout.align().max(core::mem::size_of::<usize>());
            if posix_memalign(&mut pointer, alignment, layout.size()) == 0 { pointer as *mut u8 } else { core::ptr::null_mut() }
        }

        unsafe fn dealloc(&self, pointer: *mut u8, _: Layout) {
            free(pointer as *mut c_void);
        }

        unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
            if layout.align() <= MALLOC_ALIGN {
                return realloc(pointer as *mut c_void, size) as *mut u8;
            }
            let moved = self.alloc(Layout::from_size_align_unchecked(size, layout.align()));
            if !moved.is_null() {
                core::ptr::copy_nonoverlapping(pointer, moved, layout.size().min(size));
                self.dealloc(pointer, layout);
            }
            moved
        }
    }

    #[global_allocator]
    static ALLOCATOR: CAllocator = CAllocator;

    #[panic_handler]
    fn panic(_: &core::panic::PanicInfo) -> ! {
        unsafe { abort() }
    }
}
