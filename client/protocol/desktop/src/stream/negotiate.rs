//! What a launch asks the PC for: the stream mode, bitrate, codecs, color and HDR formats,
//! and what to tell the player when the PC streams something else.

use super::*;

pub fn codec_name(format: u32) -> &'static str {
    if format & VIDEO_FORMAT_MASK_PYROWAVE != 0 {
        if format & VIDEO_FORMAT_MASK_10BIT != 0 { "PyroWave 10-bit" } else { "PyroWave" }
    } else if format & VIDEO_FORMAT_MASK_AV1 != 0 {
        if format & VIDEO_FORMAT_MASK_10BIT != 0 {
            "AV1 10-bit"
        } else {
            "AV1"
        }
    } else if format & VIDEO_FORMAT_MASK_H265 != 0 {
        if format & VIDEO_FORMAT_MASK_10BIT != 0 {
            "HEVC Main10"
        } else {
            "HEVC"
        }
    } else {
        "H.264"
    }
}

/// The stream's resolution, rate and bitrate from the settings and display. The bitrate is the one
/// for the chosen codec (`start_bitrate`), PyroWave's own while PyroWave is chosen, as the settings
/// show it; a launch decides by what it offers the PC (`offers_pyrowave`).
pub fn stream_mode(settings: &Settings, display: Display) -> (u32, u32, u32, u32) {
    let (mut w, mut h) = if settings.width == 0 || settings.height == 0 {
        (display.width, display.height)
    } else {
        (settings.width, settings.height)
    };
    w = (w.max(320) + 1) & !1;
    h = (h.max(240) + 1) & !1;
    let fps = if settings.fps == 0 {
        (display.refresh_hz.round() as u32).clamp(30, 240)
    } else {
        settings.fps
    };
    (w, h, fps, start_bitrate(settings, w, h, fps, settings.codec.is_pyrowave(), 0))
}

/// The bitrate a stream starts at: the chosen one, or the default for its size and frame rate. A
/// PyroWave stream (intra-only, it breaks up at the conventional default) starts at PyroWave's own
/// default instead, within `link_mbps` (the wired link toward the PC, 0 when unknown as in the
/// settings), or at the chosen one raised to PyroWave's floor (`nightjar_policy::pyrowave`).
pub(super) fn start_bitrate(settings: &Settings, w: u32, h: u32, fps: u32, pyrowave: bool, link_mbps: u32) -> u32 {
    if pyrowave {
        nightjar_policy::pyrowave::start_kbps(settings.bitrate_kbps, settings.bitrate_kbps == 0, w, h, fps, link_mbps)
    } else if settings.bitrate_kbps == 0 {
        config::default_bitrate_kbps(w, h, fps)
    } else {
        settings.bitrate_kbps
    }
}

/// Whether a launch that offers `offered` streams PyroWave: the handshake picks it first when the
/// PC (`scm`, the trusted codec support) encodes a PyroWave format offered
/// (`Describe::select_video_format`). Whatever the codec choice, so a PyroWave choice the PC
/// lacks starts at the conventional bitrate.
pub(super) fn offers_pyrowave(offered: u32, scm: u32) -> bool {
    [(SCM_PYROWAVE, VIDEO_FORMAT_PYROWAVE), (SCM_PYROWAVE_444, VIDEO_FORMAT_PYROWAVE_444),
        (SCM_PYROWAVE_MAIN10, VIDEO_FORMAT_PYROWAVE_MAIN10), (SCM_PYROWAVE_MAIN10_444, VIDEO_FORMAT_PYROWAVE_MAIN10_444)]
        .iter()
        .any(|&(host, client)| scm & host != 0 && offered & client != 0)
}

/// The bitrate setting's warning when PyroWave is chosen and the chosen bitrate is below its
/// minimum for the stream's `w`x`h` at `fps`, naming the minimum. The bitrate is kept as chosen;
/// the automatic one never goes below the minimum.
pub fn pyrowave_bitrate_warning(settings: &Settings, w: u32, h: u32, fps: u32) -> Option<String> {
    let minimum = nightjar_policy::pyrowave::minimum_kbps(w, h, fps);
    (settings.codec.is_pyrowave() && settings.bitrate_kbps != 0 && settings.bitrate_kbps < minimum).then(|| {
        trf("Below PyroWave’s minimum of {} Mbps at {}x{}, {} fps: the picture will break up.", &[&minimum.div_ceil(1000), &w, &h, &fps])
    })
}

/// With the link test switched on (`link_probe`, off by default because it measured worse), a new
/// stream at the automatic bitrate times the link first when the PC offers the test
/// (`Feature::BandwidthProbe`), and starts no higher than it holds. PyroWave has its own test and
/// a minimum bitrate; a resume keeps the bitrate of the running session's launch.
pub(super) fn probes_first_bitrate(settings: &Settings, resume: bool, info: &ServerInfo) -> bool {
    settings.link_probe && !resume && settings.bitrate_kbps == 0
        && !matches!(settings.codec, CodecPreference::PyroWave | CodecPreference::PyroWave444)
        && info.paired && info.capabilities.has(Feature::BandwidthProbe)
}

pub(super) fn capped_bitrate(bitrate: u32, limit: u32) -> u32 {
    if limit == 0 { bitrate } else { bitrate.min(limit) }
}

/// The formats offered for a codec preference, limited to what the host encodes.
pub fn video_formats(preference: CodecPreference, hdr: bool, host_scm: u32) -> u32 {
    let mut formats = match preference {
        CodecPreference::H264 => VIDEO_FORMAT_H264,
        CodecPreference::Hevc => VIDEO_FORMAT_H264 | VIDEO_FORMAT_H265,
        CodecPreference::Av1 => VIDEO_FORMAT_H264 | VIDEO_FORMAT_AV1_MAIN8,
        CodecPreference::Auto => VIDEO_FORMAT_H264 | VIDEO_FORMAT_H265,
        CodecPreference::PyroWave => VIDEO_FORMAT_H264 | VIDEO_FORMAT_H265 | VIDEO_FORMAT_PYROWAVE,
        CodecPreference::PyroWave444 => VIDEO_FORMAT_H264 | VIDEO_FORMAT_H265 | VIDEO_FORMAT_PYROWAVE | VIDEO_FORMAT_PYROWAVE_444,
    };
    if hdr {
        if formats & VIDEO_FORMAT_H265 != 0 {
            formats |= VIDEO_FORMAT_H265_MAIN10;
        }
        if formats & VIDEO_FORMAT_AV1_MAIN8 != 0 {
            formats |= VIDEO_FORMAT_AV1_MAIN10;
        }
        if formats & VIDEO_FORMAT_PYROWAVE != 0 { formats |= VIDEO_FORMAT_PYROWAVE_MAIN10; }
        if formats & VIDEO_FORMAT_PYROWAVE_444 != 0 { formats |= VIDEO_FORMAT_PYROWAVE_MAIN10_444; }
    }
    // Hosts that report codec support get only what they encode; H.264 always stays.
    if host_scm != 0 {
        if host_scm & SCM_HEVC == 0 {
            formats &= !VIDEO_FORMAT_H265;
        }
        if host_scm & SCM_HEVC_MAIN10 == 0 {
            formats &= !VIDEO_FORMAT_H265_MAIN10;
        }
        if host_scm & SCM_AV1_MAIN8 == 0 {
            formats &= !VIDEO_FORMAT_AV1_MAIN8;
        }
        if host_scm & SCM_AV1_MAIN10 == 0 {
            formats &= !VIDEO_FORMAT_AV1_MAIN10;
        }
        for (scm, format) in [(SCM_PYROWAVE, VIDEO_FORMAT_PYROWAVE), (SCM_PYROWAVE_444, VIDEO_FORMAT_PYROWAVE_444),
            (SCM_PYROWAVE_MAIN10, VIDEO_FORMAT_PYROWAVE_MAIN10), (SCM_PYROWAVE_MAIN10_444, VIDEO_FORMAT_PYROWAVE_MAIN10_444)] {
            if host_scm & scm == 0 { formats &= !format; }
        }
    }
    formats | VIDEO_FORMAT_H264
}

/// What "Prefer YUV 4:4:4" and "10-bit color (SDR)" ask of `color::offer` with these settings.
pub fn color_wants(settings: &Settings) -> color::Wants {
    color::Wants {
        yuv444: settings.prefer_yuv444,
        sdr10: settings.sdr_10bit,
        hdr: settings.hdr,
        packed444: settings.experimental_packed_444,
        pyrowave: settings.codec.is_pyrowave(),
    }
}

/// Whether a launch checks the decoder (`color_probe::probe`): when one of the settings asks and
/// nothing that takes the stream first (HDR, packed full chroma, PyroWave) refuses it anyway.
pub fn probes_color(settings: &Settings) -> bool {
    let wants = color_wants(settings);
    (wants.yuv444 || wants.sdr10) && !wants.hdr && !wants.packed444 && !wants.pyrowave
}

/// The formats a launch offers: the codec choice's, and the 4:4:4 and 10-bit ones the settings
/// add where this device (`decodable`, from the probe) and the PC (`scm`) both handle them.
/// Packed full chroma offers the plain formats whatever the PC reports.
pub(super) fn offered_formats(settings: &Settings, scm: u32, decodable: &(u32, String)) -> color::Offer {
    let base = video_formats(settings.codec, settings.hdr, if settings.experimental_packed_444 { 0 } else { scm });
    color::offer(base, color_wants(settings), decodable.0, &decodable.1, scm)
}

/// Whether the launch says `hdrMode=0`: an SDR stream that offers a 10-bit format. Without it a
/// host streams that format as HDR, and an SDR display ends the stream. The format is picked
/// after the launch, so a stream that ends up 8-bit says it too: for an 8-bit format it means
/// what saying nothing does.
pub(super) fn launches_ten_bit_sdr(settings: &Settings, offer: &color::Offer) -> bool {
    !settings.hdr && offer.formats & VIDEO_FORMAT_MASK_10BIT != 0
}

/// The HDR format beyond stock HDR10 to ask a Rubylight host for (one `rubylight_protocol::hdr`
/// bit, 0 for stock HDR10), as Android's HdrFormatPolicy picks it. Dolby Vision where the client
/// reads its RPUs (`reads_dolby_vision`: the Windows draw pass, whose tone map takes each frame's
/// peak from them): profile 10.1 on AV1 Main10 before 8.1 on HEVC Main10, as the handshake takes
/// AV1 first, both with the HDR10 base layer every other path shows as HDR10. Not with packed
/// full chroma, which draws PQ only. Never HLG: Android asks for it only on a display without
/// HDR10, and every desktop HDR output takes PQ. A host that lists no formats (every other
/// host, `host_formats` -1) gets 0, and the ANNOUNCE asks only for what the stream's DESCRIBE
/// offers for its codec (`sdp::announced_hdr_format`).
pub(super) fn hdr_format(hdr: bool, host_formats: i32, formats: u32, packed444: bool, reads_dolby_vision: bool) -> u8 {
    use rubylight_protocol::hdr;
    if !hdr || host_formats <= 0 || packed444 || !reads_dolby_vision {
        return 0;
    }
    let host = host_formats as u8;
    if host & hdr::DV_101 != 0 && formats & VIDEO_FORMAT_AV1_MAIN10 != 0 {
        hdr::DV_101
    } else if host & hdr::DV_81 != 0 && formats & VIDEO_FORMAT_H265_MAIN10 != 0 {
        hdr::DV_81
    } else {
        0
    }
}

/// The Windows draw pass reads Dolby Vision RPUs for its tone map, unless `NIGHTJAR_D3D11_PASS=0`
/// leaves the pictures to SDL. Nothing else does.
#[cfg(windows)]
pub(super) fn reads_dolby_vision() -> bool {
    direct::enabled()
}

#[cfg(not(windows))]
pub(super) fn reads_dolby_vision() -> bool {
    false
}

/// `hdr_format`, or the bit `NIGHTJAR_HDR_FORMAT` asks for instead (tests: HLG, which desktop never
/// picks itself), said in the log when it is not stock HDR10.
pub(super) fn asked_hdr_format(picked: u8) -> u8 {
    let format = match hint_env("NIGHTJAR_HDR_FORMAT") {
        Some(bit) => {
            let bit = u8::try_from(bit).unwrap_or(0);
            eprintln!("nightjar: NIGHTJAR_HDR_FORMAT asks the host for HDR format {bit} (picked: {picked})");
            bit
        }
        None => picked,
    };
    let name = match format {
        rubylight_protocol::hdr::DV_101 => "Dolby Vision profile 10.1",
        rubylight_protocol::hdr::DV_81 => "Dolby Vision profile 8.1",
        rubylight_protocol::hdr::DV_84 => "Dolby Vision profile 8.4",
        rubylight_protocol::hdr::HLG => "HLG",
        _ => "",
    };
    if !name.is_empty() {
        eprintln!("nightjar: asking the host for {name} where the stream's codec takes it");
    }
    format
}

fn refusal_text(refusal: &color::Refusal) -> String {
    use color::Refusal;
    match refusal {
        Refusal::Off | Refusal::Hdr => String::new(),
        Refusal::Packed444 => tr("full chroma is on and has its own 4:4:4 and 10-bit handling.").into(),
        Refusal::PyroWave => tr("PyroWave is chosen, and it has its own 4:4:4 and 10-bit modes.").into(),
        Refusal::Decoder(why) => trf("this device can’t decode or draw it ({}).", &[why]),
        Refusal::Host => tr("the PC doesn’t encode it for this codec.").into(),
        Refusal::Yuv444 => tr("4:4:4 is on, and the PC takes 4:4:4 or 10-bit, not both.").into(),
    }
}

/// What to tell the player once the stream that `offer` was made for streams `negotiated`: what
/// was granted, or why not, one line each; None when neither setting was on or had a word.
pub(super) fn color_notice(offer: &color::Offer, negotiated: u32) -> Option<String> {
    let mut lines = Vec::new();
    if offer.yuv444.is_ok() && negotiated & VIDEO_FORMAT_MASK_YUV444 != 0 {
        lines.push(tr("Streaming in YUV 4:4:4.").to_owned());
    }
    if offer.sdr10.is_ok() && negotiated & VIDEO_FORMAT_MASK_10BIT != 0 {
        lines.push(tr("Streaming 10-bit color (SDR).").to_owned());
    }
    for notice in color::notices(offer, negotiated) {
        lines.push(match notice {
            color::Notice::Yuv444Unavailable(refusal) => trf("YUV 4:4:4 unavailable; using 4:2:0: {}", &[&refusal_text(&refusal)]),
            color::Notice::Sdr10Unavailable(refusal) => trf("10-bit SDR isn’t used, so the stream is 8-bit: {}", &[&refusal_text(&refusal)]),
            color::Notice::Sdr10NotAccepted => tr("This PC didn’t accept 10-bit, so the stream is 8-bit.").to_owned(),
        });
    }
    (!lines.is_empty()).then(|| lines.join("\n"))
}

/// When PyroWave was chosen and the stream runs another codec: the log line saying why, and
/// what to tell the player. A device that can't decode PyroWave never gets here: the launch
/// stops first with its reason (`pyrowave_mode::availability`).
/// `scm` is the codec support trusted for the offer, `reported` the serverinfo value before
/// `trusted_codec_mode_support`.
pub(super) fn pyrowave_fallback(codec: CodecPreference, (scm, reported): (u32, u32), offered: u32, negotiated: u32) -> Option<(String, String)> {
    if !codec.is_pyrowave() || negotiated & VIDEO_FORMAT_MASK_PYROWAVE != 0 {
        return None;
    }
    let name = codec_name(negotiated);
    let why = if offered & VIDEO_FORMAT_MASK_PYROWAVE == 0 {
        "the host does not offer it"
    } else if scm == 0 {
        "the host reports no trusted codec support and chose another codec"
    } else {
        "the host chose another codec"
    };
    Some((
        format!("nightjar: PyroWave was chosen, but {why} (codec support {scm:#x}, reported {reported:#x}); streaming {name}"),
        trf("This PC doesn’t offer PyroWave, so Nightjar uses {}.", &[&name]),
    ))
}

/// The PyroWave fallback notice once per PC, like Android's `HostNotices`: the text the first time
/// a stream from it falls back, then nothing until PyroWave streams from it again. Reconnects
/// and later streams stay quiet. `fallback` and `format` are the stream's (`Facts`). Returns the
/// text to show and whether `record` changed.
pub fn pyrowave_notice(record: &mut HostRecord, chosen: bool, fallback: Option<&str>, format: u32) -> (Option<String>, bool) {
    use crate::config::NOTICE_PYROWAVE_FALLBACK;
    match fallback {
        Some(text) => {
            let first = record.claim_notice(NOTICE_PYROWAVE_FALLBACK);
            (first.then(|| text.to_owned()), first)
        }
        None if chosen && format & VIDEO_FORMAT_MASK_PYROWAVE != 0 => (None, record.reset_notice(NOTICE_PYROWAVE_FALLBACK)),
        None => (None, false),
    }
}
