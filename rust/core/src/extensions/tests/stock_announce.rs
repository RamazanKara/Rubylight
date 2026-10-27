//! Whole ANNOUNCE bodies as stock clients send them to a Sunshine-family host,
//! built the way moonlight-common-c's `SdpGenerator.c` builds them for one that
//! reports 7.1.431 or later: Moonlight-qt, Android and iOS all use it, and
//! Nightjar's port (`client/protocol/client/src/sdp.rs`) adds packed 4:4:4 and
//! the microphone. Each goes through the host's ANNOUNCE path and is accepted
//! whatever the host offers.
use super::ALL_ON;
use crate::{config::Config, extensions::Offered, rtsp::Negotiated, stream_policy};

#[derive(Clone)]
struct Client {
    /// `x-ml-*` and `x-ss-*`, which builds before Sunshine support leave out.
    sunshine: bool,
    width: u32,
    height: u32,
    fps: u32,
    refresh_x100: u32,
    bitrate_kbps: u32,
    packet_size: u32,
    remote: bool,
    /// The `bitStreamFormat`: 0 H.264, 1 HEVC, 2 AV1.
    codec: u8,
    ten_bit: bool,
    yuv444: bool,
    /// `x-ss-general.encryptionEnabled`: control (1), video (2), audio (4), microphone.
    encryption: u32,
    channels: u8,
    channel_mask: u32,
    high_quality_audio: bool,
    audio_packet_ms: u8,
    /// A decoder with reference frame invalidation.
    rfi: bool,
    slices: u32,
    csc: u8,
    /// Nightjar's packed 4:4:4 version, 0 for none.
    packed: u8,
}

/// Moonlight-qt's defaults on a LAN: 1080p60 H.264, stereo, encrypted audio.
fn moonlight_qt() -> Client {
    Client {
        sunshine: true,
        width: 1920,
        height: 1080,
        fps: 60,
        refresh_x100: 6000,
        bitrate_kbps: 20_000,
        packet_size: 1392,
        remote: false,
        codec: 0,
        ten_bit: false,
        yuv444: false,
        encryption: 1 | 4,
        channels: 2,
        channel_mask: 0x3,
        high_quality_audio: false,
        audio_packet_ms: 5,
        rfi: true,
        slices: 1,
        csc: 2,
        packed: 0,
    }
}

fn sdp(c: &Client) -> String {
    let mut a = String::new();
    let mut add = |name: &str, value: &dyn std::fmt::Display| {
        a.push_str(&format!("a={name}:{value} \r\n"));
    };
    if c.sunshine {
        // FEC status and session ID v1.
        add("x-ml-general.featureFlags", &3);
        add("x-ss-general.encryptionEnabled", &c.encryption);
        add("x-ss-video[0].chromaSamplingType", &u8::from(c.yuv444));
    }
    add("x-nv-video[0].clientViewportWd", &c.width);
    if c.packed != 0 {
        add(crate::packed444::REQUEST_ATTRIBUTE, &c.packed);
    }
    add("x-nv-video[0].clientViewportHt", &c.height);
    add("x-nv-video[0].maxFPS", &c.fps);
    // Encrypted video takes its 32-byte header out of the packet.
    let packet_size = c.packet_size - if c.encryption & 2 != 0 { 32 } else { 0 };
    add("x-nv-video[0].packetSize", &packet_size);
    add("x-nv-video[0].rateControlMode", &4);
    add("x-nv-video[0].timeoutLengthMs", &7000);
    add("x-nv-video[0].framesWithInvalidRefThreshold", &0);
    let share = if c.packed != 0 { 7 } else { 8 };
    let mut adjusted = c.bitrate_kbps * share / 10;
    if c.remote && adjusted > 500 {
        adjusted -= 500;
    }
    adjusted = adjusted.min(if c.packed != 0 { 140_000 } else { 100_000 });
    add("x-nv-video[0].initialBitrateKbps", &adjusted);
    add("x-nv-video[0].initialPeakBitrateKbps", &adjusted);
    add("x-nv-vqos[0].bw.minimumBitrateKbps", &adjusted);
    add("x-nv-vqos[0].bw.maximumBitrateKbps", &adjusted);
    if c.sunshine {
        add("x-ml-video.configuredBitrateKbps", &c.bitrate_kbps);
    }
    add("x-nv-vqos[0].fec.enable", &1);
    add("x-nv-vqos[0].videoQualityScoreUpdateTime", &5000);
    let (video_qos, audio_qos) = if c.remote { (0, 0) } else { (5, 4) };
    add("x-nv-vqos[0].qosTrafficType", &video_qos);
    add("x-nv-aqos.qosTrafficType", &audio_qos);
    let audio_encryption = if c.encryption & 4 != 0 { 0x20 } else { 0 };
    add(
        "x-nv-general.featureFlags",
        &(0x07 | 0x80 | audio_encryption),
    );
    add("x-nv-general.useReliableUdp", &13);
    add("x-nv-vqos[0].fec.minRequiredFecPackets", &2);
    add("x-nv-vqos[0].bllFec.enable", &0);
    if c.width < 720 || c.height < 540 {
        add("x-nv-vqos[0].drc.enable", &1);
        add("x-nv-vqos[0].drc.tableType", &2);
    } else {
        add("x-nv-vqos[0].drc.enable", &0);
    }
    add("x-nv-general.enableRecoveryMode", &0);
    add("x-nv-video[0].videoEncoderSlicesPerFrame", &c.slices);
    match c.codec {
        2 => {}
        hevc => add("x-nv-clientSupportHevc", &hevc),
    }
    add("x-nv-vqos[0].bitStreamFormat", &c.codec);
    add("x-nv-video[0].dynamicRangeMode", &u8::from(c.ten_bit));
    add("x-nv-video[0].maxNumReferenceFrames", &u8::from(!c.rfi));
    add("x-nv-video[0].clientRefreshRateX100", &c.refresh_x100);
    add("x-nv-audio.surround.numChannels", &c.channels);
    add("x-nv-audio.surround.channelMask", &c.channel_mask);
    add("x-nv-audio.surround.enable", &u8::from(c.channels > 2));
    add(
        "x-nv-audio.surround.AudioQuality",
        &u8::from(c.high_quality_audio),
    );
    add("x-nv-aqos.packetDuration", &c.audio_packet_ms);
    add("x-nv-video[0].encoderCscMode", &c.csc);
    format!(
        "v=0\r\no=android 0 14 IN IPv4 192.168.1.20\r\ns=NVIDIA Streaming Client\r\n{a}t=0 0\r\nm=video 47998  \r\n"
    )
}

/// The clients, by what they really send.
fn clients() -> Vec<(&'static str, Client)> {
    let qt = moonlight_qt();
    vec![
        ("Moonlight-qt defaults", qt.clone()),
        (
            "Moonlight-qt 4K120 HEVC HDR, 7.1",
            Client {
                width: 3840,
                height: 2160,
                fps: 120,
                refresh_x100: 11988,
                bitrate_kbps: 150_000,
                codec: 1,
                ten_bit: true,
                channels: 8,
                channel_mask: 0x63F,
                high_quality_audio: true,
                csc: 4,
                ..qt.clone()
            },
        ),
        (
            "Moonlight-qt 1440p165 HEVC 4:4:4, 5.1, full range",
            Client {
                width: 2560,
                height: 1440,
                fps: 165,
                refresh_x100: 16500,
                bitrate_kbps: 80_000,
                codec: 1,
                yuv444: true,
                channels: 6,
                channel_mask: 0x3F,
                high_quality_audio: true,
                csc: 3,
                ..qt.clone()
            },
        ),
        (
            "Moonlight-qt AV1 HDR over the internet, everything encrypted",
            Client {
                width: 3840,
                height: 2160,
                bitrate_kbps: 50_000,
                packet_size: 1024,
                remote: true,
                codec: 2,
                ten_bit: true,
                encryption: 1 | 2 | 4,
                csc: 4,
                ..qt.clone()
            },
        ),
        (
            "Android phone 2400x1080 HEVC with a slow Opus decoder",
            Client {
                width: 2400,
                height: 1080,
                fps: 120,
                refresh_x100: 12000,
                bitrate_kbps: 40_000,
                codec: 1,
                audio_packet_ms: 10,
                ..qt.clone()
            },
        ),
        (
            "Android TV 640x360 H.264, with a DRC table",
            Client {
                width: 640,
                height: 360,
                fps: 30,
                refresh_x100: 5994,
                bitrate_kbps: 2_000,
                rfi: false,
                csc: 0,
                ..qt.clone()
            },
        ),
        (
            "iPad ProMotion 2732x2048 HEVC HDR",
            Client {
                width: 2732,
                height: 2048,
                refresh_x100: 12000,
                bitrate_kbps: 60_000,
                codec: 1,
                ten_bit: true,
                slices: 4,
                csc: 4,
                ..qt.clone()
            },
        ),
        (
            "A build before Sunshine support",
            Client {
                sunshine: false,
                codec: 1,
                ..qt.clone()
            },
        ),
        (
            "Nightjar packed 4:4:4 HEVC with the microphone",
            Client {
                bitrate_kbps: 120_000,
                codec: 1,
                encryption: 1 | 4 | crate::mic::ENCRYPTION,
                packed: crate::packed444::VERSION,
                ..qt.clone()
            },
        ),
        (
            "Nightjar layered packed 4:4:4 AV1, ten-bit",
            Client {
                width: 1366,
                height: 768,
                bitrate_kbps: 200_000,
                codec: 2,
                ten_bit: true,
                packed: crate::packed444::LAYERED_VERSION,
                csc: 4,
                ..qt
            },
        ),
    ]
}

/// What the host's ANNOUNCE does with a body: the client's picks are checked,
/// host policy applied, then the stream validated.
fn announce(body: &str, offered: &Offered, config: &Config, sdr_requested: bool) -> Negotiated {
    let mut n = Negotiated::from_sdp(body.as_bytes()).unwrap_or_else(|e| panic!("{e}\n{body}"));
    offered.check_announce(&n).unwrap();
    // Launched at the rate it asks for.
    let launch_millihz = n.fps_millihz();
    stream_policy::apply(&mut n, launch_millihz, config);
    stream_policy::infer_sdr_10bit(&mut n, sdr_requested);
    if n.chroma_packing == 0 {
        stream_policy::apply_color(&mut n, config);
    }
    n.validate().unwrap_or_else(|e| panic!("{e}\n{body}"));
    offered.check_announce(&n).unwrap();
    n
}

#[test]
fn stock_announce_bodies_are_read_as_their_clients_sent_them() {
    for (name, c) in clients() {
        let body = sdp(&c);
        let n = Negotiated::from_sdp(body.as_bytes()).unwrap_or_else(|e| panic!("{name}: {e}"));
        // A build before Sunshine support asks for audio encryption in its feature flags only.
        let encryption = (if c.sunshine { c.encryption } else { 0 }) | (c.encryption & 4);
        assert_eq!(
            (
                (n.width, n.height, n.fps),
                (n.codec, n.hdr, n.yuv444, n.chroma_packing),
                (n.audio_channels, n.audio_packet_ms, n.audio_quality),
                (n.encryption, n.csc_mode, n.slices),
                (n.hdr_format, n.audio_codec),
            ),
            (
                (c.width, c.height, c.fps),
                (c.codec, c.ten_bit, c.yuv444, c.packed),
                (c.channels, c.audio_packet_ms, c.high_quality_audio),
                (encryption, c.csc, c.slices),
                (0, 0),
            ),
            "{name}"
        );
    }
}

#[test]
fn stock_announce_bodies_pass_the_host_path_whatever_is_offered() {
    for config in [
        "",
        "prefer_sdr_10bit=true\n",
        "max_bitrate=10000\nfec_percentage=50\n",
    ] {
        let config = Config::parse(config).unwrap();
        for (name, c) in clients() {
            let body = sdp(&c);
            for offered in [&Offered::NONE, &ALL_ON] {
                for sdr_requested in [false, true] {
                    let n = announce(&body, offered, &config, sdr_requested);
                    assert_eq!((n.hdr_format, n.audio_codec), (0, 0), "{name}");
                }
            }
        }
    }
}
