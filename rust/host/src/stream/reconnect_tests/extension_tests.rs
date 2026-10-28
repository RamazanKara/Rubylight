//! What a client that asks for nothing new sees through the real RTSP server: the DESCRIBE
//! answer of a host with the default configuration, byte for byte. Of the 3.0 extensions
//! only lines stock clients ignore are added.
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
             a=x-rl-video.hdrFormats:15\r\n\
             a=x-rl-audio.codecs:3\r\n\
             a=x-rl-audio.layout714:12,7,5,0,1,10,11,2,3,4,5,6,7,8,9\r\n\
             a=x-rl-audio.layout714Hq:12,12,0,0,1,10,11,2,3,4,5,6,7,8,9\r\n\
             a=x-ss-general.penBarrelRoll:1\r\n\
             a=x-rl-control:0x5539,0x553b,0x553d,0x553e\r\n\
             m=audio {mic} RTP/AVP 96\r\n\
             a=rtpmap:96 opus/48000/1\r\n\
             a=fmtp:96 minptime=10;useinbandfec=1\r\n"
        ),
        "{body}"
    );
    // The default host's 3.0 control messages (FEC report, text input, pipeline rates and
    // keyboard layout) are a second line, which stock clients ignore.
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
             a=x-rl-video.hdrFormats:15\r\n\
             a=x-rl-audio.codecs:3\r\n\
             a=x-rl-audio.layout714:12,7,5,0,1,10,11,2,3,4,5,6,7,8,9\r\n\
             a=x-rl-audio.layout714Hq:12,12,0,0,1,10,11,2,3,4,5,6,7,8,9\r\n\
             a=x-ss-general.penBarrelRoll:1\r\n\
             a=x-rl-control:0x5539,0x553b,0x553d,0x553e\r\n\
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

/// Full chroma saved while the host runs is offered in DESCRIBE once its probe has run,
/// without a restart.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn describe_offers_packed_444_saved_while_the_host_runs() -> Result<()> {
    fn probe(_: &Config, _: &butterpollo_core::rtsp::Negotiated) -> Result<()> {
        Ok(())
    }
    let h = Harness::new(10_000, 1).await?;
    h.h.probing_codecs.store(false, Ordering::Release);
    let before = h.h.config.read().unwrap().clone();
    h.h.config
        .write()
        .unwrap()
        .values
        .insert("experimental_packed_444".into(), "true".into());
    h.h.packed_settings_saved_with(&before, probe);
    until("the saved packed modes are probed", WAIT, || {
        h.h.packed_modes.load(Ordering::Acquire) == butterpollo_core::packed444::ALL_MODES
    })
    .await;
    let launch = h.launch(0, false).await?;
    let describe = h.rtsp(&launch, 10, "DESCRIBE", "").await?;
    let (_, body) = describe.split_once("\r\n\r\n").unwrap();
    assert!(
        body.contains("a=x-rl-video.packed444Modes:15\r\n"),
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
    // HLG is offered by default; with it off, hdrFormat 2 was not.
    let h = Harness::configured(
        10_000,
        1,
        "audio_passthrough=false\naudio_surround_714=false\nhdr_hlg=false\n",
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
a=x-rl-video[0].hdrFormat:2
",
            "x-rl-video[0].hdrFormat 2 was not offered",
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

/// A host whose PyroWave probe found Main10 adds the PyroWave HLG offer flag (128) to its
/// formats, and a client that then asks for HLG on PyroWave streams it and gets the HDR mode
/// message (0x010e) as on HEVC. Without PyroWave Main10 the same ANNOUNCE is refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hlg_on_pyrowave_is_offered_and_sends_the_hdr_mode_message() -> Result<()> {
    let h = Harness::new(10_000, 1).await?;
    let sdp = "a=x-nv-vqos[0].bitStreamFormat:3\r\na=x-nv-video[0].dynamicRangeMode:1\r\na=x-rl-video[0].hdrFormat:2\r\n";
    let launch = h.launch_hdr(0, false, None).await?;
    let describe = h.rtsp(&launch, 10, "DESCRIBE", "").await?;
    assert!(
        describe.contains("a=x-rl-video.hdrFormats:15\r\n"),
        "{describe}"
    );
    let response = h
        .rtsp(&launch, 11, "ANNOUNCE", &format!("{VIEWPORT}{sdp}"))
        .await
        .unwrap_err()
        .to_string();
    assert!(response.starts_with("RTSP/1.0 400"), "{response}");
    assert!(response.contains("(HLG) needs HEVC or AV1"), "{response}");
    h.h.codecs
        .store(0x1 | 0x100 | 0x200 | 0x0200_0000, Ordering::Release);
    let describe = h.rtsp(&launch, 12, "DESCRIBE", "").await?;
    assert!(
        describe.contains("a=x-rl-video.hdrFormats:143\r\n"),
        "{describe}"
    );
    let (session, client) = h.connect_announcing(&launch, 0, sdp).await?;
    assert_eq!((session.config.codec, session.config.hdr_format), (3, 2));
    assert!(session.config.hlg());
    until("HDR mode message", WAIT, || {
        !client.hdr_modes.lock().unwrap().is_empty()
    })
    .await;
    assert_eq!(client.hdr_modes.lock().unwrap()[0].1[0], 1);
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

/// A DESCRIBE or ANNOUNCE whose passthrough probe can't run, because FFmpeg's folder is unknown
/// or the probe panicked, is answered with a 500 saying why instead of a dropped connection, and
/// the launch streams once the probe runs again. Without passthrough no probe is needed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_passthrough_probe_that_cannot_run_is_answered_with_an_error() -> Result<()> {
    let h = Harness::new(10_000, 1).await?;
    let launch = h.launch(0, false).await?;
    let stock = "a=x-nv-video[0].clientViewportWd:640\r\na=x-nv-video[0].clientViewportHt:480\r\n";
    let mut sequence = 20;
    for (flag, message) in [
        (
            &h.fixture.ffmpeg_directory_unknown,
            "the test host's folder is unknown",
        ),
        (
            &h.fixture.ffmpeg_probe_panics,
            "FFmpeg's passthrough audio probe did not finish",
        ),
    ] {
        flag.store(true, Ordering::Release);
        for (method, body) in [("DESCRIBE", ""), ("ANNOUNCE", stock)] {
            sequence += 1;
            let response = h
                .rtsp(&launch, sequence, method, body)
                .await
                .unwrap_err()
                .to_string();
            assert!(
                response.starts_with("RTSP/1.0 500 Internal Server Error"),
                "{method}: {response}"
            );
            assert!(response.contains(message), "{method}: {response}");
        }
        flag.store(false, Ordering::Release);
    }
    let (session, client) = h.play(&launch).await?;
    h.end(&session, client).await?;
    h.finish().await?;

    let h = Harness::configured(10_000, 1, "audio_passthrough=false\n").await?;
    h.fixture
        .ffmpeg_directory_unknown
        .store(true, Ordering::Release);
    let launch = h.launch(0, false).await?;
    let (session, client) = h.play(&launch).await?;
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
