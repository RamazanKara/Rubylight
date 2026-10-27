//! What a client that asks for nothing new sees through the real RTSP server: the DESCRIBE
//! answer of a host with the default configuration, byte for byte.
use super::*;

/// The DESCRIBE answer's body. Only the input feature flags (what this PC's Windows can
/// inject) and the microphone port vary. `x-rl-audio.codecs:3` is Opus and PCM: the test
/// binary's folder has no FFmpeg for AC3 and E-AC3.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn describe_for_a_default_config_is_byte_identical() -> Result<()> {
    let h = Harness::new(10_000, 1).await?;
    let launch = h.launch(0, false).await?;
    let describe = h.rtsp(&launch, 10, "DESCRIBE", "").await?;
    let (_, body) = describe.split_once("\r\n\r\n").unwrap();
    let flags = butterpollo_windows::input::capabilities(&h.h.config.read().unwrap());
    let mic = h.ports.mic;
    assert_eq!(
        body,
        format!(
            "a=x-ss-general.featureFlags:{flags}\r\n\
             a=x-ss-general.encryptionSupported:13\r\n\
             a=x-ss-general.encryptionRequested:9\r\n\
             a=x-rl-control:0x5530,0x5531,0x5532,0x5533,0x5534,0x5535,0x5536,0x5537,0x5538\r\n\
             a=x-rl-roaming:1\r\n\
             a=fmtp:97 surround-params=21101\r\n\
             a=fmtp:97 surround-params=642014523\r\n\
             a=fmtp:97 surround-params=85301456723\r\n\
             a=fmtp:97 surround-params=21101\r\n\
             a=fmtp:97 surround-params=660014523\r\n\
             a=fmtp:97 surround-params=88001456723\r\n\
             a=x-rl-audio.codecs:3\r\n\
             a=x-rl-audio.layout714:12,7,5,0,1,10,11,2,3,4,5,6,7,8,9\r\n\
             a=x-rl-audio.layout714Hq:12,12,0,0,1,10,11,2,3,4,5,6,7,8,9\r\n\
             a=x-rl-control:0x5539,0x553d\r\n\
             m=audio {mic} RTP/AVP 96\r\n\
             a=rtpmap:96 opus/48000/1\r\n\
             a=fmtp:96 minptime=10;useinbandfec=1\r\n"
        ),
        "{body}"
    );
    // The default host's 3.0 control messages (FEC report, pipeline rates) are a second line.
    assert_eq!(body.matches("a=x-rl-control:").count(), 2, "{body}");
    assert!(body.contains(butterpollo_core::rtsp::CONTROL_MESSAGES));
    let (session, client) = h.play(&launch).await?;
    h.end(&session, client).await?;
    h.finish().await
}

/// With experimental packed 4:4:4 on and every packed mode probed, the packed lines
/// come before the microphone's, and nothing else changes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn describe_with_packed_444_and_the_microphone_is_byte_identical() -> Result<()> {
    let h = Harness::configured(10_000, 1, "experimental_packed_444=true\n").await?;
    h.h.packed_modes.store(
        butterpollo_core::packed444::ALL_MODES,
        std::sync::atomic::Ordering::Release,
    );
    let launch = h.launch(0, false).await?;
    let describe = h.rtsp(&launch, 10, "DESCRIBE", "").await?;
    let (_, body) = describe.split_once("\r\n\r\n").unwrap();
    let flags = butterpollo_windows::input::capabilities(&h.h.config.read().unwrap());
    let mic = h.ports.mic;
    assert_eq!(
        body,
        format!(
            "a=x-ss-general.featureFlags:{flags}\r\n\
             a=x-ss-general.encryptionSupported:13\r\n\
             a=x-ss-general.encryptionRequested:9\r\n\
             a=x-rl-control:0x5530,0x5531,0x5532,0x5533,0x5534,0x5535,0x5536,0x5537,0x5538\r\n\
             a=x-rl-roaming:1\r\n\
             a=fmtp:97 surround-params=21101\r\n\
             a=fmtp:97 surround-params=642014523\r\n\
             a=fmtp:97 surround-params=85301456723\r\n\
             a=fmtp:97 surround-params=21101\r\n\
             a=fmtp:97 surround-params=660014523\r\n\
             a=fmtp:97 surround-params=88001456723\r\n\
             a=x-rl-video.packed444Version:1\r\n\
             a=x-rl-video.packed444Modes:15\r\n\
             a=x-rl-video.packed444LayeredModes:10\r\n\
             a=x-rl-audio.codecs:3\r\n\
             a=x-rl-audio.layout714:12,7,5,0,1,10,11,2,3,4,5,6,7,8,9\r\n\
             a=x-rl-audio.layout714Hq:12,12,0,0,1,10,11,2,3,4,5,6,7,8,9\r\n\
             a=x-rl-control:0x5539,0x553d\r\n\
             m=audio {mic} RTP/AVP 96\r\n\
             a=rtpmap:96 opus/48000/1\r\n\
             a=fmtp:96 minptime=10;useinbandfec=1\r\n"
        ),
        "{body}"
    );
    let (session, client) = h.play(&launch).await?;
    h.end(&session, client).await?;
    h.finish().await
}

/// An ANNOUNCE that picks a 3.0 extension the host did not offer is refused,
/// naming it, and the stock ANNOUNCE that follows streams as before.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn announce_refuses_3_0_choices_that_were_not_offered() -> Result<()> {
    let h = Harness::configured(
        10_000,
        1,
        "audio_passthrough=false\naudio_surround_714=false\n",
    )
    .await?;
    let launch = h.launch(0, false).await?;
    let stock = "a=x-nv-video[0].clientViewportWd:640
a=x-nv-video[0].clientViewportHt:480
";
    // Sequences 1 to 3 stay free for the play below.
    for (sequence, sdp, message) in [
        (
            20,
            "a=x-nv-video[0].dynamicRangeMode:1
a=x-nv-vqos[0].bitStreamFormat:1
a=x-rl-video[0].hdrFormat:1
",
            "x-rl-video[0].hdrFormat 1 was not offered",
        ),
        (
            21,
            "a=x-rl-audio.codec:2
",
            "x-rl-audio.codec 2 was not offered",
        ),
        (
            22,
            "a=x-nv-audio.surround.numChannels:12
a=x-nv-audio.surround.channelMask:185919
",
            "7.1.4",
        ),
    ] {
        let response = h
            .rtsp(&launch, sequence, "ANNOUNCE", &format!("{stock}{sdp}"))
            .await
            .unwrap_err()
            .to_string();
        assert!(response.starts_with("RTSP/1.0 400"), "{response}");
        assert!(response.contains(message), "{response}");
    }
    let (session, client) = h.play(&launch).await?;
    assert_eq!(
        (session.config.hdr_format, session.config.audio_codec),
        (0, 0)
    );
    h.end(&session, client).await?;
    h.finish().await
}

/// A default host offers 7.1.4, so a client that asks for 12 channels streams with them.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_default_host_streams_the_7_1_4_a_client_asks_for() -> Result<()> {
    let h = Harness::new(10_000, 1).await?;
    let launch = h.launch(0, false).await?;
    let (session, client) = h
        .play_announcing(
            &launch,
            0,
            "a=x-nv-audio.surround.numChannels:12\r\na=x-nv-audio.surround.channelMask:185919\r\n",
        )
        .await?;
    assert_eq!(session.config.audio_channels, 12);
    h.end(&session, client).await?;
    h.finish().await
}

/// A default host offers PCM, so a client that picks it streams with it. Without FFmpeg beside
/// the host (the test binary's folder has none) Dolby Digital is not offered, and picking it
/// anyway is refused, never streamed as something else.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_default_host_streams_the_pcm_a_client_picks_and_refuses_dolby_without_ffmpeg()
-> Result<()> {
    let h = Harness::new(10_000, 1).await?;
    let launch = h.launch(0, false).await?;
    let stock = "a=x-nv-video[0].clientViewportWd:640\r\na=x-nv-video[0].clientViewportHt:480\r\n";
    for (sequence, sdp) in [
        (20, "a=x-rl-audio.codec:4\r\n"),
        (
            21,
            "a=x-nv-audio.surround.numChannels:6\r\na=x-nv-audio.surround.channelMask:63\r\na=x-rl-audio.codec:8\r\n",
        ),
    ] {
        let response = h
            .rtsp(&launch, sequence, "ANNOUNCE", &format!("{stock}{sdp}"))
            .await
            .unwrap_err()
            .to_string();
        assert!(response.starts_with("RTSP/1.0 400"), "{response}");
        assert!(response.contains("was not offered"), "{response}");
    }
    let (session, client) = h
        .play_announcing(&launch, 0, "a=x-rl-audio.codec:2\r\n")
        .await?;
    assert_eq!(
        (session.config.audio_codec, session.config.audio_channels),
        (2, 2)
    );
    h.end(&session, client).await?;
    h.finish().await
}
