//! What a client that asks for nothing new sees through the real RTSP server: the DESCRIBE
//! answer of a host with the default configuration, byte for byte.
use super::*;

/// The DESCRIBE answer's body. Only the input feature flags (what this PC's Windows can
/// inject) and the microphone port vary.
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
