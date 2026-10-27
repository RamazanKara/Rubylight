//! HDR dynamic metadata statistics on the compute queue
//! (`butterpollo_core::hdr_dynamic`): once an HDR frame went to the encoder,
//! [`Converter::submit_hdr_stats`] runs a list of its own that reads the same
//! source through the slot's views and constants, histograms PQ(maxRGB) on a
//! grid of at most 240x135 samples and copies it into the slot's readback
//! buffer. Nothing waits for it on the CPU: [`Converter::latest_hdr_stats`]
//! returns the newest finished result, so a late pass only keeps the previous
//! one current.
//!
//! The pass and its readback follow Foundation Sunshine's
//! hdr_luminance_analysis_cs.hlsl (AlkaidLab/foundation-sunshine, commit
//! 880f1ef). Copyright (c) Foundation Sunshine authors, GPL-3.0.
use super::*;
use butterpollo_core::hdr_dynamic::FrameStats;

/// Equal PQ bins over 0..=1, as [`FrameStats::from_histogram`] takes them.
pub const HDR_STATS_BINS: usize = 1024;
/// The samples' minimum and maximum PQ(maxRGB) as `f32` bits, then the bins.
const WORDS: usize = 2 + HDR_STATS_BINS;
const BYTES: u64 = 4 * WORDS as u64;
/// The most sample cells across and down: 16x16 pixel cells on a 4K source.
const GRID: (u32, u32) = (240, 135);
/// `Slot::stats` while `convert` has left a pass for
/// [`Converter::submit_hdr_stats`]; `u64::MAX` marks one being submitted.
pub(super) const QUEUED: u64 = u64::MAX - 1;

/// Sample cells across and down a `width` x `height` source, one per pixel
/// on a smaller one.
fn grid(width: u32, height: u32) -> (u32, u32) {
    (width.clamp(1, GRID.0), height.clamp(1, GRID.1))
}

/// A slot's `Slot::stats` once [`Compute::execute`] ran its pass, or failed
/// to. A failed pass leaves no fence value for the slot's next conversion to
/// wait for, and the shared queue that conversion also uses stays as
/// `execute` left it: `execute` retires it itself only when the queue's
/// ordering broke (a failed signal), as for any conversion.
fn executed(stats: &mut u64, executed: Result<u64>) -> Result<u64> {
    *stats = *executed.as_ref().unwrap_or(&0);
    executed
}

/// One finished statistics pass over a converted frame.
#[derive(Clone, Debug, PartialEq)]
pub struct HdrStatsReadout {
    /// The fence value of the conversion it measured ([`Converted::conversion`]).
    pub conversion: u64,
    /// [`HDR_STATS_BINS`] counts of PQ(maxRGB), one per sample.
    pub histogram: Vec<u32>,
    pub min_pq: f32,
    pub max_pq: f32,
}
impl HdrStatsReadout {
    /// The pass's `words` for `samples` cells. A readout that disagrees with
    /// itself is an error rather than a frame without statistics.
    fn parse(conversion: u64, samples: u64, words: &[u32]) -> Result<Self> {
        anyhow::ensure!(
            words.len() == WORDS,
            "HDR statistics readout has {} words, not {WORDS}",
            words.len()
        );
        let (min_pq, max_pq) = (f32::from_bits(words[0]), f32::from_bits(words[1]));
        let histogram = words[2..].to_vec();
        let counted: u64 = histogram.iter().map(|&count| u64::from(count)).sum();
        // The shader's own binning: scaling by 1024 is exact, `as` truncates like `uint()`.
        let bin = |pq: f32| ((pq * HDR_STATS_BINS as f32) as usize).min(HDR_STATS_BINS - 1);
        let first = histogram.iter().position(|&count| count > 0);
        let last = histogram.iter().rposition(|&count| count > 0);
        anyhow::ensure!(
            counted == samples
                && (0. ..=1.).contains(&min_pq)
                && (min_pq..=1.).contains(&max_pq)
                && first == Some(bin(min_pq))
                && last == Some(bin(max_pq)),
            "HDR statistics readout is inconsistent: {counted} of {samples} samples, \
             PQ {min_pq}..={max_pq}, bins {first:?}..={last:?}"
        );
        Ok(Self {
            conversion,
            histogram,
            min_pq,
            max_pq,
        })
    }
    /// The statistics the metadata writers take.
    pub fn frame_stats(&self) -> Option<FrameStats> {
        FrameStats::from_histogram(&self.histogram, self.min_pq, self.max_pq)
    }
}

struct StatsSlot {
    allocator: ID3D12CommandAllocator,
    list: ID3D12GraphicsCommandList,
    /// Persistently mapped READBACK copy of the words.
    readback: ID3D12Resource,
    mapped: *const u32,
    /// The conversion and grid `convert` left for
    /// [`Converter::submit_hdr_stats`].
    queued: Option<(u64, (u32, u32))>,
    /// The conversion the slot's last pass measured and its sample count;
    /// `None` once the slot converts again.
    measured: Option<(u64, u64)>,
}
/// The statistics pass's GPU objects.
pub(super) struct HdrStats {
    root: ID3D12RootSignature,
    clear: ID3D12PipelineState,
    pass: ID3D12PipelineState,
    /// The words the pass accumulates into, in COMMON between passes.
    words: ID3D12Resource,
    slots: Vec<StatsSlot>,
    /// [`Converter::stop_hdr_stats`]: no more passes are queued.
    stopped: bool,
}
/// Parameters 2 and 3 of the statistics root signature, after the
/// conversion's constants (b0) and source views (t0, t1): the words (u7) and
/// `cbuffer Stats` (b3).
fn stats_parameters() -> [D3D12_ROOT_PARAMETER; 2] {
    [
        D3D12_ROOT_PARAMETER {
            ParameterType: D3D12_ROOT_PARAMETER_TYPE_UAV,
            Anonymous: D3D12_ROOT_PARAMETER_0 {
                Descriptor: D3D12_ROOT_DESCRIPTOR {
                    ShaderRegister: 7,
                    RegisterSpace: 0,
                },
            },
            ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
        },
        D3D12_ROOT_PARAMETER {
            ParameterType: D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS,
            Anonymous: D3D12_ROOT_PARAMETER_0 {
                Constants: D3D12_ROOT_CONSTANTS {
                    ShaderRegister: 3,
                    RegisterSpace: 0,
                    Num32BitValues: 2,
                },
            },
            ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
        },
    ]
}
impl HdrStats {
    /// The clear, the pass over a `grid` of the source in `views` (on `heap`)
    /// with the conversion's constants at `config`, and the copy into
    /// `readback`.
    ///
    /// # Safety
    /// `list` is open; `views` and `config` hold a recorded conversion's
    /// source views and constants, unchanged until the list has run;
    /// `readback` is a READBACK buffer of at least `BYTES` in COPY_DEST; the
    /// words are in COMMON.
    pub(super) unsafe fn record(
        &self,
        list: &ID3D12GraphicsCommandList,
        heap: &ID3D12DescriptorHeap,
        views: D3D12_GPU_DESCRIPTOR_HANDLE,
        config: u64,
        grid: (u32, u32),
        readback: &ID3D12Resource,
    ) {
        // SAFETY: Per the contract above; the words are live and move through the states named.
        unsafe {
            let mut barriers = [transition(
                &self.words,
                D3D12_RESOURCE_STATE_COMMON,
                D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
            )];
            list.ResourceBarrier(&barriers);
            drop_barriers(&mut barriers);
            list.SetComputeRootSignature(&self.root);
            list.SetDescriptorHeaps(&[Some(heap.clone())]);
            list.SetComputeRootConstantBufferView(0, config);
            list.SetComputeRootDescriptorTable(1, views);
            list.SetComputeRootUnorderedAccessView(2, self.words.GetGPUVirtualAddress());
            let constants = [grid.0, grid.1];
            list.SetComputeRoot32BitConstants(3, 2, constants.as_ptr().cast(), 0);
            list.SetPipelineState(&self.clear);
            list.Dispatch((WORDS as u32).div_ceil(64), 1, 1);
            list.ResourceBarrier(&[uav_barrier()]);
            list.SetPipelineState(&self.pass);
            list.Dispatch(grid.0.div_ceil(16), grid.1.div_ceil(16), 1);
            let mut barriers = [transition(
                &self.words,
                D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                D3D12_RESOURCE_STATE_COPY_SOURCE,
            )];
            list.ResourceBarrier(&barriers);
            drop_barriers(&mut barriers);
            list.CopyBufferRegion(readback, 0, &self.words, 0, BYTES);
            let mut barriers = [transition(
                &self.words,
                D3D12_RESOURCE_STATE_COPY_SOURCE,
                D3D12_RESOURCE_STATE_COMMON,
            )];
            list.ResourceBarrier(&barriers);
            drop_barriers(&mut barriers);
        }
    }
    pub(super) fn retain(&self, work: &mut RetainedWork) {
        work.keep(&self.root);
        work.keep(&self.clear);
        work.keep(&self.pass);
        work.keep(&self.words);
        for slot in &self.slots {
            work.keep(&slot.allocator);
            work.keep(&slot.list);
            work.keep(&slot.readback);
        }
    }
    pub(super) fn unmap(&self) {
        for slot in &self.slots {
            // SAFETY: Each buffer was mapped once in `enable_hdr_stats`; nothing reads it after
            // this.
            unsafe { slot.readback.Unmap(0, None) };
        }
    }
}
impl Converter {
    /// Measure every HDR conversion from now on, for
    /// [`Converter::latest_hdr_stats`]. `convert` only: PyroWave's planes
    /// carry no dynamic metadata.
    pub fn enable_hdr_stats(&mut self) -> Result<()> {
        if let Some(stats) = &self.hdr_stats {
            anyhow::ensure!(
                !stats.stopped,
                "HDR statistics were stopped after a failed pass"
            );
            return Ok(());
        }
        self.compute.ensure_active()?;
        let device = &self.compute.device;
        let root = root_signature_with(device, 0, 0, &stats_parameters())?;
        let clear = pipeline(device, &root, b"hdr_stats_clear_cs\0")?;
        let pass = pipeline(device, &root, b"hdr_stats_cs\0")?;
        let words = focus_buffer(
            device,
            D3D12_HEAP_TYPE_DEFAULT,
            BYTES,
            D3D12_RESOURCE_STATE_COMMON,
        )?;
        let mut slots = Vec::with_capacity(SLOTS);
        for _ in 0..SLOTS {
            // SAFETY: Calls on the live device; the list is closed until a pass records it, and
            // a READBACK buffer stays mapped for its life (`unmap` ends it in Drop).
            unsafe {
                let allocator: ID3D12CommandAllocator =
                    device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_COMPUTE)?;
                let list: ID3D12GraphicsCommandList = device.CreateCommandList(
                    0,
                    D3D12_COMMAND_LIST_TYPE_COMPUTE,
                    &allocator,
                    None,
                )?;
                list.Close()?;
                let readback = focus_buffer(
                    device,
                    D3D12_HEAP_TYPE_READBACK,
                    BYTES,
                    D3D12_RESOURCE_STATE_COPY_DEST,
                )?;
                let mut mapped = std::ptr::null_mut();
                readback.Map(0, None, Some(&mut mapped))?;
                slots.push(StatsSlot {
                    allocator,
                    list,
                    readback,
                    mapped: mapped.cast_const().cast::<u32>(),
                    queued: None,
                    measured: None,
                });
            }
        }
        self.hdr_stats = Some(Box::new(HdrStats {
            root,
            clear,
            pass,
            words,
            slots,
            stopped: false,
        }));
        Ok(())
    }
    /// No more statistics passes: dynamic metadata is off for the rest of
    /// the stream. Passes left queued are dropped, so their slots hand the
    /// capture back once the conversion has run; submitted ones keep their
    /// fence, and their resources stay alive until the converter retires them.
    pub fn stop_hdr_stats(&mut self) {
        let Some(stats) = self.hdr_stats.as_deref_mut() else {
            return;
        };
        stats.stopped = true;
        for (slot, pass) in self.slots.iter_mut().zip(&mut stats.slots) {
            if pass.queued.take().is_some() {
                slot.stats = 0;
            }
        }
    }
    /// `slot` converts again: its last pass has finished (`Slot::last`) and a
    /// pass still queued is dropped.
    pub(super) fn reset_hdr_stats(&mut self, slot: usize) {
        self.slots[slot].stats = 0;
        if let Some(stats) = self.hdr_stats.as_deref_mut() {
            stats.slots[slot].queued = None;
            stats.slots[slot].measured = None;
        }
    }
    /// After `convert` queued the encoder's signal for `slot`'s `conversion`:
    /// leave the statistics pass over its source for
    /// [`Converter::submit_hdr_stats`], when enabled and the stream is HDR.
    pub(super) fn queue_hdr_stats(&mut self, slot: usize, conversion: u64) {
        let Some(stats) = self.hdr_stats.as_deref_mut() else {
            return;
        };
        // A capture read on another queue's timeline would have to wait for
        // the pass on the CPU (`Ready::read_by`).
        let foreign = self.slots[slot]
            .capture
            .as_ref()
            .and_then(|capture| capture.ready.as_ref())
            .is_some_and(|ready| {
                ready.readers.is_some() && ready.reader_fence.as_ref() != Some(&self.compute.fence)
            });
        // cbuffer Config: hdr=5.
        if self.values[5] == 0 || foreign || stats.stopped {
            return;
        }
        stats.slots[slot].queued = Some((conversion, grid(self.values[0], self.values[1])));
        self.slots[slot].stats = QUEUED;
    }
    /// Submit the statistics passes `convert` left, oldest first. Call it
    /// once the frame went to the encoder: nothing in the conversion's path
    /// records, submits or waits for a pass. A pass not submitted before its
    /// slot converts again is dropped.
    pub fn submit_hdr_stats(&mut self) -> Result<()> {
        loop {
            let Some(stats) = self.hdr_stats.as_deref_mut() else {
                return Ok(());
            };
            let Some((slot, (conversion, grid))) = (0..SLOTS)
                .filter_map(|slot| stats.slots[slot].queued.map(|queued| (slot, queued)))
                .min_by_key(|&(_, (conversion, _))| conversion)
            else {
                return Ok(());
            };
            stats.slots[slot].queued = None;
            self.slots[slot].stats = 0;
            let StatsSlot {
                allocator,
                list,
                readback,
                ..
            } = &stats.slots[slot];
            // SAFETY: `convert` waited for the slot's previous lists (`Slot::last`) before
            // writing this conversion's views and constants, and the slot keeps them and its
            // capture until it converts again, after this list has run.
            unsafe {
                let views = D3D12_GPU_DESCRIPTOR_HANDLE {
                    ptr: self.heap.GetGPUDescriptorHandleForHeapStart().ptr
                        + (slot * VIEWS * self.increment as usize) as u64,
                };
                let config = self.constants.GetGPUVirtualAddress() + (slot * 256) as u64;
                allocator.Reset()?;
                list.Reset(allocator, None)?;
                stats.record(list, &self.heap, views, config, grid, readback);
                list.Close()?;
            }
            self.slots[slot].stats = u64::MAX;
            let fence = executed(&mut self.slots[slot].stats, self.compute.execute(list))?;
            stats.slots[slot].measured = Some((conversion, u64::from(grid.0) * u64::from(grid.1)));
            // The capture's pool copy must not overwrite the source before the pass has read it.
            if let Some(ready) = self.slots[slot]
                .capture
                .as_ref()
                .and_then(|capture| capture.ready.as_ref())
            {
                ready.read_by(&self.compute, fence)?;
            }
        }
    }
    /// The newest finished statistics whose conversion is above `newer_than`.
    /// Only asks the fence: never waits.
    pub fn latest_hdr_stats(&self, newer_than: u64) -> Result<Option<HdrStatsReadout>> {
        let Some(stats) = self.hdr_stats.as_deref() else {
            return Ok(None);
        };
        let newest = (0..SLOTS)
            .filter_map(|slot| stats.slots[slot].measured.map(|measured| (slot, measured)))
            .filter(|&(slot, (conversion, _))| {
                conversion > newer_than && self.compute.completed(self.slots[slot].stats)
            })
            .max_by_key(|&(_, (conversion, _))| conversion);
        let Some((slot, (conversion, samples))) = newest else {
            return Ok(None);
        };
        // SAFETY: The slot's copy has completed and the next one needs `&mut self` (`convert`),
        // so the mapped words stay unchanged while they are read.
        let words = unsafe { std::slice::from_raw_parts(stats.slots[slot].mapped, WORDS) };
        HdrStatsReadout::parse(conversion, samples, words).map(Some)
    }
}
#[cfg(test)]
#[path = "compute_hdr_stats_tests.rs"]
mod tests;
