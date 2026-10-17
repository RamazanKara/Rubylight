//! What a virtual monitor advertises and which id names it: mode lists, the timing numbers
//! behind one `(width, height, refresh)`, monitor ids and container GUIDs.
//!
//! Adapted from punktfunk's `pf_driver_proto::vdisplay` (Enrico Bühler (unom), MIT OR
//! Apache-2.0). Refresh rates are millihertz here, as the Rubylight host keeps them, so a
//! 59.94 Hz client gets a 59.94 Hz mode instead of a rounded one.

use alloc::vec;
use alloc::vec::Vec;

/// Largest width or height the driver accepts.
pub const MAX_DIMENSION: u32 = 16384;
/// Slowest and fastest refresh, in millihertz: 1 Hz to 1000 Hz, so 120, 240 and 480 Hz fit.
pub const MIN_REFRESH_MHZ: u32 = 1_000;
pub const MAX_REFRESH_MHZ: u32 = 1_000_000;
/// Monitor ids are `1..=MAX_MONITOR_ID`: the IddCx connector index must stay below the
/// adapter's `MaxMonitorsSupported` of 16.
pub const MAX_MONITOR_ID: u32 = 15;
/// The adapter's `MaxMonitorsSupported`.
pub const MAX_MONITORS: u32 = 16;

/// One resolution and the refresh rates (millihertz) it offers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mode {
    pub width: u32,
    pub height: u32,
    pub refresh_mhz: Vec<u32>,
}

/// One `(width, height, refresh)`: a mode list flattened over its refresh rates.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ModeItem {
    pub width: u32,
    pub height: u32,
    pub refresh_mhz: u32,
}

/// Flatten a mode list in the order the mode DDIs report it.
pub fn flatten(modes: &[Mode]) -> impl Iterator<Item = ModeItem> + '_ {
    modes.iter().flat_map(|m| {
        m.refresh_mhz.iter().map(|&refresh_mhz| ModeItem {
            width: m.width,
            height: m.height,
            refresh_mhz,
        })
    })
}

/// How many resolutions a monitor's list may accumulate across resizes.
pub const MODE_LIST_CAP: usize = 12;

/// Append missing resolutions up to [`MODE_LIST_CAP`] and merge missing refresh rates.
/// Windows pins a monitor's settable set when it arrives, so retain historical rates when
/// a resolution becomes a fallback. Existing order, including the preferred mode, stays.
pub fn union_modes(into: &mut Vec<Mode>, from: &[Mode]) {
    for m in from {
        if let Some(existing) = into
            .iter_mut()
            .find(|e| (e.width, e.height) == (m.width, m.height))
        {
            for &rate in &m.refresh_mhz {
                if !existing.refresh_mhz.contains(&rate) {
                    existing.refresh_mhz.push(rate);
                }
            }
        } else if into.len() < MODE_LIST_CAP {
            into.push(m.clone());
        }
    }
}

/// The list a monitor advertises: the requested mode first, then the fallbacks, then
/// whatever it advertised before (`history`, empty when it is created).
#[must_use]
pub fn advertised_modes(requested: Mode, history: &[Mode]) -> Vec<Mode> {
    let mut modes = vec![requested];
    union_modes(&mut modes, &default_modes());
    union_modes(&mut modes, history);
    modes
}

/// Fallbacks after the requested mode, so a topology change still finds a mode.
/// Include 1440p even when a client requests 4K, allowing games to render at a
/// lower desktop resolution while the negotiated stream stays at 4K.
#[must_use]
pub fn default_modes() -> Vec<Mode> {
    vec![
        Mode {
            width: 2560,
            height: 1440,
            refresh_mhz: vec![60_000, 120_000, 240_000],
        },
        Mode {
            width: 1920,
            height: 1080,
            refresh_mhz: vec![60_000, 120_000],
        },
        Mode {
            width: 1280,
            height: 720,
            refresh_mhz: vec![60_000],
        },
    ]
}

/// The numbers of a `DISPLAYCONFIG_VIDEO_SIGNAL_INFO` without the OS struct. Total size
/// equals active size (no invented blanking), so the driver stamps one region into both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SignalInfo {
    /// Pixels per second. u64 because 8K at 240 Hz alone is 7.96e9.
    pub pixel_rate: u64,
    /// `hSyncFreq` as `(numerator, denominator)`, saturated into u32.
    pub h_sync: (u32, u32),
    /// `vSyncFreq`: the refresh rate as a reduced fraction of hertz.
    pub v_sync: (u32, u32),
    /// `AdditionalSignalInfo` read as `videoStandard`: 255 (other), with the vSync
    /// frequency divider in bits 16..21.
    pub video_standard: u32,
}

const fn gcd(mut a: u32, mut b: u32) -> u32 {
    while b != 0 {
        let t = a % b;
        a = b;
        b = t;
    }
    a
}

/// `refresh_mhz` as a reduced fraction of hertz: 60000 is `60/1`, 59940 is `2997/50`.
#[must_use]
pub const fn refresh_rational(refresh_mhz: u32) -> (u32, u32) {
    let g = gcd(refresh_mhz, 1000);
    if g == 0 {
        return (0, 1);
    }
    (refresh_mhz / g, 1000 / g)
}

/// The signal description both mode DDI families report (IddSampleDriver's shape): pixel
/// rate = refresh · width · height and total == active. Monitor modes pass divider 0 and
/// target modes 1, the only difference between the two.
#[must_use]
pub fn signal_info(
    width: u32,
    height: u32,
    refresh_mhz: u32,
    v_sync_freq_divider: u32,
) -> SignalInfo {
    let (num, den) = refresh_rational(refresh_mhz);
    SignalInfo {
        pixel_rate: u64::from(refresh_mhz) * u64::from(width) * u64::from(height) / 1000,
        h_sync: (
            u32::try_from(u64::from(num) * u64::from(height)).unwrap_or(u32::MAX),
            den,
        ),
        v_sync: (num, den),
        video_standard: 255 | (v_sync_freq_divider << 16),
    }
}

/// Bounds for a mode the host asks for: generous, but no zero or absurd value reaches the
/// EDID and [`signal_info`] arithmetic.
#[must_use]
pub fn valid_mode(width: u32, height: u32, refresh_mhz: u32) -> bool {
    (1..=MAX_DIMENSION).contains(&width)
        && (1..=MAX_DIMENSION).contains(&height)
        && (MIN_REFRESH_MHZ..=MAX_REFRESH_MHZ).contains(&refresh_mhz)
}

/// Why no monitor id was chosen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdError {
    /// The exact id asked for is held by another process; retry once it departs.
    Busy,
    /// Every id is taken.
    Full,
}

/// Pick the id for a new monitor. `live` lists the ids on the device and whether the caller
/// owns each. A free `preferred` id in `1..=MAX_MONITOR_ID` wins, so a client keeps a stable
/// identity and Windows reapplies its saved per-monitor settings. A live holder is never
/// displaced: with `exact` and another process holding the id the answer is
/// [`IdError::Busy`], otherwise the lowest free id. Reusing freed ids keeps the connector
/// index bounded, so a new monitor reuses a departed one's OS target slot instead of
/// leaving a ghost behind.
pub fn choose_id(live: &[(u32, bool)], preferred: u32, exact: bool) -> Result<u32, IdError> {
    let taken = |id: u32| live.iter().find(|(l, _)| *l == id);
    if (1..=MAX_MONITOR_ID).contains(&preferred) {
        match taken(preferred) {
            None => return Ok(preferred),
            Some((_, false)) if exact => return Err(IdError::Busy),
            Some(_) => {}
        }
    }
    (1..=MAX_MONITOR_ID)
        .find(|&id| taken(id).is_none())
        .ok_or(IdError::Full)
}

/// The id `stable` (the host's per-client 64-bit display id) prefers: stable for a client,
/// spread over `1..=MAX_MONITOR_ID`.
#[must_use]
pub const fn preferred_id(stable: u64) -> u32 {
    1 + (stable % MAX_MONITOR_ID as u64) as u32
}

/// Base of the container GUIDs, `{d8031c89-ba11-4ec5-8104-9b390fa0xxxx}`.
const CONTAINER_BASE: u128 = 0xd803_1c89_ba11_4ec5_8104_9b39_0fa0_0000;

/// A deterministic container GUID per monitor id (it groups a monitor's targets into one
/// device), as `(Data1, Data2, Data3, Data4)`.
#[must_use]
pub const fn container_guid(id: u32) -> (u32, u16, u16, [u8; 8]) {
    let (d1, d2, d3, mut d4) = crate::guid_fields(CONTAINER_BASE);
    d4[6] = (id >> 8) as u8;
    d4[7] = id as u8;
    (d1.wrapping_add(id), d2, d3, d4)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mode(width: u32, height: u32, refresh_mhz: &[u32]) -> Mode {
        Mode {
            width,
            height,
            refresh_mhz: refresh_mhz.to_vec(),
        }
    }

    #[test]
    fn the_requested_mode_leads_then_fallbacks_then_history() {
        let asked = mode(2560, 1440, &[240_000]);
        let history = [mode(3840, 2160, &[60_000]), mode(1920, 1080, &[144_000])];
        let list = advertised_modes(asked.clone(), &history);
        assert_eq!(list[0], mode(2560, 1440, &[240_000, 60_000, 120_000]));
        assert_eq!(list[1], mode(1920, 1080, &[60_000, 120_000, 144_000]));
        assert_eq!(list[2], mode(1280, 720, &[60_000]));
        // History adds 4K and its 1080p rate without duplicating the resolution.
        assert_eq!(list[3], mode(3840, 2160, &[60_000]));
        assert_eq!(list.len(), 4);
        let flat: Vec<_> = flatten(&list[..2])
            .map(|i| (i.width, i.height, i.refresh_mhz))
            .collect();
        assert_eq!(
            flat,
            [
                (2560, 1440, 240_000),
                (2560, 1440, 60_000),
                (2560, 1440, 120_000),
                (1920, 1080, 60_000),
                (1920, 1080, 120_000),
                (1920, 1080, 144_000)
            ]
        );
    }

    #[test]
    fn a_4k_stream_offers_1440p_without_prior_mode_history() {
        for rate in [119_880, 120_000, 240_000] {
            let requested = mode(3840, 2160, &[rate]);
            let list = advertised_modes(requested.clone(), &[]);
            assert_eq!(list[0], requested);
            let available: Vec<_> = flatten(&list)
                .map(|m| (m.width, m.height, m.refresh_mhz))
                .collect();
            for fallback_rate in [60_000, 120_000, 240_000] {
                assert!(available.contains(&(2560, 1440, fallback_rate)));
            }
            assert!(available.contains(&(1920, 1080, 60_000)));
            assert!(available.contains(&(1280, 720, 60_000)));
        }
    }

    #[test]
    fn adding_1440p_fallbacks_preserves_a_remembered_custom_refresh() {
        let history = [mode(2560, 1440, &[165_000, 120_000])];
        let list = advertised_modes(mode(3840, 2160, &[120_000]), &history);
        assert_eq!(
            list[1],
            mode(2560, 1440, &[60_000, 120_000, 240_000, 165_000])
        );
        assert_eq!(list.iter().filter(|m| m.width == 2560).count(), 1);
    }

    #[test]
    fn a_requested_fallback_resolution_is_not_listed_twice() {
        let list = advertised_modes(mode(1920, 1080, &[480_000]), &[]);
        assert_eq!(list[0], mode(1920, 1080, &[480_000, 60_000, 120_000]));
        assert_eq!(list.iter().filter(|m| m.width == 1920).count(), 1);
    }

    #[test]
    fn the_union_stops_at_the_cap() {
        let mut full: Vec<_> = (0..MODE_LIST_CAP as u32)
            .map(|i| mode(640 + i, 480, &[60_000]))
            .collect();
        union_modes(&mut full, &[mode(3840, 2160, &[60_000])]);
        assert_eq!(full.len(), MODE_LIST_CAP);
        assert!(!full.contains(&mode(3840, 2160, &[60_000])));
        // Reaching the resolution cap must not discard a rate for an existing mode.
        union_modes(
            &mut full,
            &[mode(3840, 2160, &[60_000]), mode(640, 480, &[120_000])],
        );
        assert_eq!(full.len(), MODE_LIST_CAP);
        assert_eq!(full[0], mode(640, 480, &[60_000, 120_000]));
    }

    #[test]
    fn refresh_rates_become_reduced_fractions() {
        assert_eq!(refresh_rational(60_000), (60, 1));
        assert_eq!(refresh_rational(480_000), (480, 1));
        assert_eq!(refresh_rational(59_940), (2997, 50));
        assert_eq!(refresh_rational(119_880), (2997, 25));
        assert_eq!(refresh_rational(0), (0, 1));
    }

    #[test]
    fn signal_info_matches_the_ddi_formula() {
        let m = signal_info(1920, 1080, 60_000, 0);
        assert_eq!(m.pixel_rate, 60 * 1920 * 1080);
        assert_eq!(m.h_sync, (60 * 1080, 1));
        assert_eq!(m.v_sync, (60, 1));
        assert_eq!(m.video_standard, 255);
        assert_eq!(
            signal_info(1920, 1080, 60_000, 1).video_standard,
            255 | (1 << 16)
        );
        let ntsc = signal_info(3840, 2160, 59_940, 1);
        assert_eq!(ntsc.v_sync, (2997, 50));
        assert_eq!(ntsc.h_sync, (2997 * 2160, 50));
        assert_eq!(ntsc.pixel_rate, 3840 * 2160 * 59_940 / 1000);
        let fast = signal_info(7680, 4320, 480_000, 1);
        assert_eq!(fast.pixel_rate, 7680 * 4320 * 480);
        assert!(fast.pixel_rate > u64::from(u32::MAX));
        // Saturates instead of overflowing inside an extern "C" mode DDI.
        assert_eq!(signal_info(1, u32::MAX, 1_000_000, 0).h_sync.0, u32::MAX);
    }

    #[test]
    fn valid_mode_takes_the_edges_and_rejects_the_rest() {
        assert!(valid_mode(1, 1, 1_000));
        assert!(valid_mode(16384, 16384, 1_000_000));
        assert!(valid_mode(1920, 1080, 480_000));
        for (w, h, r) in [
            (0, 1080, 60_000),
            (1920, 0, 60_000),
            (16385, 1080, 60_000),
            (1920, 1080, 999),
            (1920, 1080, 1_000_001),
        ] {
            assert!(!valid_mode(w, h, r), "{w}x{h}@{r}");
        }
    }

    #[test]
    fn ids_honour_the_preference_without_displacing_anyone() {
        // Free preferred id wins.
        assert_eq!(choose_id(&[(1, true), (2, false)], 7, false), Ok(7));
        // Held by the caller: next free, never the holder's id.
        assert_eq!(choose_id(&[(1, true), (2, true)], 2, true), Ok(3));
        // Held by another process: auto without `exact`, busy with it.
        assert_eq!(choose_id(&[(1, true), (2, false)], 2, false), Ok(3));
        assert_eq!(
            choose_id(&[(1, true), (2, false)], 2, true),
            Err(IdError::Busy)
        );
        // 0 and out-of-range preferences take the lowest free id, refilling gaps.
        assert_eq!(choose_id(&[(1, true), (3, true)], 0, true), Ok(2));
        assert_eq!(choose_id(&[], 16, false), Ok(1));
        let all: Vec<(u32, bool)> = (1..=15).map(|i| (i, true)).collect();
        assert_eq!(choose_id(&all, 0, false), Err(IdError::Full));
    }

    #[test]
    fn preferred_ids_stay_in_the_connector_range() {
        for stable in [0, 1, 14, 15, 0x6000_0000_0000_0000, u64::MAX] {
            let id = preferred_id(stable);
            assert!((1..=MAX_MONITOR_ID).contains(&id), "{stable:#x} -> {id}");
        }
        assert_eq!(preferred_id(15), 1);
    }

    #[test]
    fn container_guids_are_unique_and_ours() {
        let all: Vec<_> = (1u32..=16).map(container_guid).collect();
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_ne!(a, b);
            }
        }
        assert_eq!(
            container_guid(1),
            (
                0xd803_1c8a,
                0xba11,
                0x4ec5,
                [0x81, 0x04, 0x9b, 0x39, 0x0f, 0xa0, 0x00, 0x01]
            )
        );
    }
}
