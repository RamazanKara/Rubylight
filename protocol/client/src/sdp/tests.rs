//! Golden descriptions, written line by line from android_sdp.c for each configuration.

use super::*;

fn lines(attributes: &[&str]) -> String {
    attributes.iter().map(|a| format!("a={a} \r\n")).collect()
}

fn document(version: i32, family_address: &str, attributes: &[&str], port: u16) -> String {
    format!(
        "v=0\r\no=android 0 {version} IN {family_address}\r\ns=NVIDIA Streaming Client\r\n{}t=0 0\r\nm=video {port}  \r\n",
        lines(attributes)
    )
}

fn params<'a>(
    config: &'a StreamConfig,
    version: [i32; 4],
    address: &str,
    format: u32,
) -> SdpParams<'a> {
    SdpParams {
        config,
        app_version: version,
        address: address.parse().unwrap(),
        rtsp_port: 48010,
        video_port: 47998,
        rtsp_client_version: 14,
        negotiated_video_format: format,
        video_capabilities: config.video_capabilities,
        encryption_supported: 0,
        encryption_requested: 0,
        mic_port: 0,
        high_quality_surround_supported: false,
        pyrowave_records_enabled: false,
    }
}

const SUNSHINE: [i32; 4] = [7, 1, 431, -1];

#[test]
fn packed_bitrate_reserves_packet_overhead_and_fec() {
    for (budget, location, video) in [
        (80_000, StreamLocation::Local, 56_000),
        (200_000, StreamLocation::Local, 140_000),
        (200_000, StreamLocation::Remote, 139_500),
        (300_000, StreamLocation::Local, 140_000),
    ] {
        let config = StreamConfig {
            bitrate_kbps: budget,
            location,
            packet_size: 1392,
            encryption_flags: ENCFLG_ALL,
            experimental_packed_444: true,
            ..Default::default()
        };
        let mut p = params(&config, SUNSHINE, "::1", VIDEO_FORMAT_H265);
        p.encryption_supported = SS_ENC_VIDEO;
        let sdp = generate(&p);
        assert_eq!(sdp.packet_size, 1360);
        assert!(sdp.text.contains(&format!(
            "a=x-nv-vqos[0].bw.maximumBitrateKbps:{video} \r\n"
        )));
        assert!(sdp
            .text
            .contains(&format!("a=x-ml-video.configuredBitrateKbps:{budget} \r\n")));
    }
}

#[test]
fn sunshine_hevc_1080p60_local_stereo_encrypted() {
    let config = StreamConfig {
        width: 1920,
        height: 1080,
        fps: 60,
        bitrate_kbps: 20000,
        packet_size: 1392,
        client_refresh_rate_x100: 6000,
        encryption_flags: ENCFLG_ALL,
        video_capabilities: CAPABILITY_REFERENCE_FRAME_INVALIDATION_HEVC,
        ..Default::default()
    };
    let mut p = params(&config, SUNSHINE, "192.168.1.20", VIDEO_FORMAT_H265);
    p.encryption_supported = SS_ENC_CONTROL_V2 | SS_ENC_VIDEO | SS_ENC_AUDIO;
    p.encryption_requested = SS_ENC_CONTROL_V2;
    let sdp = generate(&p);
    assert_eq!(
        sdp.text,
        document(
            14,
            "IPv4 192.168.1.20",
            &[
                "x-ml-general.featureFlags:3",
                "x-ss-general.encryptionEnabled:7",
                "x-ss-video[0].chromaSamplingType:0",
                "x-nv-video[0].clientViewportWd:1920",
                "x-nv-video[0].clientViewportHt:1080",
                "x-nv-video[0].maxFPS:60",
                "x-nv-video[0].packetSize:1360",
                "x-nv-video[0].rateControlMode:4",
                "x-nv-video[0].timeoutLengthMs:7000",
                "x-nv-video[0].framesWithInvalidRefThreshold:0",
                "x-nv-video[0].initialBitrateKbps:16000",
                "x-nv-video[0].initialPeakBitrateKbps:16000",
                "x-nv-vqos[0].bw.minimumBitrateKbps:16000",
                "x-nv-vqos[0].bw.maximumBitrateKbps:16000",
                "x-ml-video.configuredBitrateKbps:20000",
                "x-nv-vqos[0].fec.enable:1",
                "x-nv-vqos[0].videoQualityScoreUpdateTime:5000",
                "x-nv-vqos[0].qosTrafficType:5",
                "x-nv-aqos.qosTrafficType:4",
                "x-nv-general.featureFlags:167",
                "x-nv-general.useReliableUdp:13",
                "x-nv-vqos[0].fec.minRequiredFecPackets:2",
                "x-nv-vqos[0].bllFec.enable:0",
                "x-nv-vqos[0].drc.enable:0",
                "x-nv-general.enableRecoveryMode:0",
                "x-nv-video[0].videoEncoderSlicesPerFrame:1",
                "x-nv-clientSupportHevc:1",
                "x-nv-vqos[0].bitStreamFormat:1",
                "x-nv-video[0].dynamicRangeMode:0",
                "x-nv-video[0].maxNumReferenceFrames:0",
                "x-nv-video[0].clientRefreshRateX100:6000",
                "x-nv-audio.surround.numChannels:2",
                "x-nv-audio.surround.channelMask:3",
                "x-nv-audio.surround.enable:0",
                "x-nv-audio.surround.AudioQuality:0",
                "x-nv-aqos.packetDuration:5",
                "x-nv-video[0].encoderCscMode:2",
            ],
            47998
        )
    );
    assert_eq!(sdp.encryption_enabled, 7);
    assert!(sdp.audio_encryption_enabled);
    assert_eq!((sdp.packet_size, sdp.audio_packet_duration_ms), (1360, 5));
    assert!(!sdp.high_quality_surround_enabled);
}

#[test]
fn sunshine_av1_4k120_remote_71_with_required_encryption_mic_and_haptics() {
    let config = StreamConfig {
        width: 3840,
        height: 2160,
        fps: 120,
        bitrate_kbps: 150_000,
        packet_size: 1024,
        location: StreamLocation::Remote,
        audio: AudioConfiguration::SURROUND_71,
        client_refresh_rate_x100: 11988,
        color_space: COLORSPACE_REC_2020,
        color_range: COLOR_RANGE_FULL,
        encryption_flags: ENCFLG_NONE,
        video_capabilities: capability_slices_per_frame(4)
            | CAPABILITY_REFERENCE_FRAME_INVALIDATION_AV1,
        audio_capabilities: CAPABILITY_SUPPORTS_ARBITRARY_AUDIO_DURATION,
        haptics_pcm: true,
        microphone: true,
        ..Default::default()
    };
    let mut p = params(
        &config,
        SUNSHINE,
        "2001:db8::5",
        VIDEO_FORMAT_AV1_HIGH10_444,
    );
    p.encryption_supported = 15;
    p.encryption_requested = 15;
    p.mic_port = 48001;
    p.high_quality_surround_supported = true;
    p.video_port = 48100;
    let sdp = generate(&p);
    assert_eq!(
        sdp.text,
        document(
            14,
            "IPv6 [2001:db8::5]",
            &[
                "x-ml-general.featureFlags:7",
                "x-ss-general.encryptionEnabled:15",
                "x-ss-video[0].chromaSamplingType:1",
                "x-nv-video[0].clientViewportWd:3840",
                "x-nv-video[0].clientViewportHt:2160",
                "x-nv-video[0].maxFPS:120",
                "x-nv-video[0].packetSize:992",
                "x-nv-video[0].rateControlMode:4",
                "x-nv-video[0].timeoutLengthMs:7000",
                "x-nv-video[0].framesWithInvalidRefThreshold:0",
                "x-nv-video[0].initialBitrateKbps:100000",
                "x-nv-video[0].initialPeakBitrateKbps:100000",
                "x-nv-vqos[0].bw.minimumBitrateKbps:100000",
                "x-nv-vqos[0].bw.maximumBitrateKbps:100000",
                "x-ml-video.configuredBitrateKbps:150000",
                "x-nv-vqos[0].fec.enable:1",
                "x-nv-vqos[0].videoQualityScoreUpdateTime:5000",
                "x-nv-vqos[0].qosTrafficType:0",
                "x-nv-aqos.qosTrafficType:0",
                "x-nv-general.featureFlags:167",
                "x-nv-general.useReliableUdp:13",
                "x-nv-vqos[0].fec.minRequiredFecPackets:2",
                "x-nv-vqos[0].bllFec.enable:0",
                "x-nv-vqos[0].drc.enable:0",
                "x-nv-general.enableRecoveryMode:0",
                "x-nv-video[0].videoEncoderSlicesPerFrame:4",
                "x-nv-vqos[0].bitStreamFormat:2",
                "x-nv-video[0].dynamicRangeMode:1",
                "x-nv-video[0].maxNumReferenceFrames:0",
                "x-nv-video[0].clientRefreshRateX100:11988",
                "x-nv-audio.surround.numChannels:8",
                "x-nv-audio.surround.channelMask:1599",
                "x-nv-audio.surround.enable:1",
                "x-nv-audio.surround.AudioQuality:1",
                "x-nv-aqos.packetDuration:5",
                "x-nv-video[0].encoderCscMode:5",
            ],
            48100
        )
    );
    assert_eq!(sdp.encryption_enabled, 15);
    assert!(sdp.high_quality_surround_enabled);
}

#[test]
fn sunshine_pyrowave_with_records_and_pacing_hints() {
    let config = StreamConfig {
        width: 2560,
        height: 1440,
        fps: 240,
        bitrate_kbps: 800_000,
        packet_size: 1392,
        client_refresh_rate_x100: 24000,
        color_space: COLORSPACE_REC_2020,
        encryption_flags: ENCFLG_VIDEO | ENCFLG_AUDIO,
        pyrowave_link_mbps: 2500,
        pyrowave_pace_mbps: 2100,
        video_capabilities: CAPABILITY_REFERENCE_FRAME_INVALIDATION_HEVC,
        audio_capabilities: CAPABILITY_SLOW_OPUS_DECODER,
        ..Default::default()
    };
    let mut p = params(&config, SUNSHINE, "10.0.0.7", VIDEO_FORMAT_PYROWAVE_MAIN10);
    p.encryption_supported = SS_ENC_CONTROL_V2 | SS_ENC_AUDIO;
    p.encryption_requested = SS_ENC_CONTROL_V2;
    p.pyrowave_records_enabled = true;
    let sdp = generate(&p);
    assert_eq!(
        sdp.text,
        document(
            14,
            "IPv4 10.0.0.7",
            &[
                "x-ml-general.featureFlags:3",
                "x-ss-general.encryptionEnabled:5",
                "x-ss-video[0].chromaSamplingType:0",
                "x-nv-video[0].clientViewportWd:2560",
                "x-nv-video[0].clientViewportHt:1440",
                "x-nv-video[0].maxFPS:240",
                "x-nv-video[0].packetSize:1392",
                "x-nv-video[0].rateControlMode:4",
                "x-nv-video[0].timeoutLengthMs:7000",
                "x-nv-video[0].framesWithInvalidRefThreshold:0",
                "x-nv-video[0].initialBitrateKbps:100000",
                "x-nv-video[0].initialPeakBitrateKbps:100000",
                "x-nv-vqos[0].bw.minimumBitrateKbps:100000",
                "x-nv-vqos[0].bw.maximumBitrateKbps:100000",
                "x-ml-video.configuredBitrateKbps:800000",
                "x-nv-vqos[0].fec.enable:1",
                "x-nv-vqos[0].videoQualityScoreUpdateTime:5000",
                "x-nv-vqos[0].qosTrafficType:5",
                "x-nv-aqos.qosTrafficType:4",
                "x-nv-general.featureFlags:167",
                "x-nv-general.useReliableUdp:13",
                "x-nv-vqos[0].fec.minRequiredFecPackets:2",
                "x-nv-vqos[0].bllFec.enable:0",
                "x-nv-vqos[0].drc.enable:0",
                "x-nv-general.enableRecoveryMode:0",
                "x-nv-video[0].videoEncoderSlicesPerFrame:1",
                "x-nv-vqos[0].bitStreamFormat:3",
                "x-ss-video[0].pyrowaveFeatures:1",
                "x-ss-video[0].pyrowavePaceMbps:2100",
                "x-ss-video[0].pyrowaveLinkMbps:2500",
                "x-nv-video[0].dynamicRangeMode:1",
                "x-nv-video[0].maxNumReferenceFrames:1",
                "x-nv-video[0].clientRefreshRateX100:24000",
                "x-nv-audio.surround.numChannels:2",
                "x-nv-audio.surround.channelMask:3",
                "x-nv-audio.surround.enable:0",
                "x-nv-audio.surround.AudioQuality:0",
                "x-nv-aqos.packetDuration:10",
                "x-nv-video[0].encoderCscMode:4",
            ],
            47998
        )
    );
}

/// The `a=` line of an attribute, or None when the description lacks it.
fn attribute<'a>(sdp: &'a str, name: &str) -> Option<&'a str> {
    let prefix = format!("a={name}:");
    sdp.lines().find(|l| l.starts_with(&prefix)).map(|l| l.trim_end())
}

#[test]
fn link_speed_hint_is_sent_for_every_codec_and_only_pyrowave_gets_the_pace() {
    const LINK: &str = "x-ss-video[0].pyrowaveLinkMbps";
    const PACE: &str = "x-ss-video[0].pyrowavePaceMbps";
    let formats = [
        (VIDEO_FORMAT_H264, false),
        (VIDEO_FORMAT_H265, false),
        (VIDEO_FORMAT_AV1_MAIN8, false),
        (VIDEO_FORMAT_PYROWAVE, true),
    ];
    for (format, pyrowave) in formats {
        // Wired with a known speed and a calibrated pace.
        let config =
            StreamConfig { pyrowave_link_mbps: 2500, pyrowave_pace_mbps: 2100, ..Default::default() };
        let sdp = generate(&params(&config, SUNSHINE, "10.0.0.7", format)).text;
        assert_eq!(attribute(&sdp, LINK), Some("a=x-ss-video[0].pyrowaveLinkMbps:2500"), "{format:#x}");
        assert_eq!(attribute(&sdp, PACE).is_some(), pyrowave, "{format:#x}");

        // Wi-Fi, cellular or an unreadable port reports 0: no hint, as before.
        let config = StreamConfig { pyrowave_link_mbps: 0, pyrowave_pace_mbps: 0, ..Default::default() };
        let sdp = generate(&params(&config, SUNSHINE, "10.0.0.7", format)).text;
        assert_eq!(attribute(&sdp, LINK), None, "{format:#x}");
        assert_eq!(attribute(&sdp, PACE), None, "{format:#x}");
    }
}

#[test]
fn row_order_is_requested_only_for_pyrowave_with_records_and_the_setting() {
    const ROW_ORDER: &str = "x-ss-video[0].pyrowaveRowOrder";
    for (format, records, row_order, sent) in [
        (VIDEO_FORMAT_PYROWAVE, true, true, true),
        (VIDEO_FORMAT_PYROWAVE, true, false, false),
        (VIDEO_FORMAT_PYROWAVE, false, true, false),
        (VIDEO_FORMAT_H264, true, true, false),
        (VIDEO_FORMAT_H265, true, true, false),
    ] {
        let config = StreamConfig { pyrowave_row_order: row_order, ..Default::default() };
        let mut p = params(&config, SUNSHINE, "10.0.0.7", format);
        p.pyrowave_records_enabled = records;
        let sdp = generate(&p).text;
        let line = attribute(&sdp, ROW_ORDER);
        assert_eq!(line, sent.then_some("a=x-ss-video[0].pyrowaveRowOrder:1"), "{format:#x} records={records} row_order={row_order}");
        // Next to the records key, before the pace hint, and nothing else moves.
        if sent {
            let features = sdp.find("pyrowaveFeatures:1").unwrap();
            assert!(features < sdp.find("pyrowaveRowOrder:1").unwrap());
        }
    }
    // With the request off the description is the same as a client that never heard of it.
    let off = StreamConfig { pyrowave_row_order: false, ..Default::default() };
    let on = StreamConfig { pyrowave_row_order: true, ..Default::default() };
    let mut p = params(&off, SUNSHINE, "10.0.0.7", VIDEO_FORMAT_PYROWAVE);
    p.pyrowave_records_enabled = true;
    let without = generate(&p).text;
    let mut p = params(&on, SUNSHINE, "10.0.0.7", VIDEO_FORMAT_PYROWAVE);
    p.pyrowave_records_enabled = true;
    let with = generate(&p).text;
    assert_eq!(with.replace("a=x-ss-video[0].pyrowaveRowOrder:1 \r\n", ""), without);
}

#[test]
fn gamestream_h264_720p_on_gfe_3_22() {
    let config = StreamConfig {
        width: 1280,
        height: 720,
        fps: 60,
        bitrate_kbps: 10000,
        packet_size: 1024,
        color_space: COLORSPACE_REC_601,
        ..Default::default()
    };
    let p = params(&config, [7, 1, 431, 0], "10.0.0.2", VIDEO_FORMAT_H264);
    let sdp = generate(&p);
    assert_eq!(
        sdp.text,
        document(
            14,
            "IPv4 10.0.0.2",
            &[
                "x-nv-video[0].clientViewportWd:1280",
                "x-nv-video[0].clientViewportHt:720",
                "x-nv-video[0].maxFPS:60",
                "x-nv-video[0].packetSize:1024",
                "x-nv-video[0].rateControlMode:4",
                "x-nv-video[0].timeoutLengthMs:7000",
                "x-nv-video[0].framesWithInvalidRefThreshold:0",
                "x-nv-video[0].initialBitrateKbps:8000",
                "x-nv-video[0].initialPeakBitrateKbps:8000",
                "x-nv-vqos[0].bw.minimumBitrateKbps:8000",
                "x-nv-vqos[0].bw.maximumBitrateKbps:8000",
                "x-nv-vqos[0].fec.enable:1",
                "x-nv-vqos[0].videoQualityScoreUpdateTime:5000",
                "x-nv-vqos[0].qosTrafficType:5",
                "x-nv-aqos.qosTrafficType:4",
                "x-nv-general.featureFlags:135",
                "x-nv-general.useReliableUdp:13",
                "x-nv-vqos[0].fec.minRequiredFecPackets:2",
                "x-nv-vqos[0].bllFec.enable:0",
                "x-nv-vqos[0].drc.enable:0",
                "x-nv-general.enableRecoveryMode:0",
                "x-nv-video[0].videoEncoderSlicesPerFrame:1",
                "x-nv-clientSupportHevc:0",
                "x-nv-vqos[0].bitStreamFormat:0",
                "x-nv-video[0].dynamicRangeMode:0",
                "x-nv-video[0].maxNumReferenceFrames:1",
                "x-nv-video[0].clientRefreshRateX100:0",
                "x-nv-audio.surround.numChannels:2",
                "x-nv-audio.surround.channelMask:3",
                "x-nv-audio.surround.enable:0",
                "x-nv-audio.surround.AudioQuality:0",
                "x-nv-aqos.packetDuration:5",
                "x-nv-video[0].encoderCscMode:0",
            ],
            47998
        )
    );
    assert!(!sdp.audio_encryption_enabled);
}

#[test]
fn gamestream_hevc_4k_remote_51_on_gfe_before_3_22() {
    let config = StreamConfig {
        width: 3840,
        height: 2160,
        fps: 60,
        bitrate_kbps: 50000,
        packet_size: 1392,
        location: StreamLocation::Remote,
        audio: AudioConfiguration::SURROUND_51,
        ..Default::default()
    };
    let p = params(&config, [7, 1, 415, 0], "203.0.113.9", VIDEO_FORMAT_H265);
    assert_eq!(
        generate(&p).text,
        document(
            14,
            "IPv4 203.0.113.9",
            &[
                "x-nv-video[0].clientViewportWd:3840",
                "x-nv-video[0].clientViewportHt:2160",
                "x-nv-video[0].maxFPS:60",
                "x-nv-video[0].packetSize:1392",
                "x-nv-video[0].rateControlMode:4",
                "x-nv-video[0].timeoutLengthMs:7000",
                "x-nv-video[0].framesWithInvalidRefThreshold:0",
                "x-nv-video[0].initialBitrateKbps:39500",
                "x-nv-video[0].initialPeakBitrateKbps:39500",
                "x-nv-vqos[0].bw.minimumBitrateKbps:39500",
                "x-nv-vqos[0].bw.maximumBitrateKbps:39500",
                "x-nv-vqos[0].fec.enable:1",
                "x-nv-vqos[0].videoQualityScoreUpdateTime:5000",
                "x-nv-vqos[0].qosTrafficType:0",
                "x-nv-aqos.qosTrafficType:0",
                "x-nv-general.useReliableUdp:1",
                "x-nv-ri.useControlChannel:1",
                "x-nv-vqos[0].fec.repairPercent:5",
                "x-nv-vqos[0].drc.enable:0",
                "x-nv-general.enableRecoveryMode:0",
                "x-nv-video[0].videoEncoderSlicesPerFrame:1",
                "x-nv-clientSupportHevc:1",
                "x-nv-vqos[0].bitStreamFormat:1",
                "x-nv-video[0].dynamicRangeMode:0",
                "x-nv-video[0].maxNumReferenceFrames:1",
                "x-nv-video[0].clientRefreshRateX100:0",
                "x-nv-audio.surround.numChannels:6",
                "x-nv-audio.surround.channelMask:63",
                "x-nv-audio.surround.enable:1",
                "x-nv-audio.surround.AudioQuality:0",
                "x-nv-aqos.packetDuration:5",
                "x-nv-video[0].encoderCscMode:2",
            ],
            47998
        )
    );
}

#[test]
fn gamestream_generation_3_remote() {
    let config = StreamConfig {
        width: 1280,
        height: 720,
        fps: 30,
        bitrate_kbps: 5000,
        packet_size: 1024,
        location: StreamLocation::Remote,
        ..Default::default()
    };
    let mut p = params(&config, [3, 0, 0, 0], "10.1.1.1", VIDEO_FORMAT_H264);
    p.rtsp_client_version = 10;
    assert_eq!(
        generate(&p).text,
        document(
            10,
            "IPv4 10.1.1.1",
            &[
                "x-nv-video[0].clientViewportWd:1280",
                "x-nv-video[0].clientViewportHt:720",
                "x-nv-video[0].maxFPS:30",
                "x-nv-video[0].packetSize:1024",
                "x-nv-video[0].rateControlMode:4",
                "x-nv-video[0].timeoutLengthMs:7000",
                "x-nv-video[0].framesWithInvalidRefThreshold:0",
                "x-nv-video[0].averageBitrate:4",
                "x-nv-video[0].peakBitrate:4",
                "x-nv-vqos[0].bw.minimumBitrate:3500",
                "x-nv-vqos[0].bw.maximumBitrate:3500",
                "x-nv-vqos[0].fec.enable:1",
                "x-nv-vqos[0].videoQualityScoreUpdateTime:5000",
                "x-nv-vqos[0].qosTrafficType:0",
                "x-nv-aqos.qosTrafficType:0",
                "x-nv-general.serverAddress:10.1.1.1",
                "x-nv-general.featureFlags:BwAA",
                "x-nv-video[0].transferProtocol:AQAA",
                "x-nv-video[1].transferProtocol:AQAA",
                "x-nv-video[2].transferProtocol:AQAA",
                "x-nv-video[3].transferProtocol:AQAA",
                "x-nv-video[0].rateControlMode:BAAA",
                "x-nv-video[1].rateControlMode:BQAA",
                "x-nv-video[2].rateControlMode:BQAA",
                "x-nv-video[3].rateControlMode:BQAA",
                "x-nv-vqos[0].bw.flags:14083",
                "x-nv-vqos[0].videoQosMaxConsecutiveDrops:0",
                "x-nv-vqos[1].videoQosMaxConsecutiveDrops:0",
                "x-nv-vqos[2].videoQosMaxConsecutiveDrops:0",
                "x-nv-vqos[3].videoQosMaxConsecutiveDrops:0",
            ],
            47996
        )
    );
}

#[test]
fn generation_4_and_low_resolution_gfe_quirks() {
    let config = StreamConfig {
        width: 640,
        height: 480,
        ..Default::default()
    };
    let mut p = params(&config, [4, 0, 0, 0], "10.1.1.1", VIDEO_FORMAT_H264);
    p.rtsp_client_version = 11;
    let text = generate(&p).text;
    assert!(text.contains("a=x-nv-general.serverAddress:rtsp://10.1.1.1:48010 \r\n"));
    assert!(text.contains("a=x-nv-audio.surround.enable:0 \r\n"));
    assert!(!text.contains("dynamicRangeMode") && !text.contains("packetDuration"));
    assert!(text.ends_with("m=video 47998  \r\n"));

    let text = generate(&params(
        &config,
        [7, 1, 450, 0],
        "10.1.1.1",
        VIDEO_FORMAT_H264,
    ))
    .text;
    assert!(text.contains("a=x-nv-vqos[0].drc.enable:1 \r\na=x-nv-vqos[0].drc.tableType:2 \r\n"));

    let text = generate(&params(
        &config,
        [7, 1, 400, 0],
        "10.1.1.1",
        VIDEO_FORMAT_H265,
    ))
    .text;
    assert!(text.contains(
        "a=x-nv-vqos[0].bitStreamFormat:1 \r\na=x-nv-video[0].encoderFeatureSetting:0 \r\n"
    ));
}

#[test]
fn audio_packet_duration_rules() {
    let duration = |bitrate, audio_capabilities| {
        let config = StreamConfig {
            bitrate_kbps: bitrate,
            audio_capabilities,
            ..Default::default()
        };
        generate(&params(&config, SUNSHINE, "10.0.0.1", VIDEO_FORMAT_H264)).audio_packet_duration_ms
    };
    assert_eq!(
        duration(4000, CAPABILITY_SUPPORTS_ARBITRARY_AUDIO_DURATION),
        10
    );
    assert_eq!(duration(4000, 0), 5);
    assert_eq!(duration(20000, CAPABILITY_SLOW_OPUS_DECODER), 10);
    assert_eq!(
        duration(20000, CAPABILITY_SUPPORTS_ARBITRARY_AUDIO_DURATION),
        5
    );
}

#[test]
fn host_requested_encryption_overrides_the_client_opt_out_only_on_request() {
    let config = StreamConfig {
        encryption_flags: ENCFLG_NONE,
        ..Default::default()
    };
    let mut p = params(&config, SUNSHINE, "10.0.0.1", VIDEO_FORMAT_H264);
    p.encryption_supported = 7;
    p.encryption_requested = 1;
    let sdp = generate(&p);
    assert_eq!(sdp.encryption_enabled, 1);
    assert!(!sdp.audio_encryption_enabled);
    assert!(sdp.text.contains("a=x-nv-general.featureFlags:135 \r\n"));
    // The microphone is encrypted only once its stream exists.
    p.encryption_supported = 15;
    assert_eq!(generate(&p).encryption_enabled, 1);
    p.mic_port = 48001;
    assert_eq!(generate(&p).encryption_enabled, 9);
}
