//! Encoder focus on AMF (docs/encoder-focus-design.md): the converter's
//! importance map goes on the input surface as the codec's ROI map, and the
//! startup probe proves from block QP feedback that the driver honours it.
use super::*;
use butterpollo_core::encoder_focus::{self, Reason, Stats, Support};
use std::collections::VecDeque;

/// The probe map must move the unimportant half at least this many QP above
/// the important half ([`Q_INDEX_SCALE`] times as many AV1 q-index steps).
pub const ROI_MIN_DELTA: f32 = 2.0;
/// AV1 feedback is q-index (0..=255), about four steps per QP.
pub const Q_INDEX_SCALE: f32 = 4.0;
/// Probe frames before the measured ones, the first an IDR, so rate control settles.
const PROBE_WARMUP: u32 = 4;
/// Measured probe frames with the left half important, then as many with the right.
const PROBE_FRAMES: u32 = 3;
/// Frames after a keyframe, a recovery picture or the encoder's first frame
/// that carry no map. Any ROI map, even an all-zero one, lifts the
/// latency-constrained rate control's limit on big frames from the frame it
/// is on, so maps while the rate control settles after an IDR made frames of
/// 75 to 145 KB against an 83 KB cap (HEVC 1080p60 at 20 Mbps, RX 7900 XT,
/// 2026-10-27). Maps from the 16th frame on matched focus off; this is twice that.
pub(crate) const SETTLE_FRAMES: u32 = 30;
/// `IID_AMFSurface` (Surface.h), an inline function bindgen does not emit.
const SURFACE_IID: AMFGuid = AMFGuid {
    data1: 0x3075dbe3,
    data2: 0x8718,
    data3: 0x4cfa,
    data41: 0x86,
    data42: 0xfb,
    data43: 0x21,
    data44: 0x14,
    data45: 0xc0,
    data46: 0xa5,
    data47: 0xa4,
    data48: 0x51,
};

pub(crate) fn roi_name(codec: u8) -> &'static str {
    match codec {
        0 => "ROIData",
        1 => "HevcROIData",
        _ => "Av1ROIData",
    }
}
/// The ROI capability; AV1 has none.
pub(crate) fn cap_name(codec: u8) -> Option<&'static str> {
    match codec {
        0 => Some("ROIMap"),
        1 => Some("HevcROIMap"),
        _ => None,
    }
}
pub(crate) fn qp_feedback(codec: u8) -> &'static str {
    match codec {
        0 => "BlockQpFeedback",
        1 => "HevcBlockQpFeedback",
        _ => "Av1BlockQIndexFeedback",
    }
}
pub(crate) fn qp_map(codec: u8) -> &'static str {
    match codec {
        0 => "BlockQpMap",
        1 => "HevcBlockQpMap",
        _ => "Av1BlockQIndexMap",
    }
}
/// The size VCN codes a frame at, in whole 16x16 blocks: 1080 rows are 1088.
pub(crate) fn coded_size((width, height): (u32, u32)) -> (u32, u32) {
    (width.next_multiple_of(16), height.next_multiple_of(16))
}
/// A map whose grid is not the coded frame's would weight the wrong blocks.
pub(crate) fn check_grid(codec: u8, frame: (u32, u32), map: (u32, u32)) -> Result<()> {
    let (width, height) = coded_size(frame);
    let expected = encoder_focus::grid(codec, width, height);
    anyhow::ensure!(
        map == expected,
        "encoder focus map is {}x{} blocks, but the encoder codes {width}x{height} in {}x{} blocks",
        map.0,
        map.1,
        expected.0,
        expected.1
    );
    Ok(())
}

/// One u32 per block, row by row. In a QP map, 0 marks a block the driver
/// returned no QP for.
#[derive(Clone, Debug, PartialEq)]
pub struct Grid {
    pub width: u32,
    pub height: u32,
    pub values: Vec<u32>,
}
/// Undo the layout the RX 7900 XT's driver returns block QP maps in: it packs
/// a map whose rows are `stride` values apart (the surface pitch) as if they
/// were `width` apart, so block (x, y) lands at flat index `y * stride + x`
/// of the `width`-wide grid, and rows past `width * height / stride` are lost.
/// A grid whose nonzero values all sit at such positions is laid out again,
/// with the lost blocks 0; any other grid is the plain layout and kept.
pub(crate) fn unstride(grid: Grid, stride: usize) -> Grid {
    let width = grid.width as usize;
    if stride <= width || grid.height < 2 {
        return grid;
    }
    let strided = grid
        .values
        .iter()
        .enumerate()
        .all(|(index, &value)| value == 0 || index % stride < width);
    if !strided {
        return grid;
    }
    let values = (0..grid.values.len())
        .map(|index| {
            let (x, y) = (index % width, index / width);
            grid.values.get(y * stride + x).copied().unwrap_or(0)
        })
        .collect();
    Grid { values, ..grid }
}
/// Mean of the right half's blocks minus the left half's; an odd middle
/// column and blocks without a QP count for neither.
pub(crate) fn half_delta(grid: &Grid) -> Option<f32> {
    let half = grid.width / 2;
    let (mut left, mut right) = ((0f64, 0u32), (0f64, 0u32));
    for (index, &value) in grid.values.iter().enumerate() {
        let x = index as u32 % grid.width.max(1);
        let side = if value == 0 {
            continue;
        } else if x < half {
            &mut left
        } else if x >= grid.width - half {
            &mut right
        } else {
            continue;
        };
        side.0 += f64::from(value);
        side.1 += 1;
    }
    (left.1 > 0 && right.1 > 0)
        .then(|| (right.0 / f64::from(right.1) - left.0 / f64::from(left.1)) as f32)
}
/// Mean QP of the least important blocks minus the most important, with
/// each QP block placed on the importance grid; `None` for a uniform map.
/// Blocks without a QP count for neither.
pub(crate) fn importance_delta(qp: &Grid, importance: &Grid) -> Option<f32> {
    let low = *importance.values.iter().min()?;
    let high = *importance.values.iter().max()?;
    if low == high || qp.width == 0 || qp.height == 0 {
        return None;
    }
    let (mut lows, mut highs) = ((0f64, 0u32), (0f64, 0u32));
    for (index, &value) in qp.values.iter().enumerate() {
        if value == 0 {
            continue;
        }
        let (x, y) = (index as u32 % qp.width, index as u32 / qp.width);
        let at = (y * importance.height / qp.height) * importance.width
            + x * importance.width / qp.width;
        let side = match importance.values.get(at as usize) {
            Some(&v) if v == low => &mut lows,
            Some(&v) if v == high => &mut highs,
            _ => continue,
        };
        side.0 += f64::from(value);
        side.1 += 1;
    }
    (lows.1 > 0 && highs.1 > 0)
        .then(|| (lows.0 / f64::from(lows.1) - highs.0 / f64::from(highs.1)) as f32)
}

/// What one frame does with the newest finished map, given as `(value, uniform)`.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Pick {
    /// A keyframe, recovery picture or first frame, or one of the
    /// [`SETTLE_FRAMES`] after them: no map.
    Skip,
    /// No map newer than the last one used.
    Stale,
    /// Every block alike: attaching it would change nothing.
    Uniform,
    Attach,
}
fn pick(
    idr: bool,
    recover: bool,
    first: bool,
    settling: bool,
    map: Option<(u64, bool)>,
    last: u64,
) -> Pick {
    if settling || !encoder_focus::attach_for(idr, recover, first, false) {
        return Pick::Skip;
    }
    match map {
        None => Pick::Stale,
        Some((value, _)) if value <= last => Pick::Stale,
        Some((_, true)) => Pick::Uniform,
        Some(_) => Pick::Attach,
    }
}
/// Counts one frame in `settled`, from 0 on a keyframe, recovery picture or
/// first frame (`reset`); true while the frame is one of those or of the
/// [`SETTLE_FRAMES`] after them.
fn settle(settled: &mut u32, reset: bool) -> bool {
    *settled = if reset { 0 } else { settled.saturating_add(1) };
    *settled <= SETTLE_FRAMES
}
fn count(stats: &mut Stats, pick: Pick) {
    match pick {
        Pick::Skip => stats.skipped_recovery += 1,
        Pick::Stale => stats.stale += 1,
        Pick::Uniform => stats.uniform += 1,
        Pick::Attach => stats.maps += 1,
    }
}

/// A frame that asked AMF for its block QP map.
struct Ask {
    pts: i64,
    /// The importances it carried, for `qp_delta`.
    importance: Option<Grid>,
    /// The probe keeps the answer.
    keep: bool,
}
/// The encoder's focus state.
#[derive(Default)]
pub(crate) struct State {
    settings: Option<crate::compute::FocusSettings>,
    /// The value of the last map a frame used.
    last: u64,
    /// Frames since the last keyframe, recovery picture or first frame.
    settled: u32,
    stats: Stats,
    /// Ask for QP feedback on one frame in this many that carry a map.
    verify: Option<u32>,
    since_verify: u32,
    qp_sum: f64,
    qp_count: u32,
    /// The probe's map, attached instead of the converter's.
    fixed: Option<Grid>,
    /// The probe asks every frame for feedback.
    keep: bool,
    asked: VecDeque<Ask>,
    /// The probe's answers in frame order: the QP map, `None` when AMF sent
    /// none, or why it could not be read.
    answers: Vec<std::result::Result<Option<Grid>, String>>,
}

/// SetProperty on an input surface.
///
/// # Safety
/// `surface` is live, and an interface in `value` stays live for the call.
unsafe fn set(surface: *mut AMFSurface, name: &str, value: AMFVariantStruct) -> Result<()> {
    // SAFETY: Per the contract; AMF copies the terminated name and value and
    // acquires its own reference to an interface value.
    unsafe {
        check(((*(*surface).pVtbl).SetProperty.unwrap())(
            surface,
            to_wide(name).as_ptr(),
            value,
        ))
        .with_context(|| format!("AMF surface property {name}"))
    }
}
/// Put `map` on `surface` as the codec's ROI map, as AMD's SimpleROI does: a
/// host GRAY32 surface of the grid, filled row by row at its pitch.
///
/// # Safety
/// `context` and `surface` are this encoder's, live, and not yet submitted.
pub(crate) unsafe fn attach(
    context: *mut AMFContext,
    surface: *mut AMFSurface,
    codec: u8,
    rows: &[u32],
    width: u32,
    height: u32,
) -> Result<()> {
    anyhow::ensure!(
        width > 0 && rows.len() == width as usize * height as usize,
        "encoder focus map of {} values is not {width}x{height}",
        rows.len()
    );
    // SAFETY: The context allocates an owned host surface; plane 0's native
    // memory holds `height` rows of `pitch` bytes, checked to fit a row, until
    // our reference is released after SetProperty took its own.
    unsafe {
        let mut map = ptr::null_mut();
        check(((*(*context).pVtbl).AllocSurface.unwrap())(
            context,
            AMF_MEMORY_TYPE_AMF_MEMORY_HOST,
            AMF_SURFACE_FORMAT_AMF_SURFACE_GRAY32,
            width as i32,
            height as i32,
            &mut map,
        ))
        .context("AMF ROI map surface")?;
        anyhow::ensure!(!map.is_null(), "AMF returned no ROI map surface");
        let filled = (|| -> Result<()> {
            let plane = ((*(*map).pVtbl).GetPlaneAt.unwrap())(map, 0);
            anyhow::ensure!(!plane.is_null(), "AMF ROI map surface has no plane");
            let v = &*(*plane).pVtbl;
            let native = (v.GetNative.unwrap())(plane).cast::<u8>();
            let pitch = (v.GetHPitch.unwrap())(plane) as usize;
            anyhow::ensure!(
                !native.is_null() && pitch >= width as usize * 4,
                "AMF ROI map surface is not writable host memory"
            );
            for (y, row) in rows.chunks_exact(width as usize).enumerate() {
                ptr::copy_nonoverlapping(
                    row.as_ptr().cast::<u8>(),
                    native.add(y * pitch),
                    row.len() * 4,
                );
            }
            set(
                surface,
                roi_name(codec),
                AMFVariantStruct {
                    type_: AMF_VARIANT_TYPE_AMF_VARIANT_INTERFACE,
                    __bindgen_anon_1: AMFVariantStruct__bindgen_ty_1 {
                        pInterface: map.cast(),
                    },
                },
            )
        })();
        ((*(*map).pVtbl).Release.unwrap())(map);
        filled
    }
}
/// The GRAY32 map named `name` on an output buffer: `None` when AMF set none.
///
/// # Safety
/// `buffer` is a live output buffer.
unsafe fn read_grid(buffer: *mut AMFBuffer, name: &str) -> Result<Option<Grid>> {
    // SAFETY: GetProperty hands over one reference to an interface value, which
    // is released once; QueryInterface adds the surface's own, released after
    // its host plane, converted first if needed, has been copied out.
    unsafe {
        let mut value: AMFVariantStruct = std::mem::zeroed();
        let found =
            ((*(*buffer).pVtbl).GetProperty.unwrap())(buffer, to_wide(name).as_ptr(), &mut value);
        if found != AMF_RESULT_AMF_OK {
            return Ok(None);
        }
        anyhow::ensure!(
            value.type_ == AMF_VARIANT_TYPE_AMF_VARIANT_INTERFACE,
            "{name} is variant type {}, not an interface",
            value.type_
        );
        let interface = value.__bindgen_anon_1.pInterface;
        if interface.is_null() {
            return Ok(None);
        }
        let mut queried = ptr::null_mut();
        let result =
            ((*(*interface).pVtbl).QueryInterface.unwrap())(interface, &SURFACE_IID, &mut queried);
        ((*(*interface).pVtbl).Release.unwrap())(interface);
        check(result).with_context(|| format!("{name} is not a surface"))?;
        let surface = queried.cast::<AMFSurface>();
        anyhow::ensure!(!surface.is_null(), "{name} returned no surface");
        let read = (|| -> Result<Grid> {
            let v = &*(*surface).pVtbl;
            if (v.GetMemoryType.unwrap())(surface) != AMF_MEMORY_TYPE_AMF_MEMORY_HOST {
                check((v.Convert.unwrap())(
                    surface,
                    AMF_MEMORY_TYPE_AMF_MEMORY_HOST,
                ))
                .with_context(|| format!("{name} to host memory"))?;
            }
            let plane = (v.GetPlaneAt.unwrap())(surface, 0);
            anyhow::ensure!(!plane.is_null(), "{name} has no plane");
            let p = &*(*plane).pVtbl;
            let (width, height) = ((p.GetWidth.unwrap())(plane), (p.GetHeight.unwrap())(plane));
            let pitch = (p.GetHPitch.unwrap())(plane) as usize;
            let native = (p.GetNative.unwrap())(plane).cast::<u8>().cast_const();
            anyhow::ensure!(
                width > 0
                    && height > 0
                    && width <= 4096
                    && height <= 4096
                    && !native.is_null()
                    && pitch >= width as usize * 4,
                "{name} is an unreadable {width}x{height} plane"
            );
            let mut values = Vec::with_capacity(width as usize * height as usize);
            for y in 0..height as usize {
                let row = native.add(y * pitch).cast::<u32>();
                values.extend((0..width as usize).map(|x| row.add(x).read_unaligned()));
            }
            Ok(unstride(
                Grid {
                    width: width as u32,
                    height: height as u32,
                    values,
                },
                pitch / 4,
            ))
        })();
        ((*(*surface).pVtbl).Release.unwrap())(surface);
        read.map(Some)
    }
}

/// The startup check's result for one codec and bit depth.
#[derive(Clone, Debug, PartialEq)]
pub struct Probe {
    pub support: Support,
    /// The map's effect: half of the right-minus-left mean QP (AV1 q-index)
    /// with the left half important minus the same with the right half
    /// important, so a difference the picture itself makes cancels.
    pub qp_delta: Option<f32>,
    /// The mean of the two: the right-minus-left difference the picture and
    /// encoder make whatever the map.
    pub control_delta: Option<f32>,
    /// The feedback map's size in blocks.
    pub feedback_grid: Option<(u32, u32)>,
}
impl Probe {
    fn unsupported(reason: Reason, message: impl Into<String>) -> Self {
        Self {
            support: Support::Unsupported {
                reason,
                message: message.into(),
            },
            qp_delta: None,
            control_delta: None,
            feedback_grid: None,
        }
    }
}
/// The map's effect and the picture's own difference from the mean
/// right-minus-left differences with the left half important and with the
/// right half important.
fn effect(left_important: Option<f32>, right_important: Option<f32>) -> (Option<f32>, Option<f32>) {
    match (left_important, right_important) {
        (Some(left), Some(right)) => (Some((left - right) / 2.0), Some((left + right) / 2.0)),
        _ => (None, None),
    }
}
/// The probe's verdict from the map's measured effect.
fn verdict(codec: u8, effect: Option<f32>) -> Support {
    let (scale, unit) = if codec == 2 {
        (Q_INDEX_SCALE, "q-index")
    } else {
        (1.0, "QP")
    };
    let unsupported = |message: String| Support::Unsupported {
        reason: Reason::CodecUnsupported,
        message,
    };
    let Some(effect) = effect else {
        return unsupported(
            "the driver returned no block QP map, so ROI cannot be verified".into(),
        );
    };
    if effect >= ROI_MIN_DELTA * scale {
        Support::Verified { delta: effect }
    } else {
        unsupported(format!(
            "the ROI map moved {unit} by {effect:+.1}, under the {:+.1} required",
            ROI_MIN_DELTA * scale
        ))
    }
}
fn mix(mut value: u32) -> u32 {
    value ^= value >> 16;
    value = value.wrapping_mul(0x7feb_352d);
    value ^= value >> 15;
    value = value.wrapping_mul(0x846c_a68b);
    value ^ (value >> 16)
}
/// A probe frame: 8x8 cells of hashed grey scrolling 3 pixels a frame, with
/// fresh grain, so both halves code the same kind of residual every frame.
fn probe_picture(device: &Device, width: u32, height: u32, frame: u32) -> Result<GpuImage> {
    let mut bytes = vec![0u8; width as usize * height as usize * 4];
    for (y, row) in bytes.chunks_exact_mut(width as usize * 4).enumerate() {
        let y = y as u32;
        for (x, pixel) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let x = x as u32;
            let cell = mix(((x + 3 * frame) / 8) | ((y / 8) << 16)) % 160;
            let grain = mix(x ^ (y << 13) ^ (frame << 26) ^ 0x5bd1_e995) % 24;
            let level = (48 + cell + grain) as u8;
            *pixel = [level, level, level, 255];
        }
    }
    let image = GpuImage::upload(
        device,
        &Image {
            width,
            height,
            stride: width as usize * 4,
            bytes,
            captured: Instant::now(),
            pixel: crate::capture::Pixel::Bgra8,
        },
    )?;
    // Finish the D3D11 upload before a D3D12 conversion reads it.
    image.readback(&mut None)?;
    Ok(image)
}

impl Encoder {
    /// Focus on with `settings`, or off. On needs the D3D12 conversion and
    /// planar 4:2:0, and fails loudly without them.
    pub fn set_focus(&mut self, settings: Option<crate::compute::FocusSettings>) -> Result<()> {
        if settings.is_some() {
            anyhow::ensure!(
                self.config.chroma_packing == 0,
                "encoder focus: {}",
                Reason::Packed444.message()
            );
            let input = self.compute.as_mut().with_context(|| {
                format!(
                    "encoder focus: {}",
                    Reason::D3d12ConversionUnavailable.message()
                )
            })?;
            input
                .converter
                .enable_focus(encoder_focus::block(self.codec))?;
        }
        if let Some(input) = self.compute.as_mut() {
            input.converter.set_focus(settings)?;
        }
        self.focus.settings = settings;
        Ok(())
    }
    /// Ask for block QP feedback on one frame in `every` that carries a map,
    /// for [`Stats::qp_delta`]. Feedback may cost encoder time.
    pub fn set_focus_verify(&mut self, every: Option<u32>) {
        self.focus.verify = every.map(|every| every.max(1));
    }
    pub fn focus_stats(&self) -> Stats {
        let mut stats = self.focus.stats;
        stats.qp_delta = (self.focus.qp_count > 0)
            .then(|| (self.focus.qp_sum / f64::from(self.focus.qp_count)) as f32);
        stats
    }
    /// After `prepare_surface`, before SubmitInput: the newest finished map
    /// from a conversion up to this frame's (`conversion`), unless the frame
    /// is a keyframe, a recovery picture or the encoder's first, or one of the
    /// [`SETTLE_FRAMES`] after them.
    ///
    /// # Safety
    /// `surface` is this frame's live input surface.
    pub(super) unsafe fn attach_focus(
        &mut self,
        surface: *mut AMFSurface,
        idr: bool,
        recover: bool,
        conversion: Option<u64>,
    ) -> Result<()> {
        let first = self.index == 0;
        // Counted with focus off too: turning it on just after a keyframe still waits.
        let settling = settle(&mut self.focus.settled, idr || recover || first);
        let (context, codec) = (self.context, self.codec);
        if let Some(fixed) = &self.focus.fixed {
            if pick(idr, recover, first, false, Some((u64::MAX, false)), 0) == Pick::Attach {
                // SAFETY: Per this function's contract.
                unsafe {
                    attach(
                        context,
                        surface,
                        codec,
                        &fixed.values,
                        fixed.width,
                        fixed.height,
                    )?
                };
            }
        } else if self.focus.settings.is_some() {
            let input = self
                .compute
                .as_ref()
                .context("encoder focus is on without the D3D12 conversion")?;
            let view = conversion.and_then(|conversion| {
                input
                    .converter
                    .latest_map(self.focus.last)
                    .filter(|view| view.value <= conversion)
            });
            let picked = pick(
                idr,
                recover,
                first,
                settling,
                view.as_ref().map(|view| (view.value, view.uniform)),
                self.focus.last,
            );
            count(&mut self.focus.stats, picked);
            let Some(view) = view.filter(|_| matches!(picked, Pick::Attach | Pick::Uniform)) else {
                return Ok(());
            };
            self.focus.last = view.value;
            if picked == Pick::Uniform {
                return Ok(());
            }
            check_grid(codec, self.config.coded_size(), (view.width, view.height))?;
            // SAFETY: Per this function's contract.
            unsafe { attach(context, surface, codec, view.rows, view.width, view.height)? };
            let Some(every) = self.focus.verify else {
                return Ok(());
            };
            self.focus.since_verify += 1;
            if self.focus.since_verify < every {
                return Ok(());
            }
            self.focus.since_verify = 0;
            let importance = Grid {
                width: view.width,
                height: view.height,
                values: view.rows.to_vec(),
            };
            // SAFETY: Per this function's contract.
            unsafe { set(surface, qp_feedback(codec), gpu::boolean(true))? };
            self.focus.asked.push_back(Ask {
                pts: self.index,
                importance: Some(importance),
                keep: false,
            });
            return Ok(());
        }
        if self.focus.keep {
            // SAFETY: Per this function's contract.
            unsafe { set(surface, qp_feedback(codec), gpu::boolean(true))? };
            self.focus.asked.push_back(Ask {
                pts: self.index,
                importance: self.focus.fixed.clone(),
                keep: true,
            });
        }
        Ok(())
    }
    /// Collect the QP map a frame asked for. With slice output it may come on
    /// any of the frame's units; `last` marks the frame's final one.
    ///
    /// # Safety
    /// `buffer` is a live output buffer of this encoder, for `pts`.
    pub(super) unsafe fn focus_feedback(&mut self, buffer: *mut AMFBuffer, pts: i64, last: bool) {
        // Asks older than this output belong to frames that were lost.
        while self.focus.asked.front().is_some_and(|ask| ask.pts < pts) {
            self.focus.asked.pop_front();
        }
        if self.focus.asked.front().is_none_or(|ask| ask.pts != pts) {
            return;
        }
        // SAFETY: Per this function's contract.
        let qp = unsafe { read_grid(buffer, qp_map(self.codec)) };
        if matches!(qp, Ok(None)) && !last {
            return;
        }
        let ask = self.focus.asked.pop_front().expect("checked above");
        if let (Ok(Some(qp)), Some(importance)) = (&qp, &ask.importance)
            && let Some(delta) = importance_delta(qp, importance)
        {
            self.focus.qp_sum += f64::from(delta);
            self.focus.qp_count += 1;
        }
        if ask.keep {
            self.focus
                .answers
                .push(qp.map_err(|error| format!("{error:#}")));
        } else if let Err(error) = qp {
            tracing::warn!(error = %format!("{error:#}"), "encoder focus QP feedback unreadable");
        }
    }
    /// The startup check (design section 7): the ROI cap, then frames with the
    /// left half at 10 and the right at 0, and as many the other way round,
    /// each with block QP feedback. An encoder failure is an error; a driver
    /// that ignores the map is `Unsupported`.
    pub fn probe_focus(&mut self) -> Result<Probe> {
        if self.config.chroma_packing != 0 {
            return Ok(Probe::unsupported(
                Reason::Packed444,
                Reason::Packed444.message(),
            ));
        }
        if let Some(cap) = cap_name(self.codec)
            && self.capability(cap) == Some(0)
        {
            return Ok(Probe::unsupported(
                Reason::CodecUnsupported,
                Reason::CodecUnsupported.message(),
            ));
        }
        let (width, height) = coded_size(self.config.coded_size());
        let (columns, rows) = encoder_focus::grid(self.codec, width, height);
        let half = Grid {
            width: columns,
            height: rows,
            values: (0..columns * rows)
                .map(|index| if index % columns < columns / 2 { 10 } else { 0 })
                .collect(),
        };
        self.focus.answers.clear();
        let measured = self.probe_frames(&half);
        self.focus.fixed = None;
        self.focus.keep = false;
        self.focus.asked.clear();
        measured?;
        let answers = std::mem::take(&mut self.focus.answers);
        anyhow::ensure!(
            answers.len() == 2 * PROBE_FRAMES as usize,
            "the focus probe got {} of {} feedback answers",
            answers.len(),
            2 * PROBE_FRAMES
        );
        let mut grids = Vec::with_capacity(answers.len());
        for answer in answers {
            match answer {
                Ok(Some(grid)) => grids.push(grid),
                // Missing feedback leaves both deltas unknown.
                Ok(None) => {
                    grids.clear();
                    break;
                }
                Err(message) => {
                    return Ok(Probe::unsupported(
                        Reason::CodecUnsupported,
                        format!("the block QP map could not be read: {message}"),
                    ));
                }
            }
        }
        let mean = |grids: &[Grid]| -> Option<f32> {
            let deltas = grids.iter().map(half_delta).collect::<Option<Vec<_>>>()?;
            (!deltas.is_empty()).then(|| deltas.iter().sum::<f32>() / deltas.len() as f32)
        };
        let (qp_delta, control_delta) = if grids.is_empty() {
            (None, None)
        } else {
            let (left, right) = grids.split_at(PROBE_FRAMES as usize);
            effect(mean(left), mean(right))
        };
        Ok(Probe {
            support: verdict(self.codec, qp_delta),
            qp_delta,
            control_delta,
            feedback_grid: grids.first().map(|grid| (grid.width, grid.height)),
        })
    }
    fn probe_frames(&mut self, half: &Grid) -> Result<()> {
        let bitrate = self.config.bitrate_kbps;
        let mirrored = Grid {
            values: half.values.iter().map(|value| 10 - value).collect(),
            ..half.clone()
        };
        for frame in 0..PROBE_WARMUP + 2 * PROBE_FRAMES {
            self.focus.keep = frame >= PROBE_WARMUP;
            self.focus.fixed = (frame >= PROBE_WARMUP).then(|| {
                if frame < PROBE_WARMUP + PROBE_FRAMES {
                    half.clone()
                } else {
                    mirrored.clone()
                }
            });
            let image = probe_picture(&self._device, self.config.width, self.config.height, frame)?;
            let started = Instant::now();
            let output = self.encode_gpu(&image, frame == 0, bitrate)?;
            let mut idr = output.iter().any(|encoded| encoded.idr);
            while self.pending() {
                anyhow::ensure!(
                    started.elapsed() < Duration::from_secs(2),
                    "focus probe frame {frame} was not encoded within two seconds"
                );
                idr |= self.poll()?.iter().any(|encoded| encoded.idr);
            }
            anyhow::ensure!(
                !idr || frame == 0,
                "focus probe frame {frame} came out as a keyframe"
            );
        }
        Ok(())
    }
}

/// The row pitch, in bytes, of a host GRAY32 surface of `width`x`height` as
/// [`attach`] allocates it.
#[cfg(test)]
pub(crate) fn map_pitch(context: *mut AMFContext, width: u32, height: u32) -> Result<usize> {
    // SAFETY: The caller's context is live; the owned surface is released after
    // its plane's pitch is read.
    unsafe {
        let mut map = ptr::null_mut();
        check(((*(*context).pVtbl).AllocSurface.unwrap())(
            context,
            AMF_MEMORY_TYPE_AMF_MEMORY_HOST,
            AMF_SURFACE_FORMAT_AMF_SURFACE_GRAY32,
            width as i32,
            height as i32,
            &mut map,
        ))?;
        let plane = ((*(*map).pVtbl).GetPlaneAt.unwrap())(map, 0);
        let pitch = ((*(*plane).pVtbl).GetHPitch.unwrap())(plane) as usize;
        ((*(*map).pVtbl).Release.unwrap())(map);
        Ok(pitch)
    }
}
#[cfg(test)]
impl Encoder {
    pub(crate) fn context_for_tests(&self) -> *mut AMFContext {
        self.context
    }
    /// Diagnostics: `warmup` frames, then one frame per entry of `maps` (`None`
    /// carries no map), each asking for block QP feedback. Per measured frame:
    /// the QP grid and the coded bytes. `grain` adds fresh noise every frame.
    pub(crate) fn focus_diag(
        &mut self,
        warmup: u32,
        maps: &[Option<Grid>],
        grain: bool,
    ) -> Result<Vec<(Option<Grid>, usize)>> {
        self.focus.answers.clear();
        let bitrate = self.config.bitrate_kbps;
        let (width, height) = (self.config.width, self.config.height);
        let mut sizes = Vec::new();
        let run = (|| -> Result<()> {
            for frame in 0..warmup + maps.len() as u32 {
                let measured = frame.checked_sub(warmup).map(|index| index as usize);
                self.focus.keep = measured.is_some();
                self.focus.fixed = measured.and_then(|index| maps[index].clone());
                let image = if grain {
                    probe_picture(&self._device, width, height, frame)?
                } else {
                    smooth_picture(&self._device, width, height, frame)?
                };
                let started = Instant::now();
                let output = self.encode_gpu(&image, frame == 0, bitrate)?;
                let mut size: usize = output.iter().map(|encoded| encoded.bytes.len()).sum();
                let mut idr = output.iter().any(|encoded| encoded.idr);
                while self.pending() {
                    anyhow::ensure!(
                        started.elapsed() < Duration::from_secs(2),
                        "focus diag frame {frame} was not encoded within two seconds"
                    );
                    for encoded in self.poll()? {
                        size += encoded.bytes.len();
                        idr |= encoded.idr;
                    }
                }
                anyhow::ensure!(
                    !idr || frame == 0,
                    "focus diag frame {frame} was a keyframe"
                );
                if measured.is_some() {
                    sizes.push(size);
                }
            }
            Ok(())
        })();
        self.focus.fixed = None;
        self.focus.keep = false;
        self.focus.asked.clear();
        run?;
        let answers = std::mem::take(&mut self.focus.answers);
        anyhow::ensure!(answers.len() == maps.len(), "{} answers", answers.len());
        answers
            .into_iter()
            .zip(sizes)
            .map(|(answer, size)| Ok((answer.map_err(anyhow::Error::msg)?, size)))
            .collect()
    }
}
/// A diagnostic frame without grain: the probe's scrolling cells alone.
#[cfg(test)]
fn smooth_picture(device: &Device, width: u32, height: u32, frame: u32) -> Result<GpuImage> {
    let mut bytes = vec![0u8; width as usize * height as usize * 4];
    for (y, row) in bytes.chunks_exact_mut(width as usize * 4).enumerate() {
        let y = y as u32;
        for (x, pixel) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let x = x as u32;
            let level = (48 + mix(((x + 3 * frame) / 8) | ((y / 8) << 16)) % 160) as u8;
            *pixel = [level, level, level, 255];
        }
    }
    let image = GpuImage::upload(
        device,
        &Image {
            width,
            height,
            stride: width as usize * 4,
            bytes,
            captured: Instant::now(),
            pixel: crate::capture::Pixel::Bgra8,
        },
    )?;
    image.readback(&mut None)?;
    Ok(image)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_map_on_keyframes_recovery_pictures_or_the_first_frame() {
        let fresh = Some((7, false));
        assert_eq!(pick(true, false, false, false, fresh, 3), Pick::Skip);
        assert_eq!(pick(false, true, false, false, fresh, 3), Pick::Skip);
        assert_eq!(pick(false, false, true, false, fresh, 3), Pick::Skip);
        assert_eq!(pick(false, false, false, false, fresh, 3), Pick::Attach);
        let mut stats = Stats::default();
        for picked in [
            Pick::Skip,
            Pick::Skip,
            Pick::Attach,
            Pick::Stale,
            Pick::Uniform,
        ] {
            count(&mut stats, picked);
        }
        assert_eq!(
            (
                stats.skipped_recovery,
                stats.maps,
                stats.stale,
                stats.uniform
            ),
            (2, 1, 1, 1)
        );
    }

    #[test]
    fn a_map_is_used_once() {
        assert_eq!(
            pick(false, false, false, false, Some((7, false)), 7),
            Pick::Stale
        );
        assert_eq!(
            pick(false, false, false, false, Some((6, false)), 7),
            Pick::Stale
        );
        assert_eq!(pick(false, false, false, false, None, 7), Pick::Stale);
        assert_eq!(
            pick(false, false, false, false, Some((8, false)), 7),
            Pick::Attach
        );
    }

    #[test]
    fn a_uniform_map_is_never_attached() {
        assert_eq!(
            pick(false, false, false, false, Some((8, true)), 7),
            Pick::Uniform
        );
        // A keyframe skips before the map is looked at.
        assert_eq!(
            pick(true, false, false, false, Some((8, true)), 7),
            Pick::Skip
        );
    }

    #[test]
    fn no_map_while_the_rate_control_settles() {
        // Frame 0 is the first, frame 100 a keyframe, frame 200 a recovery
        // picture: each and the SETTLE_FRAMES after it skip a fresh map.
        let mut settled = 0;
        let picks: Vec<Pick> = (0..300u32)
            .map(|frame| {
                let (idr, recover, first) = (frame == 100, frame == 200, frame == 0);
                let settling = settle(&mut settled, idr || recover || first);
                pick(idr, recover, first, settling, Some((u64::MAX, false)), 0)
            })
            .collect();
        for start in [0, 100, 200] {
            let window = start..=start + SETTLE_FRAMES as usize;
            assert!(picks[window].iter().all(|&picked| picked == Pick::Skip));
            let rest = start + SETTLE_FRAMES as usize + 1..start + 100;
            assert!(picks[rest].iter().all(|&picked| picked == Pick::Attach));
        }
        // A settled stream stays settled.
        let mut settled = u32::MAX;
        assert!(!settle(&mut settled, false));
        assert_eq!(settled, u32::MAX);
    }

    #[test]
    fn the_map_grid_matches_the_coded_frame() -> Result<()> {
        // 1080 rows code as 1088: 68 macroblock rows, 17 rows of 64.
        assert_eq!(coded_size((1920, 1080)), (1920, 1088));
        check_grid(0, (1920, 1080), (120, 68))?;
        check_grid(1, (1920, 1080), (30, 17))?;
        check_grid(2, (1920, 1080), (30, 17))?;
        check_grid(0, (1280, 720), (80, 45))?;
        let error = check_grid(0, (1920, 1080), (120, 67)).unwrap_err();
        assert!(
            format!("{error}").contains("1920x1088 in 120x68"),
            "{error}"
        );
        assert!(check_grid(1, (1920, 1080), (120, 68)).is_err());
        assert!(check_grid(0, (3840, 2160), (120, 68)).is_err());
        Ok(())
    }

    #[test]
    fn codec_property_names() {
        assert_eq!(
            [roi_name(0), roi_name(1), roi_name(2)],
            ["ROIData", "HevcROIData", "Av1ROIData"]
        );
        assert_eq!(
            [cap_name(0), cap_name(1), cap_name(2)],
            [Some("ROIMap"), Some("HevcROIMap"), None]
        );
        assert_eq!(qp_feedback(2), "Av1BlockQIndexFeedback");
        assert_eq!(qp_map(1), "HevcBlockQpMap");
    }

    #[test]
    fn half_and_importance_deltas() {
        let qp = Grid {
            width: 5,
            height: 2,
            values: vec![30, 30, 99, 34, 34, 30, 30, 0, 34, 34],
        };
        assert_eq!(half_delta(&qp), Some(4.0));
        let importance = Grid {
            width: 5,
            height: 1,
            values: vec![10, 10, 5, 0, 0],
        };
        assert_eq!(importance_delta(&qp, &importance), Some(4.0));
        let uniform = Grid {
            width: 5,
            height: 1,
            values: vec![6; 5],
        };
        assert_eq!(importance_delta(&qp, &uniform), None);
    }

    /// What the RX 7900 XT's driver returns for `truth`: rows `stride` apart
    /// in its own memory, copied out as if they were `width` apart.
    fn strided(truth: &Grid, stride: usize) -> Grid {
        let width = truth.width as usize;
        let mut source = vec![0; stride * truth.height as usize];
        for (y, row) in truth.values.chunks_exact(width).enumerate() {
            source[y * stride..y * stride + width].copy_from_slice(row);
        }
        source.truncate(truth.values.len());
        Grid {
            values: source,
            ..truth.clone()
        }
    }
    /// QP 24 on the left half and 34 on the right, plus the row number.
    fn halves_grid(width: u32, height: u32) -> Grid {
        Grid {
            width,
            height,
            values: (0..width * height)
                .map(|index| (index / width) + if index % width < width / 2 { 24 } else { 34 })
                .collect(),
        }
    }

    #[test]
    fn strided_qp_maps_are_laid_out_again() {
        // HEVC and AV1 at 1080p: 30x17 blocks, a 1024-byte pitch. Rows 0 and 1
        // survive; at plain layout row 1 reads as zeros and the halves mix.
        let truth = halves_grid(30, 17);
        let read = strided(&truth, 256);
        assert_eq!(&read.values[30..60], &[0; 30]);
        let fixed = unstride(read, 256);
        assert_eq!(fixed.values[..60], truth.values[..60]);
        assert!(fixed.values[60..].iter().all(|&value| value == 0));
        assert_eq!(half_delta(&fixed), Some(10.0));
        // H.264 at 640x480: 40x30 macroblocks, rows 0 to 4 survive.
        let truth = halves_grid(40, 30);
        let fixed = unstride(strided(&truth, 256), 256);
        assert_eq!(fixed.values[..200], truth.values[..200]);
        assert!(fixed.values[200..].iter().all(|&value| value == 0));
        assert_eq!(half_delta(&fixed), Some(10.0));
        let importance = Grid {
            width: 40,
            height: 30,
            values: (0..1200)
                .map(|index| u32::from(index % 40 < 20) * 10)
                .collect(),
        };
        assert_eq!(importance_delta(&fixed, &importance), Some(10.0));
        // 10x8 at 640x480 HEVC: only row 0 fits below the stride.
        let truth = halves_grid(10, 8);
        let fixed = unstride(strided(&truth, 256), 256);
        assert_eq!(fixed.values[..10], truth.values[..10]);
        assert!(fixed.values[10..].iter().all(|&value| value == 0));
    }

    #[test]
    fn plain_qp_maps_are_kept() {
        for (width, height) in [(30, 17), (10, 8), (120, 68), (40, 1)] {
            let truth = halves_grid(width, height);
            assert_eq!(unstride(truth.clone(), 256), truth);
        }
        // A packed pitch is the same either way.
        let truth = halves_grid(64, 4);
        assert_eq!(unstride(truth.clone(), 64), truth);
        assert_eq!(unstride(strided(&truth, 64), 64), truth);
        let empty = Grid {
            width: 30,
            height: 17,
            values: vec![0; 510],
        };
        assert_eq!(half_delta(&unstride(empty, 256)), None);
    }

    #[test]
    fn verdict_needs_the_mirrored_maps_to_differ() {
        // HEVC at 1080p on the RX 7900 XT: +10.0 and -10.1, the picture +0.0.
        assert_eq!(effect(Some(10.0), Some(-10.0)), (Some(10.0), Some(0.0)));
        // A picture that codes its right half 3 QP higher cancels out.
        assert_eq!(effect(Some(7.0), Some(-1.0)), (Some(4.0), Some(3.0)));
        // A driver that ignores the map sees only the picture.
        assert_eq!(effect(Some(1.8), Some(1.8)), (Some(0.0), Some(1.8)));
        assert_eq!(effect(Some(1.0), None), (None, None));
        assert_eq!(verdict(1, Some(3.5)), Support::Verified { delta: 3.5 });
        assert!(matches!(
            verdict(1, Some(1.5)),
            Support::Unsupported {
                reason: Reason::CodecUnsupported,
                ..
            }
        ));
        // A map applied the wrong way round does not verify.
        assert!(matches!(
            verdict(0, Some(-10.0)),
            Support::Unsupported { .. }
        ));
        // AV1 q-index steps are four to a QP.
        assert!(matches!(verdict(2, Some(6.0)), Support::Unsupported { .. }));
        assert_eq!(verdict(2, Some(9.0)), Support::Verified { delta: 9.0 });
        let Support::Unsupported { message, .. } = verdict(0, None) else {
            panic!("no feedback must not verify");
        };
        assert!(message.contains("no block QP map"), "{message}");
    }
}
