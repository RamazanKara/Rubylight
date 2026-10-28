//! What a client offers beyond 8-bit 4:2:0: native YUV 4:4:4 and 10-bit SDR. Both clients decide
//! it the same way: from the settings, the formats their decoder decodes and they can draw, and
//! the PC's codec support. Mirrors Android's `Sdr10Policy` and its 4:4:4 offer; no new wire
//! formats, only the existing format bits.
//!
//! 10-bit SDR has no protocol flag of its own: the client offers Main10 formats without asking
//! for HDR in the launch, and the PC encodes 10-bit BT.709. 4:4:4 is the High 4:4:4 (H.264),
//! RExt (HEVC) and High (AV1) 8-bit formats. HDR takes priority over both.

use crate::sdp::*;

/// The 8-bit 4:4:4 formats "Prefer YUV 4:4:4" can add.
pub const YUV444_FORMATS: u32 =
    VIDEO_FORMAT_H264_HIGH8_444 | VIDEO_FORMAT_H265_REXT8_444 | VIDEO_FORMAT_AV1_HIGH8_444;
/// The 10-bit 4:2:0 formats 10-bit SDR can add.
pub const SDR10_FORMATS: u32 = VIDEO_FORMAT_H265_MAIN10 | VIDEO_FORMAT_AV1_MAIN10;

/// The settings that decide the offer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Wants {
    /// "Prefer YUV 4:4:4".
    pub yuv444: bool,
    /// "10-bit color (SDR)".
    pub sdr10: bool,
    /// HDR is on for this stream.
    pub hdr: bool,
    /// The experimental packed full-chroma mode is on.
    pub packed444: bool,
    /// PyroWave is the chosen codec.
    pub pyrowave: bool,
}

/// Why 4:4:4 or 10-bit SDR is not offered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The setting is off.
    Off,
    /// HDR is on: HDR takes priority over 4:4:4, and a 10-bit HDR stream is not SDR.
    Hdr,
    /// Packed full chroma is on: it has its own 4:4:4 and 10-bit handling.
    Packed444,
    /// PyroWave is chosen: its 4:4:4 is the PyroWave 4:4:4 codec, its 10-bit output HDR only.
    PyroWave,
    /// This device decodes or draws none of the formats, with the decoder check's reason.
    Decoder(String),
    /// The PC encodes none of them for the codec that will be streamed.
    Host,
    /// 10-bit SDR only: 4:4:4 is on and the PC takes it, and the PC streams 4:4:4 or 10-bit,
    /// not both.
    Yuv444,
}

impl Refusal {
    /// Whether the player is told: a setting that is off, or HDR (which is the 10-bit stream
    /// they asked for), needs no word.
    pub fn worth_saying(&self) -> bool {
        !matches!(self, Refusal::Off | Refusal::Hdr)
    }
}

/// The formats to offer and what became of each setting.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Offer {
    /// `base` with the 4:4:4 and 10-bit SDR formats that are offered.
    pub formats: u32,
    pub yuv444: Result<(), Refusal>,
    pub sdr10: Result<(), Refusal>,
}

/// The codec family the PC will stream from `formats`, by `DescribeInfo::select_video_format`'s
/// order: AV1, then HEVC, then H.264.
fn streamed_family(formats: u32, host_scm: u32) -> u32 {
    if formats & VIDEO_FORMAT_MASK_AV1 != 0 && host_scm & SCM_AV1_MAIN8 != 0 {
        VIDEO_FORMAT_MASK_AV1
    } else if formats & VIDEO_FORMAT_MASK_H265 != 0 && host_scm & SCM_HEVC != 0 {
        VIDEO_FORMAT_MASK_H265
    } else {
        VIDEO_FORMAT_MASK_H264
    }
}

/// The formats in `formats` the PC encodes. A PC that reports no codec support (`0`) encodes
/// none of the 4:4:4 or 10-bit ones.
fn encoded(formats: u32, host_scm: u32) -> u32 {
    [
        (VIDEO_FORMAT_H264_HIGH8_444, SCM_H264_HIGH8_444),
        (VIDEO_FORMAT_H265_REXT8_444, SCM_HEVC_REXT8_444),
        (VIDEO_FORMAT_AV1_HIGH8_444, SCM_AV1_HIGH8_444),
        (VIDEO_FORMAT_H265_MAIN10, SCM_HEVC_MAIN10),
        (VIDEO_FORMAT_AV1_MAIN10, SCM_AV1_MAIN10),
    ]
    .into_iter()
    .filter(|&(format, scm)| formats & format != 0 && host_scm & scm != 0)
    .fold(0, |all, (format, _)| all | format)
}

/// The 4:4:4 and 10-bit formats of the codec families in `base`.
fn of_families(base: u32, formats: u32) -> u32 {
    let mut families = VIDEO_FORMAT_MASK_H264;
    if base & VIDEO_FORMAT_H265 != 0 {
        families |= VIDEO_FORMAT_MASK_H265;
    }
    if base & VIDEO_FORMAT_AV1_MAIN8 != 0 {
        families |= VIDEO_FORMAT_MASK_AV1;
    }
    formats & families
}

/// The offer for `wants`. `base` is what the codec choice offers without these settings;
/// `decodable` the formats of [`YUV444_FORMATS`] and [`SDR10_FORMATS`] this device decodes and
/// draws, and `decoder_reason` why the others are not; `host_scm` the PC's codec support.
pub fn offer(base: u32, wants: Wants, decodable: u32, decoder_reason: &str, host_scm: u32) -> Offer {
    let family = streamed_family(base, host_scm);
    let decoder = || Refusal::Decoder(decoder_reason.to_owned());
    let yuv444 = (|| {
        if !wants.yuv444 {
            return Err(Refusal::Off);
        }
        if wants.hdr {
            return Err(Refusal::Hdr);
        }
        if wants.packed444 {
            return Err(Refusal::Packed444);
        }
        if wants.pyrowave {
            return Err(Refusal::PyroWave);
        }
        let wanted = of_families(base, YUV444_FORMATS);
        if wanted & decodable == 0 {
            return Err(decoder());
        }
        // Only the streamed family's 4:4:4 format can be picked.
        let usable = encoded(wanted & decodable, host_scm) & family;
        if usable == 0 { Err(Refusal::Host) } else { Ok(usable) }
    })();
    let sdr10 = (|| {
        if !wants.sdr10 {
            return Err(Refusal::Off);
        }
        if wants.hdr {
            return Err(Refusal::Hdr);
        }
        if wants.pyrowave {
            return Err(Refusal::PyroWave);
        }
        if wants.packed444 {
            return Err(Refusal::Packed444);
        }
        let wanted = of_families(base, SDR10_FORMATS);
        if wanted & decodable == 0 {
            return Err(decoder());
        }
        let usable = encoded(wanted & decodable, host_scm) & family;
        if usable == 0 {
            return Err(Refusal::Host);
        }
        // The pick order ranks Main10 above 8-bit 4:4:4: leaving 10-bit out keeps 4:4:4.
        if yuv444.is_ok() { Err(Refusal::Yuv444) } else { Ok(usable) }
    })();
    Offer {
        formats: base | yuv444.as_ref().map_or(0, |f| *f) | sdr10.as_ref().map_or(0, |f| *f),
        yuv444: yuv444.map(|_| ()),
        sdr10: sdr10.map(|_| ()),
    }
}

/// What to tell the player once the stream's format is known.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Notice {
    /// 4:4:4 was asked for and the stream is 4:2:0.
    Yuv444Unavailable(Refusal),
    /// 10-bit SDR was asked for and is not offered.
    Sdr10Unavailable(Refusal),
    /// 10-bit SDR was offered and the PC streams 8-bit.
    Sdr10NotAccepted,
}

/// The notices for a stream that `offer` was made for and that streams `negotiated`.
pub fn notices(offer: &Offer, negotiated: u32) -> Vec<Notice> {
    let mut notices = Vec::new();
    match &offer.yuv444 {
        Err(refusal) if refusal.worth_saying() => notices.push(Notice::Yuv444Unavailable(refusal.clone())),
        // The PC took another codec or profile than the 4:4:4 one offered.
        Ok(()) if negotiated & VIDEO_FORMAT_MASK_YUV444 == 0 => notices.push(Notice::Yuv444Unavailable(Refusal::Host)),
        _ => {}
    }
    match &offer.sdr10 {
        Err(refusal) if refusal.worth_saying() => notices.push(Notice::Sdr10Unavailable(refusal.clone())),
        Ok(()) if negotiated & VIDEO_FORMAT_MASK_10BIT == 0 => notices.push(Notice::Sdr10NotAccepted),
        _ => {}
    }
    notices
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEVC: u32 = VIDEO_FORMAT_H264 | VIDEO_FORMAT_H265;
    const AV1: u32 = VIDEO_FORMAT_H264 | VIDEO_FORMAT_AV1_MAIN8;
    const ALL_SCM: u32 = SCM_H264 | SCM_HEVC | SCM_AV1_MAIN8 | SCM_HEVC_MAIN10 | SCM_AV1_MAIN10
        | SCM_H264_HIGH8_444 | SCM_HEVC_REXT8_444 | SCM_AV1_HIGH8_444;
    const DECODES_ALL: u32 = YUV444_FORMATS | SDR10_FORMATS;

    fn wants(yuv444: bool, sdr10: bool) -> Wants {
        Wants { yuv444, sdr10, ..Default::default() }
    }

    /// A PC that streams HEVC and AV1.
    fn describe() -> crate::rtsp::DescribeInfo {
        crate::rtsp::DescribeInfo::parse("sprop-parameter-sets=AAAAAU\r\na=rtpmap:98 AV1/90000\r\n")
    }

    #[test]
    fn nothing_is_added_with_both_settings_off() {
        let offer = offer(HEVC, Wants::default(), DECODES_ALL, "", ALL_SCM);
        assert_eq!(offer, Offer { formats: HEVC, yuv444: Err(Refusal::Off), sdr10: Err(Refusal::Off) });
        assert!(notices(&offer, VIDEO_FORMAT_H265).is_empty());
    }

    #[test]
    fn yuv444_offers_the_streamed_familys_444_format_and_the_pc_picks_it() {
        let offer = offer(HEVC, wants(true, false), DECODES_ALL, "", ALL_SCM);
        assert_eq!(offer.formats, HEVC | VIDEO_FORMAT_H265_REXT8_444);
        assert_eq!(offer.yuv444, Ok(()));
        let picked = describe().select_video_format(offer.formats, ALL_SCM);
        assert_eq!(picked, VIDEO_FORMAT_H265_REXT8_444);
        assert!(notices(&offer, picked).is_empty());
        let offer = super::offer(VIDEO_FORMAT_H264, wants(true, false), DECODES_ALL, "", ALL_SCM);
        assert_eq!(offer.formats, VIDEO_FORMAT_H264 | VIDEO_FORMAT_H264_HIGH8_444);
        let offer = super::offer(AV1, wants(true, false), DECODES_ALL, "", ALL_SCM);
        assert_eq!(offer.formats, AV1 | VIDEO_FORMAT_AV1_HIGH8_444);
    }

    #[test]
    fn sdr10_offers_main10_and_the_pc_picks_it_without_hdr() {
        let offer = offer(HEVC, wants(false, true), DECODES_ALL, "", ALL_SCM);
        assert_eq!(offer.formats, HEVC | VIDEO_FORMAT_H265_MAIN10);
        let picked = describe().select_video_format(offer.formats, ALL_SCM);
        assert_eq!(picked, VIDEO_FORMAT_H265_MAIN10);
        assert!(notices(&offer, picked).is_empty());
        let offer = super::offer(AV1, wants(false, true), DECODES_ALL, "", ALL_SCM);
        assert_eq!(offer.formats, AV1 | VIDEO_FORMAT_AV1_MAIN10);
        let offer = super::offer(VIDEO_FORMAT_H264, wants(false, true), DECODES_ALL, "", ALL_SCM);
        assert_eq!(offer.sdr10, Err(Refusal::Decoder(String::new())), "H.264 has no Main10");
    }

    #[test]
    fn yuv444_wins_over_sdr10_when_the_pc_takes_it_and_says_so() {
        let offer = offer(HEVC, wants(true, true), DECODES_ALL, "", ALL_SCM);
        assert_eq!(offer.formats, HEVC | VIDEO_FORMAT_H265_REXT8_444, "Main10 would outrank RExt 4:4:4");
        assert_eq!(offer.sdr10, Err(Refusal::Yuv444));
        let picked = describe().select_video_format(offer.formats, ALL_SCM);
        assert_eq!(picked, VIDEO_FORMAT_H265_REXT8_444);
        assert_eq!(notices(&offer, picked), [Notice::Sdr10Unavailable(Refusal::Yuv444)]);
        // A PC without HEVC 4:4:4 keeps 10-bit SDR.
        let offer = super::offer(HEVC, wants(true, true), DECODES_ALL, "", ALL_SCM & !SCM_HEVC_REXT8_444);
        assert_eq!(offer.formats, HEVC | VIDEO_FORMAT_H265_MAIN10);
        assert_eq!((offer.yuv444.clone(), offer.sdr10.clone()), (Err(Refusal::Host), Ok(())));
        assert_eq!(notices(&offer, VIDEO_FORMAT_H265_MAIN10), [Notice::Yuv444Unavailable(Refusal::Host)]);
    }

    #[test]
    fn the_decoder_gate_offers_only_what_decodes_and_names_why() {
        let offer = offer(HEVC, wants(true, true), VIDEO_FORMAT_H265_MAIN10, "no HEVC 4:4:4 on the GPU decoder", ALL_SCM);
        assert_eq!(offer.formats, HEVC | VIDEO_FORMAT_H265_MAIN10);
        let reason = Refusal::Decoder("no HEVC 4:4:4 on the GPU decoder".into());
        assert_eq!(offer.yuv444, Err(reason.clone()));
        assert_eq!(notices(&offer, VIDEO_FORMAT_H265_MAIN10), [Notice::Yuv444Unavailable(reason)]);
        let offer = super::offer(HEVC, wants(false, true), 0, "no Main10 decoder", ALL_SCM);
        assert_eq!(offer.formats, HEVC);
        assert_eq!(notices(&offer, VIDEO_FORMAT_H265), [Notice::Sdr10Unavailable(Refusal::Decoder("no Main10 decoder".into()))]);
    }

    #[test]
    fn hdr_packed_and_pyrowave_take_the_stream_first() {
        let hdr = Wants { hdr: true, ..wants(true, true) };
        let offer = offer(HEVC | VIDEO_FORMAT_H265_MAIN10, hdr, DECODES_ALL, "", ALL_SCM);
        assert_eq!(offer.formats, HEVC | VIDEO_FORMAT_H265_MAIN10);
        assert_eq!((offer.yuv444.clone(), offer.sdr10.clone()), (Err(Refusal::Hdr), Err(Refusal::Hdr)));
        assert!(notices(&offer, VIDEO_FORMAT_H265_MAIN10).is_empty(), "HDR is what was asked for");
        let packed = super::offer(HEVC, Wants { packed444: true, ..wants(true, true) }, DECODES_ALL, "", ALL_SCM);
        assert_eq!((packed.formats, packed.yuv444, packed.sdr10), (HEVC, Err(Refusal::Packed444), Err(Refusal::Packed444)));
        let pyrowave = super::offer(HEVC, Wants { pyrowave: true, ..wants(true, true) }, DECODES_ALL, "", ALL_SCM);
        assert_eq!((pyrowave.formats, pyrowave.yuv444, pyrowave.sdr10), (HEVC, Err(Refusal::PyroWave), Err(Refusal::PyroWave)));
    }

    #[test]
    fn a_pc_that_encodes_none_of_them_is_named_and_an_8_bit_answer_is_told() {
        let offer = offer(HEVC, wants(true, true), DECODES_ALL, "", SCM_H264 | SCM_HEVC);
        assert_eq!(offer.formats, HEVC);
        assert_eq!(notices(&offer, VIDEO_FORMAT_H265), [Notice::Yuv444Unavailable(Refusal::Host), Notice::Sdr10Unavailable(Refusal::Host)]);
        // A PC that reports no codec support streams neither.
        assert_eq!(super::offer(HEVC, wants(true, true), DECODES_ALL, "", 0).formats, HEVC);
        // Offered, but the stream came back 8-bit.
        let offer = super::offer(HEVC, wants(false, true), DECODES_ALL, "", ALL_SCM);
        assert_eq!(notices(&offer, VIDEO_FORMAT_H265), [Notice::Sdr10NotAccepted]);
        // The streamed family's formats only: HEVC 4:4:4 does not help an AV1 stream.
        let both = VIDEO_FORMAT_H264 | VIDEO_FORMAT_H265 | VIDEO_FORMAT_AV1_MAIN8;
        let offer = super::offer(both, wants(true, false), VIDEO_FORMAT_H265_REXT8_444, "", ALL_SCM);
        assert_eq!((offer.formats, offer.yuv444), (both, Err(Refusal::Host)));
    }
}
