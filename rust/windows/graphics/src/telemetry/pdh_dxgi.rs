//! Per-adapter GPU engine load from Windows performance counters (PDH), matched to DXGI adapters
//! by LUID. Adds `gpu_encoder_percent` to ADLX's AMD samples and builds whole samples for
//! adapters ADLX does not cover (NVIDIA, Intel). Runs only on the host's telemetry worker.
//!
//! Counters (English names through `PdhAddEnglishCounterW`, so any Windows display language works):
//! `\GPU Engine(*)\Utilization Percentage` and `\GPU Adapter Memory(*)\Dedicated Usage`. Each
//! instance name carries `luid_0xHIGH_0xLOW`; readings are never summed across adapters.
//! Engine types are the names the driver gives its engines: NVIDIA's NVENC reports
//! `VideoEncode`, AMD's unified VCN reports `Video Codec Engine` (seen on a Radeon RX 7900 XT;
//! that engine also decodes), and Intel's drivers fold encoding into engines named
//! `VideoDecode`, which this module does not count, so Intel GPUs without an encode engine get no
//! `gpu_encoder_percent`.
use super::{GpuSample, wanted};
use std::collections::BTreeMap;
use windows::{
    Win32::{
        Graphics::Dxgi::{CreateDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE, IDXGIFactory1},
        System::Performance::{
            PDH_CSTATUS_NEW_DATA, PDH_CSTATUS_VALID_DATA, PDH_FMT_COUNTERVALUE_ITEM_W,
            PDH_FMT_DOUBLE, PDH_HCOUNTER, PDH_HQUERY, PDH_MORE_DATA, PdhAddEnglishCounterW,
            PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterArrayW, PdhOpenQueryW,
        },
    },
    core::{PCWSTR, w},
};

pub(crate) const ENCODER: &str = "gpu_encoder_percent";
const MICROSOFT: u32 = 0x1414;

/// A hardware DXGI adapter, enumerated once when the collector starts.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Adapter {
    pub(crate) luid: u64,
    pub(crate) name: String,
    pub(crate) dedicated_bytes: u64,
}

/// Hardware adapters in DXGI order; software and Microsoft adapters are skipped.
pub(crate) fn adapters() -> Vec<Adapter> {
    let mut result = vec![];
    // SAFETY: The factory and adapters are live COM objects returned by these calls, and GetDesc1
    // returns an owned struct.
    unsafe {
        let Ok(factory) = CreateDXGIFactory1::<IDXGIFactory1>() else {
            return result;
        };
        for index in 0..32 {
            let Ok(adapter) = factory.EnumAdapters1(index) else {
                break;
            };
            let Ok(desc) = adapter.GetDesc1() else {
                continue;
            };
            if desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 || desc.VendorId == MICROSOFT {
                continue;
            }
            let length = desc.Description.iter().position(|&c| c == 0);
            let length = length.unwrap_or(desc.Description.len());
            result.push(Adapter {
                luid: luid(desc.AdapterLuid.HighPart as u32, desc.AdapterLuid.LowPart),
                name: String::from_utf16_lossy(&desc.Description[..length]),
                dedicated_bytes: desc.DedicatedVideoMemory as u64,
            });
        }
    }
    result
}

fn luid(high: u32, low: u32) -> u64 {
    (u64::from(high) << 32) | u64::from(low)
}

/// What one PDH instance name says about its engine or memory segment.
#[derive(Debug, PartialEq)]
pub(crate) struct Instance<'a> {
    luid: u64,
    phys: u32,
    engine: Option<u32>,
    engine_type: &'a str,
}

/// Parses `pid_1_luid_0x00000000_0x0000D1A4_phys_0_eng_3_engtype_3D` and
/// `luid_0x00000000_0x0000D1A4_phys_0`; anything without a LUID is rejected.
pub(crate) fn parse_instance(name: &str) -> Option<Instance<'_>> {
    let rest = &name[name.find("luid_0x")? + "luid_0x".len()..];
    let (high, rest) = rest.split_once("_0x")?;
    let low_end = rest.find('_').unwrap_or(rest.len());
    let (low, rest) = rest.split_at(low_end);
    let high = u32::from_str_radix(high, 16).ok()?;
    let low = u32::from_str_radix(low, 16).ok()?;
    let number = |key: &str| {
        let at = rest.find(key)? + key.len();
        let digits = rest[at..].split('_').next()?;
        digits.parse::<u32>().ok()
    };
    let engine_type = rest
        .find("engtype_")
        .map_or("", |at| &rest[at + "engtype_".len()..]);
    Some(Instance {
        luid: luid(high, low),
        phys: number("phys_").unwrap_or(0),
        engine: number("eng_"),
        engine_type,
    })
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    Encoder,
    Render,
}

/// The engine type with spaces and engine numbers removed, e.g. `Video Encode 1` -> `videoencode`.
fn kind(engine_type: &str) -> Option<Kind> {
    let name: String = engine_type
        .chars()
        .filter(|c| c.is_ascii_alphabetic())
        .map(|c| c.to_ascii_lowercase())
        .collect();
    if name.contains("encode") || name.contains("codec") {
        Some(Kind::Encoder)
    } else if engine_type.trim().eq_ignore_ascii_case("3d") {
        Some(Kind::Render)
    } else {
        None
    }
}

/// One adapter's readings; `None` where PDH had nothing for it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Load {
    pub(crate) encoder_percent: Option<f64>,
    pub(crate) render_percent: Option<f64>,
    pub(crate) dedicated_bytes: Option<f64>,
}

/// Per-adapter load. Engine readings are per process: they add up per engine, and the busiest
/// engine of a kind is the adapter's load for that kind, as Task Manager shows it, capped at 100.
pub(crate) fn aggregate(
    engines: &[(String, f64)],
    memory: &[(String, f64)],
) -> BTreeMap<u64, Load> {
    let mut per_engine: BTreeMap<(u64, u32, u32, u8), f64> = BTreeMap::new();
    for (name, value) in engines {
        let Some(instance) = parse_instance(name) else {
            continue;
        };
        let Some(kind) = kind(instance.engine_type) else {
            continue;
        };
        if !value.is_finite() || *value < 0. {
            continue;
        }
        let key = (
            instance.luid,
            instance.phys,
            instance.engine.unwrap_or(0),
            kind as u8,
        );
        *per_engine.entry(key).or_default() += value;
    }
    let mut loads: BTreeMap<u64, Load> = BTreeMap::new();
    for ((luid, _, _, kind), value) in per_engine {
        let load = loads.entry(luid).or_default();
        let slot = if kind == Kind::Encoder as u8 {
            &mut load.encoder_percent
        } else {
            &mut load.render_percent
        };
        *slot = Some(slot.unwrap_or(0.).max(value.min(100.)));
    }
    for (name, value) in memory {
        if let Some(instance) = parse_instance(name)
            && value.is_finite()
            && *value >= 0.
        {
            let load = loads.entry(instance.luid).or_default();
            load.dedicated_bytes = Some(load.dedicated_bytes.unwrap_or(0.) + value);
        }
    }
    loads
}

/// A stable id for an adapter ADLX does not list: negative, so it stays apart from ADLX ids.
pub(crate) fn synthetic_id(luid: u64) -> i32 {
    i32::MIN | ((luid as u32 ^ (luid >> 32) as u32) & 0x7FFF_FFFF) as i32
}

/// Pairs each adapter with the ADLX sample of the same name, in order, so two identical cards
/// pair up one to one. Returns, per adapter, the index of its ADLX sample.
pub(crate) fn pair(adapters: &[Adapter], names: &[&str]) -> Vec<Option<usize>> {
    let mut taken = vec![false; names.len()];
    adapters
        .iter()
        .map(|adapter| {
            let index = (0..names.len()).find(|&i| !taken[i] && names[i] == adapter.name)?;
            taken[index] = true;
            Some(index)
        })
        .collect()
}

/// Adds the encoder load to ADLX samples and appends samples for adapters ADLX did not report.
/// An adapter without any engine reading (an indirect display, say) gets no sample.
pub(crate) fn merge(
    gpus: &mut Vec<GpuSample>,
    adapters: &[Adapter],
    loads: &BTreeMap<u64, Load>,
    selected: &[String],
) {
    let names: Vec<String> = gpus.iter().map(|gpu| gpu.name.clone()).collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let pairs = pair(adapters, &names);
    let percent = |value: Option<f64>| value.filter(|v| v.is_finite() && *v >= 0.);
    for (adapter, paired) in adapters.iter().zip(pairs) {
        let load = loads.get(&adapter.luid).copied().unwrap_or_default();
        if let Some(index) = paired {
            if wanted(selected, ENCODER)
                && let Some(value) = percent(load.encoder_percent)
            {
                gpus[index].metrics.insert(ENCODER.into(), value);
            }
            continue;
        }
        if load.encoder_percent.is_none() && load.render_percent.is_none() {
            continue;
        }
        let mut sample = GpuSample {
            id: synthetic_id(adapter.luid),
            name: adapter.name.clone(),
            metrics: BTreeMap::new(),
        };
        let mut add = |name: &str, value: Option<f64>| {
            if wanted(selected, name)
                && let Some(value) = percent(value)
            {
                sample.metrics.insert(name.into(), value);
            }
        };
        add("gpu_usage_percent", load.render_percent);
        add(ENCODER, load.encoder_percent);
        if let Some(bytes) = load.dedicated_bytes {
            add("gpu_vram_used_mb", Some(bytes / (1024. * 1024.)));
            if adapter.dedicated_bytes > 0 {
                add(
                    "gpu_vram_usage_percent",
                    Some(100. * bytes / adapter.dedicated_bytes as f64),
                );
            }
        }
        gpus.push(sample);
    }
}

/// One open PDH query; dropping it closes the query.
pub(crate) struct Pdh {
    query: PDH_HQUERY,
    engines: PDH_HCOUNTER,
    memory: PDH_HCOUNTER,
    primed: bool,
}

// SAFETY: The query is used only by the telemetry worker that owns the collector; PDH handles
// are not tied to the creating thread.
unsafe impl Send for Pdh {}

impl Pdh {
    pub(crate) fn open() -> Option<Self> {
        let mut query = PDH_HQUERY::default();
        // SAFETY: A null data source selects live data; the output is a live handle slot.
        if unsafe { PdhOpenQueryW(PCWSTR::null(), 0, &mut query) } != 0 {
            return None;
        }
        let mut pdh = Self {
            query,
            engines: PDH_HCOUNTER::default(),
            memory: PDH_HCOUNTER::default(),
            primed: false,
        };
        // SAFETY: The query is open, the paths are static UTF-16 strings and the outputs are live.
        let added = unsafe {
            PdhAddEnglishCounterW(
                pdh.query,
                w!("\\GPU Engine(*)\\Utilization Percentage"),
                0,
                &mut pdh.engines,
            ) == 0
                && PdhAddEnglishCounterW(
                    pdh.query,
                    w!("\\GPU Adapter Memory(*)\\Dedicated Usage"),
                    0,
                    &mut pdh.memory,
                ) == 0
        };
        if !added {
            tracing::debug!("GPU performance counters unavailable");
            return None;
        }
        Some(pdh)
    }

    /// One collection. The first one only sets the baseline the utilization rates need, so its
    /// engine readings are left out instead of waiting for a second collection.
    pub(crate) fn sample(&mut self) -> Option<BTreeMap<u64, Load>> {
        // SAFETY: The query is open and owned by this collector.
        if unsafe { PdhCollectQueryData(self.query) } != 0 {
            return None;
        }
        let engines = if self.primed {
            values(self.engines)
        } else {
            vec![]
        };
        self.primed = true;
        Some(aggregate(&engines, &values(self.memory)))
    }
}

impl Drop for Pdh {
    fn drop(&mut self) {
        // SAFETY: The query is open and closed exactly once; its counters close with it.
        unsafe {
            PdhCloseQuery(self.query);
        }
    }
}

/// Every instance of a wildcard counter with valid data, as (instance name, value).
fn values(counter: PDH_HCOUNTER) -> Vec<(String, f64)> {
    let (mut bytes, mut count) = (0u32, 0u32);
    // SAFETY: A null buffer with size 0 asks for the size the next call needs.
    let status = unsafe {
        PdhGetFormattedCounterArrayW(counter, PDH_FMT_DOUBLE, &mut bytes, &mut count, None)
    };
    if status != PDH_MORE_DATA || bytes == 0 {
        return vec![];
    }
    let item = size_of::<PDH_FMT_COUNTERVALUE_ITEM_W>();
    let mut buffer = vec![PDH_FMT_COUNTERVALUE_ITEM_W::default(); (bytes as usize).div_ceil(item)];
    // SAFETY: The buffer holds at least `bytes` bytes, aligned for the items PDH writes first;
    // the instance names it points into follow in the same buffer.
    let status = unsafe {
        PdhGetFormattedCounterArrayW(
            counter,
            PDH_FMT_DOUBLE,
            &mut bytes,
            &mut count,
            Some(buffer.as_mut_ptr()),
        )
    };
    if status != 0 {
        return vec![];
    }
    buffer
        .iter()
        .take((count as usize).min(buffer.len()))
        .filter(|item| {
            matches!(
                item.FmtValue.CStatus,
                PDH_CSTATUS_VALID_DATA | PDH_CSTATUS_NEW_DATA
            )
        })
        .filter_map(|item| {
            // SAFETY: PDH wrote a NUL-terminated name inside `buffer` and a double value.
            let name = unsafe { item.szName.to_string() }.ok()?;
            Some((name, unsafe { item.FmtValue.Anonymous.doubleValue }))
        })
        .collect()
}

#[cfg(test)]
#[path = "pdh_dxgi_tests.rs"]
mod tests;
