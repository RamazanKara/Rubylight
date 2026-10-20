//! The 256-byte EDID each virtual monitor hands IddCx: an EDID 1.4 base block plus a
//! CTA-861.3 extension with a BT.2020 Colorimetry block and an HDR Static Metadata block
//! declaring the PQ EOTF. Windows reads HDR capability from that CTA block; without it the
//! monitor stays SDR whatever the adapter's FP16 caps say.
//!
//! Adapted from punktfunk's `pf_driver_proto::edid` (Enrico Bühler (unom), MIT OR
//! Apache-2.0). Rubylight's identity: manufacturer `RBY`, product name `Rubylight`, the
//! per-monitor serial at base offset 0x0C (which [`get_serial`] reads back in the mode
//! callbacks). The IddCx mode callbacks declare the supported timings; the EDID does not
//! impose a smaller, fixed frequency range on those modes.

/// `2^(k/32)` for `k = 0..32` in Q16 fixed point: the fractional step table of the
/// CTA-861.3 luminance exponent.
const POW2_Q16: [u32; 32] = [
    65536, 66971, 68438, 69936, 71468, 73032, 74632, 76266, 77936, 79642, 81386, 83169, 84990,
    86851, 88752, 90696, 92682, 94711, 96785, 98905, 101070, 103283, 105545, 107856, 110218,
    112631, 115098, 117618, 120194, 122825, 125515, 128263,
];

/// A CTA-861.3 max or frame-average luminance code in millinits: `50 * 2^(CV/32)` cd/m².
#[must_use]
pub const fn cta_max_millinits(code: u8) -> u64 {
    let whole = code as u32 / 32;
    let frac = code as u32 % 32;
    ((50_000u64 << whole) * POW2_Q16[frac as usize] as u64) >> 16
}

/// The largest code whose luminance does not exceed `nits`, never brighter than the panel.
/// Clamped to `1..=255`: `0` means "no data" and callers gate on `nits > 0`.
#[must_use]
pub fn cta_max_luminance_code(nits: u32) -> u8 {
    let target = u64::from(nits) * 1000;
    let mut code = 1u8;
    while code < 255 && cta_max_millinits(code + 1) <= target {
        code += 1;
    }
    code
}

/// A display's min luminance (millinits) as the CTA-861.3 code, relative to the block's max:
/// `CV = 255 * sqrt(100 * L_min / L_max)`, rounded to nearest. `0` stays `0` (unknown).
#[must_use]
pub fn cta_min_luminance_code(millinits: u32, max_code: u8) -> u8 {
    let max_millinits = cta_max_millinits(max_code);
    if millinits == 0 || max_millinits == 0 {
        return 0;
    }
    let x = (100u64 * 255 * 255).saturating_mul(u64::from(millinits)) / max_millinits;
    let floor = x.isqrt();
    let cv = if (floor + 1) * (floor + 1) - x <= x - floor * floor {
        floor + 1
    } else {
        floor
    };
    cv.min(255) as u8
}

/// Reduced blanking for [`dtd`]: a virtual display has no scan-out, so the blanking only has
/// to be self-consistent with the pixel clock derived from it.
const DTD_H_BLANK: u32 = 80;
const DTD_V_BLANK: u32 = 45;
const DTD_H_SYNC_OFFSET: u32 = 8;
const DTD_H_SYNC_WIDTH: u32 = 32;
const DTD_V_SYNC_OFFSET: u32 = 3;
const DTD_V_SYNC_WIDTH: u32 = 5;

/// The 18-byte detailed timing descriptor for `width`x`height` at `refresh_mhz`. `None` when
/// the mode does not fit the encoding: a pixel clock above 655.35 MHz (4K120 class) or a
/// dimension above 12 bits.
#[must_use]
pub fn dtd(width: u32, height: u32, refresh_mhz: u32) -> Option<[u8; 18]> {
    if width == 0 || height == 0 || refresh_mhz == 0 || width > 4095 || height > 4095 {
        return None;
    }
    let h_total = u64::from(width + DTD_H_BLANK);
    let v_total = u64::from(height + DTD_V_BLANK);
    let clock_10khz = h_total * v_total * u64::from(refresh_mhz) / 10_000_000;
    let clock_10khz = u16::try_from(clock_10khz).ok()?;
    let mut d = [0u8; 18];
    d[0..2].copy_from_slice(&clock_10khz.to_le_bytes());
    d[2] = (width & 0xFF) as u8;
    d[3] = (DTD_H_BLANK & 0xFF) as u8;
    d[4] = ((((width >> 8) & 0x0F) << 4) | ((DTD_H_BLANK >> 8) & 0x0F)) as u8;
    d[5] = (height & 0xFF) as u8;
    d[6] = (DTD_V_BLANK & 0xFF) as u8;
    d[7] = ((((height >> 8) & 0x0F) << 4) | ((DTD_V_BLANK >> 8) & 0x0F)) as u8;
    d[8] = (DTD_H_SYNC_OFFSET & 0xFF) as u8;
    d[9] = (DTD_H_SYNC_WIDTH & 0xFF) as u8;
    d[10] = (((DTD_V_SYNC_OFFSET & 0x0F) << 4) | (DTD_V_SYNC_WIDTH & 0x0F)) as u8;
    d[11] = ((((DTD_H_SYNC_OFFSET >> 8) & 0x03) << 6)
        | (((DTD_H_SYNC_WIDTH >> 8) & 0x03) << 4)
        | (((DTD_V_SYNC_OFFSET >> 4) & 0x03) << 2)
        | ((DTD_V_SYNC_WIDTH >> 4) & 0x03)) as u8;
    // Bytes 12..17 (image size, borders) stay 0 = undefined. Digital separate sync, +H/+V.
    d[17] = 0x1E;
    Some(d)
}

/// Per-monitor serial: base offset 0x0C, little-endian.
const SERIAL_OFFSET: usize = 0x0C;

/// EDID 1.4 base block. The checksum, the serial, the panel size and the preferred timing
/// are patched in [`generate`]. This is a discrete-frequency display: IddCx supplies its
/// mode list. A base-block range descriptor cannot represent our 1000 Hz modes (its
/// vertical and horizontal fields top out at 510 Hz and 510 kHz), so do not advertise
/// continuous frequency or a contradictory range. The dummy descriptor is EDID tag 0x10.
#[rustfmt::skip]
const BASE: [u8; 128] = [
    0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00, // fixed header
    0x48, 0x59, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, // manufacturer "RBY", product 1, serial (patched)
    0xFF, 0x21, 0x01, 0x04, 0xB0, 0x32, 0x1F, 0x78, // model year, EDID 1.4, 10-bit digital, size, gamma 2.2
    0x02, 0x78, 0xB1, 0xB5, 0x4A, 0x2B, 0xCC, 0x21, // features (preferred timing, discrete frequency), BT.2020 primaries...
    0x0B, 0x50, 0x54, 0x00, 0x00, 0x00, 0x01, 0x01, // ...primaries, no established timings, standard timings unused
    0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01,
    0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x02, 0x3A, // DTD 1: 1080p60 placeholder preferred timing
    0x80, 0x18, 0x71, 0x38, 0x2D, 0x40, 0x58, 0x2C,
    0x45, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x1E,
    0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x00, // dummy descriptor: mode limits come from IddCx
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0xFC, 0x00, 0x52, // name descriptor "Rubylight"
    0x75, 0x62, 0x79, 0x6C, 0x69, 0x67, 0x68, 0x74,
    0x0A, 0x20, 0x20, 0x20, 0x00, 0x00, 0x00, 0x00, // fourth descriptor unused...
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, // ...one extension block, checksum (patched)
];

/// CTA-861.3 extension header: revision 3, data blocks in bytes 4..15, no native DTDs, RGB
/// only (the IddCx wire format is RGB).
const CTA_HEADER: [u8; 4] = [0x02, 0x03, 0x0F, 0x00];

/// Colorimetry Data Block: BT.2020 RGB.
const COLORIMETRY_DB: [u8; 4] = [0xE3, 0x05, 0x80, 0x00];

/// HDR Static Metadata Data Block: traditional SDR and PQ EOTFs, Static Metadata Type 1, and
/// built-in desired-content luminance (~993 / 400 / 0.05 nits) that [`generate`] replaces
/// with the client display's volume when the host knows it.
const HDR_STATIC_METADATA_DB: [u8; 7] = [0xE6, 0x06, 0x05, 0x01, 0x8A, 0x60, 0x12];

/// The client display's luminance volume, in [`crate::control::AddRequest`]'s units.
/// `max_nits == 0` keeps the built-in defaults.
#[derive(Debug, Clone, Copy, Default)]
pub struct ClientLuminance {
    pub max_nits: u32,
    pub max_frame_avg_nits: u32,
    pub min_millinits: u32,
}

/// The panel size (cm) that puts `px` near 96 DPI, so Windows' scaling stays the client's
/// choice instead of following the mode.
fn size_cm(px: u32) -> u8 {
    (u64::from(px) * 254 / 9600).clamp(1, 255) as u8
}

const H_SIZE_CM_OFFSET: usize = 0x15;
const V_SIZE_CM_OFFSET: usize = 0x16;

/// The EDID of monitor `serial`, both block checksums recomputed. `preferred` is the
/// session's `(width, height, refresh_mhz)`: it becomes the preferred timing when it fits
/// the descriptor and sets the panel size. The modes Windows offers still come from the
/// driver's mode list.
#[must_use]
pub fn generate(
    serial: u32,
    lum: ClientLuminance,
    preferred: Option<(u32, u32, u32)>,
) -> [u8; 256] {
    let mut edid = [0u8; 256];
    edid[..128].copy_from_slice(&BASE);
    edid[SERIAL_OFFSET..SERIAL_OFFSET + 4].copy_from_slice(&serial.to_le_bytes());
    // A rate too fast for the descriptor keeps the resolution at 60 Hz, which the mode
    // list always offers, so Windows' default is not the 1080p placeholder.
    if let Some(d) = preferred.and_then(|(w, h, r)| dtd(w, h, r).or_else(|| dtd(w, h, 60_000))) {
        edid[54..72].copy_from_slice(&d);
    }
    if let Some((w, h, _)) = preferred {
        edid[H_SIZE_CM_OFFSET] = size_cm(w);
        edid[V_SIZE_CM_OFFSET] = size_cm(h);
    }
    edid[128..132].copy_from_slice(&CTA_HEADER);
    edid[132..136].copy_from_slice(&COLORIMETRY_DB);
    let mut hdr_db = HDR_STATIC_METADATA_DB;
    if lum.max_nits > 0 {
        let max_code = cta_max_luminance_code(lum.max_nits);
        hdr_db[4] = max_code;
        hdr_db[5] = if lum.max_frame_avg_nits > 0 {
            cta_max_luminance_code(lum.max_frame_avg_nits)
        } else {
            0
        };
        hdr_db[6] = cta_min_luminance_code(lum.min_millinits, max_code);
    }
    edid[136..143].copy_from_slice(&hdr_db);
    fix_block_checksum(&mut edid, 0);
    fix_block_checksum(&mut edid, 128);
    edid
}

/// The serial of an EDID the OS hands back (full or base block only); an error, never a
/// panic, for a short buffer.
pub fn get_serial(edid: &[u8]) -> Result<u32, core::array::TryFromSliceError> {
    let bytes: [u8; 4] = edid
        .get(SERIAL_OFFSET..SERIAL_OFFSET + 4)
        .unwrap_or(&[])
        .try_into()?;
    Ok(u32::from_le_bytes(bytes))
}

fn fix_block_checksum(edid: &mut [u8], start: usize) {
    let sum = edid[start..start + 127]
        .iter()
        .fold(0u8, |acc, &b| acc.wrapping_add(b));
    edid[start + 127] = 0u8.wrapping_sub(sum);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block_sum(block: &[u8]) -> u8 {
        block.iter().fold(0u8, |acc, &b| acc.wrapping_add(b))
    }

    /// The three letters packed into bytes 8 and 9.
    fn manufacturer(edid: &[u8]) -> [u8; 3] {
        let v = u16::from_be_bytes([edid[8], edid[9]]);
        [
            b'A' - 1 + ((v >> 10) & 0x1F) as u8,
            b'A' - 1 + ((v >> 5) & 0x1F) as u8,
            b'A' - 1 + (v & 0x1F) as u8,
        ]
    }

    #[test]
    fn the_identity_is_rubylights() {
        let e = generate(3, ClientLuminance::default(), None);
        assert_eq!(&manufacturer(&e), b"RBY");
        assert_eq!(e[0x5D], 0xFC, "name descriptor");
        assert_eq!(&e[0x5F..0x69], b"Rubylight\n");
        assert_eq!(e[0x7E], 1, "one extension");
    }

    #[test]
    fn the_edid_does_not_constrain_the_drivers_discrete_high_rate_modes() {
        for requested in [
            (1920, 1080, 480_000),
            (3840, 2160, 1_000_000),
            (7680, 4320, 240_000),
        ] {
            let e = generate(1, ClientLuminance::default(), Some(requested));
            assert_eq!(
                e[0x18] & 1,
                0,
                "discrete frequency does not require a range descriptor"
            );
            for descriptor in e[54..126].as_chunks::<18>().0 {
                assert!(
                    descriptor[0..2] != [0, 0] || descriptor[3] != 0xFD,
                    "a fixed range must not filter out {requested:?}"
                );
            }
            assert_eq!(e[0x4B], 0x10, "valid dummy descriptor");
            assert!(e[0x4C..0x5A].iter().all(|b| *b == 0));
            assert_eq!(block_sum(&e[..128]), 0);
            assert_eq!(block_sum(&e[128..]), 0);
            assert_eq!(
                &e[136..143],
                &generate(1, ClientLuminance::default(), None)[136..143],
                "HDR metadata is preserved"
            );
        }
    }

    #[test]
    fn dtd_encodes_the_session_mode() {
        let d = dtd(1920, 1080, 60_000).expect("1080p60 fits");
        assert_eq!(u16::from_le_bytes([d[0], d[1]]), 13_500);
        assert_eq!(u32::from(d[2]) | (u32::from(d[4] >> 4) << 8), 1920);
        assert_eq!(u32::from(d[5]) | (u32::from(d[7] >> 4) << 8), 1080);
        assert_eq!(d[17], 0x1E);
        // 59.94 Hz keeps its fraction in the clock: 2000 x 1125 x 59.94 = 134.865 MHz.
        let ntsc = dtd(1920, 1080, 59_940).unwrap();
        assert_eq!(u16::from_le_bytes([ntsc[0], ntsc[1]]), 13_486);
        assert_eq!(dtd(3840, 2160, 120_000), None);
        assert!(dtd(3840, 2160, 60_000).is_some());
        assert_eq!(dtd(0, 1080, 60_000), None);
        assert_eq!(dtd(5000, 1080, 10_000), None);
    }

    #[test]
    fn blocks_checksum_to_zero_and_the_serial_reads_back() {
        let lum = ClientLuminance {
            max_nits: 1000,
            max_frame_avg_nits: 400,
            min_millinits: 50,
        };
        for (serial, l, mode) in [
            (0, ClientLuminance::default(), None),
            (1, lum, Some((1920, 1080, 60_000))),
            (u32::MAX, lum, Some((3840, 2160, 240_000))),
        ] {
            let e = generate(serial, l, mode);
            assert_eq!(block_sum(&e[..128]), 0, "base block, serial {serial}");
            assert_eq!(block_sum(&e[128..]), 0, "CTA block, serial {serial}");
            assert_eq!(get_serial(&e).unwrap(), serial);
            assert_eq!(get_serial(&e[..128]).unwrap(), serial);
        }
        assert!(get_serial(&[0u8; 8]).is_err());
    }

    #[test]
    fn the_preferred_timing_and_panel_size_follow_the_mode() {
        let lum = ClientLuminance::default();
        let placeholder = generate(1, lum, None);
        assert_eq!(
            u16::from_le_bytes([placeholder[54], placeholder[55]]),
            14_850
        );
        let swapped = generate(1, lum, Some((2560, 1440, 120_000)));
        assert_eq!(swapped[54..72], dtd(2560, 1440, 120_000).unwrap());
        assert_eq!((swapped[0x15], swapped[0x16]), (67, 38));
        // Too fast for the descriptor: the resolution stays preferred at 60 Hz (#40).
        for rate in [120_000, 1_000_000] {
            let too_fast = generate(1, lum, Some((3840, 2160, rate)));
            assert_eq!(too_fast[54..72], dtd(3840, 2160, 60_000).unwrap());
        }
        let too_large = generate(1, lum, Some((7680, 4320, 60_000)));
        assert_eq!(too_large[54..72], placeholder[54..72]);
    }

    #[test]
    fn the_hdr_block_tracks_the_client_volume() {
        let stock = generate(1, ClientLuminance::default(), None);
        assert_eq!(stock[136..143], [0xE6, 0x06, 0x05, 0x01, 0x8A, 0x60, 0x12]);
        let coded = generate(
            1,
            ClientLuminance {
                max_nits: 400,
                max_frame_avg_nits: 400,
                min_millinits: 50,
            },
            None,
        );
        assert_eq!(coded[136..140], [0xE6, 0x06, 0x05, 0x01]);
        assert_eq!(coded[140], cta_max_luminance_code(400));
        let partial = generate(
            1,
            ClientLuminance {
                max_nits: 400,
                ..Default::default()
            },
            None,
        );
        assert_eq!((partial[141], partial[142]), (0, 0));
    }

    #[test]
    fn luminance_codes_hit_the_reference_points() {
        assert_eq!(cta_max_millinits(0x60), 400_000);
        assert_eq!(cta_max_millinits(0x8A) / 1000, 993);
        assert_eq!(cta_max_luminance_code(400), 0x60);
        assert_eq!(cta_max_luminance_code(1000), 138);
        assert_eq!(cta_min_luminance_code(50, 0x8A), 0x12);
        assert_eq!(cta_max_luminance_code(0), 1);
        assert_eq!(cta_max_luminance_code(u32::MAX), 255);
        assert_eq!(cta_min_luminance_code(0, 0x8A), 0);
        assert_eq!(cta_min_luminance_code(u32::MAX, 1), 255);
    }
}
