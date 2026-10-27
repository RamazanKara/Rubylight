//! Nightjar against this host: the client engine (`nightjar-core`, `client/protocol/client`)
//! pairs, reads serverinfo and the native API, launches, streams, roams, transfers files and
//! asks for sleep through the real HTTP, HTTPS, RTSP, ENet and session workers on loopback.
//! Only the reconnect fixture's capture, encoder and native input are fake.
//!
//! Nightjar builds with its own ENet fork (`client/protocol/vendor/rusty_enet`) beside the
//! host's rusty_enet, as it ships. Its native API types are this repository's
//! (`rust/core/native-api`, patched in the workspace's Cargo.toml), not the rev Nightjar pins
//! (17fc45bb, v1.1.0 without the file transfer error codes): built against that rev, Nightjar
//! reads `too_large` and the other file refusals as an unknown error.
//!
//! Nightjar's calls block. The test body runs on the runtime's own thread, not on a worker, so
//! they don't hold up the host's tasks. Waits are long ([`LOADED`]): these tests run beside the
//! whole suite on a busy machine.
use super::*;
use butterpollo_core::{
    native::{FILE_DOWNLOAD, FILE_UPLOAD},
    native_api::feature,
    power_action::SleepStates,
};
use nightjar_core::{
    control::{ControlEvent, ptype},
    http::{
        Availability, ClientIdentity, Family, Feature, FileError, HostClient, LaunchRequest,
        PairResult, PowerError, Reason, Refusal, ServerInfo, TransferCancel,
    },
    roaming::RoamEvent,
    sdp::{StreamConfig, VIDEO_FORMAT_H264},
    session::{
        AudioRenderer, AudioSetup, DecodeStatus, HostInfo, Interrupt, Listener, Location,
        Session as Stream, SessionConfig, VideoDecoder, VideoSetup,
    },
    video::Frame,
};

/// How long a step may take on a loaded machine.
const LOADED: Duration = Duration::from_secs(30);
/// The remote input key and its ID Nightjar launches with.
const KEY: [u8; 16] = *b"nightjar-interop";
const KEY_ID: i32 = 7;
/// Where the client roams to.
const MOVED: [u8; 4] = [127, 0, 0, 2];

/// Nightjar as a device this host paired, with `perm` and the console's `extra` settings.
fn device(h: &Harness, perm: u32, extra: &[(&str, bool)]) -> Result<HostClient> {
    let identity = Arc::new(ClientIdentity::generate()?);
    h.h.paired.write().unwrap().clients.push(PairedClient {
        name: "Nightjar".into(),
        uuid: "nightjar".into(),
        cert: identity.certificate_pem().into(),
        perm,
        enabled: true,
        extra: extra
            .iter()
            .map(|(key, value)| ((*key).into(), (*value).into()))
            .collect(),
    });
    Ok(HostClient::new("127.0.0.1", h.ports.http, identity)?
        .with_https_port(h.ports.https)
        .with_server_certificate_pem(&h.h.identity.certificate)?)
}

/// The paired Nightjar device's entry in the host's client list.
fn paired<R>(h: &Harness, change: impl FnOnce(&mut PairedClient) -> R) -> R {
    let mut paired = h.h.paired.write().unwrap();
    change(
        paired
            .clients
            .iter_mut()
            .find(|c| c.uuid == "nightjar")
            .expect("the Nightjar device is paired"),
    )
}

/// The only launch waiting for its RTSP handshake.
fn pending(h: &Harness) -> Launch {
    let sessions = h.h.sessions.lock().unwrap();
    assert_eq!(sessions.pending.len(), 1, "one launch is pending");
    sessions.pending.values().next().unwrap().clone()
}

/// What Nightjar's decoder, renderer and listener saw.
#[derive(Default)]
struct Seen {
    setup: Option<VideoSetup>,
    frames: usize,
    control_messages: Option<Vec<u16>>,
    events: Vec<ControlEvent>,
    terminated: Option<i32>,
}
#[derive(Clone, Default)]
struct Recorder(Arc<Mutex<Seen>>);
impl Recorder {
    fn seen<R>(&self, read: impl FnOnce(&Seen) -> R) -> R {
        read(&self.0.lock().unwrap())
    }
    fn frames(&self) -> usize {
        self.seen(|s| s.frames)
    }
    fn saw(&self, event: impl Fn(&ControlEvent) -> bool) -> bool {
        self.seen(|s| s.events.iter().any(event))
    }
}
impl VideoDecoder for Recorder {
    fn setup(&mut self, setup: &VideoSetup) -> Result<(), i32> {
        self.0.lock().unwrap().setup = Some(*setup);
        Ok(())
    }
    fn submit(&mut self, _: &Frame) -> DecodeStatus {
        self.0.lock().unwrap().frames += 1;
        DecodeStatus::Ok
    }
}
impl AudioRenderer for Recorder {
    fn init(&mut self, _: &AudioSetup) -> Result<(), i32> {
        Ok(())
    }
    fn play(&mut self, _: Option<&[u8]>) {}
}
impl Listener for Recorder {
    fn host_control_messages(&self, messages: &[u16]) {
        self.0.lock().unwrap().control_messages = Some(messages.to_vec());
    }
    fn connection_terminated(&self, error: i32) {
        self.0.lock().unwrap().terminated = Some(error);
    }
    fn control_event(&self, event: ControlEvent) {
        self.0.lock().unwrap().events.push(event);
    }
}

/// Launches the fixture's desktop as Nightjar does, checks what reached the host and streams
/// it: the RTSP handshake, ENet control and video, until frames arrive.
async fn stream(
    h: &Harness,
    client: &HostClient,
) -> Result<(Launch, Arc<Session>, Stream, Recorder)> {
    let info = client.server_info(true)?;
    assert!(info.paired);
    let apps = client.app_list(&info.capabilities)?;
    let desktop = h.h.apps.read().unwrap()[0].clone();
    let app = apps
        .iter()
        .find(|app| app.id as u32 == desktop.id())
        .expect("the desktop is in Nightjar's app list");
    let request = LaunchRequest {
        app_id: app.id,
        app_uuid: app.uuid.clone(),
        width: 640,
        height: 480,
        fps: 60,
        ri_key: KEY,
        ri_key_id: KEY_ID,
        ..Default::default()
    };
    let launched = client.launch(&request, &info)?.expect("a session URL");
    assert_eq!(
        launched.rtsp_session_url,
        Some(format!("rtspenc://127.0.0.1:{}", h.ports.rtsp))
    );
    let launch = pending(h);
    assert_eq!(launch.client.uuid, "nightjar");
    assert_eq!(launch.app_id, desktop.id());
    assert_eq!((launch.key, launch.key_id), (KEY, KEY_ID as u32));
    assert!(launch.rtsp_encrypted, "corever=1 asks for encrypted RTSP");

    let recorder = Recorder::default();
    let mut iv = [0; 16];
    iv[..4].copy_from_slice(&KEY_ID.to_be_bytes());
    let stream = Stream::start(
        &HostInfo {
            address: "127.0.0.1".into(),
            app_version: info.app_version.clone().expect("appversion"),
            rtsp_session_url: launched.rtsp_session_url,
            server_codec_mode_support: info.server_codec_mode_support,
        },
        SessionConfig {
            stream: StreamConfig {
                width: 640,
                height: 480,
                fps: 60,
                // The fixture's packetizer sends 1024-byte packets.
                packet_size: 1024,
                remote_input_aes_key: KEY,
                remote_input_aes_iv: iv,
                supported_video_formats: VIDEO_FORMAT_H264,
                ..Default::default()
            },
            location: Location::Auto,
            ..Default::default()
        },
        Box::new(recorder.clone()),
        Box::new(recorder.clone()),
        Arc::new(recorder.clone()),
        &Interrupt::default(),
    )
    .map_err(|e| anyhow::anyhow!("Nightjar's stream did not start: {e:?}"))?;
    let session = h.h.sessions.lock().unwrap().active[&launch.id].clone();
    until("video at Nightjar's decoder", LOADED, || {
        recorder.frames() > 2
    })
    .await;
    assert!(!session.stopping());
    Ok((launch, session, stream, recorder))
}

/// The host's control extensions Nightjar must know, in the order the host announces them.
const CONTROL_EXTENSIONS: [u16; 10] = [
    ptype::PHASE_LOCK_REPORT,
    ptype::DISPLAY_CAPS,
    ptype::RECONFIGURE,
    ptype::CONTROLLER_SPEAKER,
    ptype::CONTROLLER_MICROPHONE,
    ptype::CONTROLLER_AUDIO_CONTROL,
    ptype::HOST_TIMING,
    ptype::CLOCK_SYNC,
    ptype::LINK_NOTICE,
    ptype::PIPELINE_RATES,
];

/// Launch, the RTSP handshake and the control stream both ways: Nightjar's keyframe request
/// and clock sync probe reach the host, and the host's HDR mode and clock sync answer reach
/// Nightjar. Stopping the stream ends the host's session.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nightjar_launches_and_streams_with_control_both_ways() -> Result<()> {
    let h = Harness::new(10_000, 1).await?;
    let client = device(&h, u32::MAX, &[])?;
    let (launch, session, stream, recorder) = stream(&h, &client).await?;
    h.counts(1, 0).await?;

    let outcome = stream.outcome();
    assert_eq!(outcome.negotiated_video_format, VIDEO_FORMAT_H264);
    assert_eq!(outcome.host_control_messages, CONTROL_EXTENSIONS);
    assert!(
        outcome.host_roaming,
        "the host offers roaming in its DESCRIBE"
    );
    assert_eq!(
        recorder.seen(|s| s.control_messages.clone()),
        Some(CONTROL_EXTENSIONS.to_vec())
    );
    let setup = recorder.seen(|s| s.setup).expect("the decoder was set up");
    assert_eq!((setup.width, setup.height), (640, 480));

    // Nightjar to the host: a keyframe request and a clock sync probe.
    let requests = session.stats.idr_requests.load(Ordering::Acquire);
    stream.request_idr_frame();
    until("Nightjar's keyframe request at the host", LOADED, || {
        session.stats.idr_requests.load(Ordering::Acquire) > requests
    })
    .await;
    stream.send_clock_sync_probe(nightjar_core::session::now_us())?;
    until("the host's clock sync answer", LOADED, || {
        recorder.saw(|e| matches!(e, ControlEvent::ClockSync { .. }))
    })
    .await;
    // The host to Nightjar: the fixture's display reports HDR metadata.
    until("the host's HDR mode", LOADED, || {
        recorder.saw(|e| matches!(e, ControlEvent::HdrMode { .. }))
    })
    .await;

    stream.stop();
    until("the host's session ended", LOADED, || session.stopping()).await;
    until("sessions released", LOADED, || {
        h.h.sessions.lock().unwrap().idle()
    })
    .await;
    assert!(!h.h.sessions.lock().unwrap().active.contains_key(&launch.id));
    assert_eq!(recorder.seen(|s| s.terminated), None);
    h.finish().await
}

/// Nightjar roams to 127.0.0.2: PROBE, CONFIRM and MIGRATED against the host's roaming table
/// move control and video, and the host's termination reaches Nightjar at the new address.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nightjar_roams_to_a_new_address_and_keeps_its_stream() -> Result<()> {
    let h = Harness::new(10_000, 1).await?;
    let client = device(&h, u32::MAX, &[])?;
    let (launch, session, stream, recorder) = stream(&h, &client).await?;
    assert_eq!(session.client_ip(), IpAddr::from([127, 0, 0, 1]));
    assert_eq!(session.roams(), 0);

    stream.roam_to(Some(IpAddr::from(MOVED)))?;
    until("Nightjar roamed", LOADED, || {
        recorder.saw(|e| matches!(e, ControlEvent::Roam(RoamEvent::Roamed { .. })))
    })
    .await;
    assert_eq!(session.roams(), 1);
    assert_eq!(session.client_ip(), IpAddr::from(MOVED));
    // The roam drops the video peer until Nightjar's first ping on the new path.
    let video = (launch.id.clone(), false);
    until("video to the new address", LOADED, || {
        h.media
            .as_ref()
            .unwrap()
            .peers
            .lock()
            .unwrap()
            .get(&video)
            .is_some_and(|peer| peer.ip() == IpAddr::from(MOVED))
    })
    .await;
    let frames = recorder.frames();
    until("video at Nightjar after the roam", LOADED, || {
        recorder.frames() > frames + 2
    })
    .await;

    // TEARDOWN as the host's console ends a stream; the sequence is past Nightjar's own.
    h.rtsp(&launch, 1000, "TEARDOWN", "").await?;
    until("the termination at Nightjar", LOADED, || {
        recorder.seen(|s| s.terminated.is_some())
    })
    .await;
    assert_eq!(session.termination_reason(), CLOSED);
    // ML_ERROR_GRACEFUL_TERMINATION.
    assert_eq!(recorder.seen(|s| s.terminated), Some(0));
    stream.stop();
    h.finish().await
}

/// Nightjar pairs with the PIN the console enters, then reads serverinfo as a paired device
/// over HTTPS pinned to the certificate it got. A wrong PIN is caught on both ends.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nightjar_pairs_with_the_pin_entered_on_the_host() -> Result<()> {
    let h = Harness::new(10_000, 1).await?;
    let identity = Arc::new(ClientIdentity::generate()?);
    let client = Arc::new(
        HostClient::new("127.0.0.1", h.ports.http, identity.clone())?
            .with_device_name("Nightjar interop"),
    );
    let info = client.server_info(true)?;
    assert!(!info.paired);
    assert_eq!(info.https_port, h.ports.https);
    assert_eq!(info.capabilities.family(), Family::Rubylight);

    // The console enters another PIN first.
    for (typed, entered) in [("1111", "4821"), ("4821", "4821")] {
        let pairing = tokio::task::spawn_blocking({
            let (client, info) = (client.clone(), info.clone());
            move || client.pair(&info, typed)
        });
        let deadline = Instant::now() + LOADED;
        let pin = loop {
            let waiting = {
                let mut pins = h.h.pins.lock().unwrap();
                let id = pins.keys().next().cloned();
                id.and_then(|id| pins.remove(&id))
            };
            if let Some(pin) = waiting {
                break pin;
            }
            assert!(
                Instant::now() < deadline,
                "timed out: Nightjar's pairing request"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        };
        assert_eq!(pin.name, "Nightjar interop");
        assert_eq!(pin.certificate.trim(), identity.certificate_pem().trim());
        pin.sender.send((entered.into(), pin.name.clone())).unwrap();
        let result = pairing.await??;
        if typed != entered {
            assert_eq!(result, PairResult::PinWrong);
            assert!(!client.server_info(true)?.paired);
            continue;
        }
        let PairResult::Paired {
            server_certificate_pem,
        } = result
        else {
            panic!("{result:?}");
        };
        assert_eq!(server_certificate_pem, h.h.identity.certificate);
    }
    let perm =
        h.h.paired
            .read()
            .unwrap()
            .clients
            .iter()
            .find(|c| c.name == "Nightjar interop")
            .expect("the host stored the device")
            .perm;
    let info = client.server_info(true)?;
    assert!(info.paired);
    assert_eq!(info.permission, Some(perm));
    // A client restored from storage needs no pairing.
    let stored = HostClient::new("127.0.0.1", h.ports.http, identity)?
        .with_https_port(h.ports.https)
        .with_server_certificate_pem(&client.server_certificate_pem().unwrap())?;
    assert!(stored.server_info_https()?.paired);
    Ok(())
}

/// How Nightjar reads each field of the host's serverinfo. Every field the host sends is
/// listed: a new one needs a decision here, and Nightjar's reading of it.
enum Reading {
    /// A `ServerInfo` field or the evidence for a [`Feature`].
    Read,
    /// Nightjar has no use for it; the reason says why.
    Ignored(&'static str),
}
const SERVERINFO_FIELDS: &[(&str, Reading)] = &[
    ("hostname", Reading::Read),
    ("appversion", Reading::Read),
    ("GfeVersion", Reading::Read),
    ("uniqueid", Reading::Read),
    ("HttpsPort", Reading::Read),
    ("ExternalPort", Reading::Read),
    ("mac", Reading::Read),
    ("LocalIP", Reading::Read),
    ("PairStatus", Reading::Read),
    ("currentgame", Reading::Read),
    ("currentgameuuid", Reading::Read),
    ("state", Reading::Read),
    ("MaxLumaPixelsHEVC", Reading::Read),
    ("ServerCodecModeSupport", Reading::Read),
    ("Permission", Reading::Read),
    ("ServerCommand", Reading::Read),
    ("RustHostVersion", Reading::Read),
    ("RustHostEncoderBitrateKbps", Reading::Read),
    ("RustHostInitialEncoderBitrateKbps", Reading::Read),
    ("NetworkPathVersion", Reading::Read),
    ("HardwareMetricsVersion", Reading::Read),
    ("NativeApiVersion", Reading::Read),
    ("PowerActionVersion", Reading::Read),
    ("SleepSupported", Reading::Read),
    ("SleepAllowed", Reading::Read),
    ("ControllerPreparation", Reading::Read),
    ("VirtualDisplayCapable", Reading::Read),
    ("VirtualDisplayDriverReady", Reading::Read),
    ("VirtualDisplayHDRCapable", Reading::Read),
    ("FrameLimiterSupported", Reading::Read),
    ("FrameLimiterEnabled", Reading::Read),
    ("VirtualDisplayFrameLimiterEnabled", Reading::Read),
    ("FrameLimiterFpsLimitMilliHz", Reading::Read),
    ("PyroWaveHostLinkMbps", Reading::Read),
    ("PyroWaveBandwidthProbeBytes", Reading::Read),
    ("PyroWaveUdpProbeVersion", Reading::Read),
    ("PyroWaveUdpHandshakeVersion", Reading::Read),
    ("PyroWaveUdpProbeBurstVersion", Reading::Read),
    ("PyroWaveWireBudgetVersion", Reading::Read),
    ("PyroWaveCriticalFecPercentage", Reading::Read),
    ("PyroWaveMinParityShards", Reading::Read),
    ("AudioCodecs", Reading::Read),
    ("AudioLayouts", Reading::Read),
    ("PipelineRatesVersion", Reading::Read),
    (
        "RustHostSessionCount",
        Reading::Ignored("diagnostics for scripts and the release harness"),
    ),
    (
        "RustHostPendingSessionCount",
        Reading::Ignored("diagnostics for scripts and the release harness"),
    ),
    (
        "RustHostApplicationActive",
        Reading::Ignored("diagnostics; Nightjar reads state and currentgame"),
    ),
    (
        "RustHostProfile",
        Reading::Ignored("the build profile, for bug reports"),
    ),
];

/// The serverinfo fields in `raw`, once each.
fn fields(raw: &str) -> Result<Vec<String>> {
    let root = xmltree::Element::parse(raw.as_bytes())?;
    let mut names: Vec<String> = root
        .children
        .iter()
        .filter_map(|n| n.as_element().map(|e| e.name.clone()))
        .collect();
    names.dedup();
    Ok(names)
}

/// `raw` with the field `name` removed (`None`) or set to `value`.
fn edited(raw: &str, name: &str, value: Option<&str>) -> Result<String> {
    let mut root = xmltree::Element::parse(raw.as_bytes())?;
    match value {
        None => root
            .children
            .retain(|n| n.as_element().is_none_or(|e| e.name != name)),
        Some(value) => {
            for field in root.children.iter_mut().filter_map(|n| n.as_mut_element()) {
                if field.name == name {
                    field.children = vec![xmltree::XMLNode::Text(value.into())];
                }
            }
        }
    }
    let mut text = vec![];
    root.write(&mut text)?;
    Ok(String::from_utf8(text)?)
}

/// What Nightjar makes of the serverinfo reply `raw`, a refusal included.
fn reading(raw: &str, http_port: u16) -> String {
    match ServerInfo::parse(raw, http_port) {
        Ok(mut info) => {
            info.raw.clear();
            format!("{info:?}")
        }
        Err(refused) => format!("refused: {refused:?}"),
    }
}

/// Whether Nightjar's reading of `raw` changes when the field `name` goes missing or takes
/// another value: a field Nightjar reads but that holds its default shows under one of them.
/// Each is tried on the reply as sent and as a busy host's, which is when `currentgame` counts.
fn reads(raw: &str, name: &str, http_port: u16) -> Result<bool> {
    for raw in [
        raw.to_owned(),
        edited(raw, "state", Some("SUNSHINE_SERVER_BUSY"))?,
    ] {
        // Written back unedited, like the edits, so only the edit differs.
        let full = reading(&edited(&raw, "", None)?, http_port);
        for value in [None, Some("0"), Some("1"), Some("7"), Some("x")] {
            if reading(&edited(&raw, name, value)?, http_port) != full {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// Every Feature Nightjar knows, as this host offers it to a paired device with every
/// permission and Sleep PC allowed, on the default settings (no virtual display driver, no
/// commands, no hardware metrics, file transfer off) and the fixture's H.264 and HEVC
/// encoders. An exhaustive match: a feature Nightjar adds fails to build here until this
/// host's answer is decided.
fn expected(feature: Feature) -> Availability {
    use Availability::*;
    match feature {
        Feature::Permissions
        | Feature::OtpPairing
        | Feature::Clipboard
        | Feature::AppUuids
        | Feature::LaunchByUuid
        | Feature::FractionalRefresh
        | Feature::HostVrr
        | Feature::RuntimeBitrate
        | Feature::PyroWaveBandwidthProbe
        | Feature::NetworkPath
        | Feature::FrameLimiterStatus
        | Feature::ControllerPreparation
        | Feature::RemoteSessionTiles
        | Feature::HostConfirmsReplacement
        | Feature::HostTiming
        | Feature::PipelineRates
        | Feature::WebUi
        | Feature::NativeApi
        | Feature::Sleep
        // 7.1.4 and PCM audio are on by default.
        | Feature::Surround714
        | Feature::AudioPassthrough => Supported,
        Feature::ServerCommands => Unsupported(Some(Reason::NoCommands)),
        // Display modes are the virtual display's. Before Windows 11 (CI's Server 2022)
        // the default mode is disabled, so the host says it is off, not missing a driver.
        Feature::VirtualDisplay | Feature::VirtualDisplayHdr | Feature::HostDisplayModes => {
            if butterpollo_windows::display::virtual_display_os() {
                Unsupported(Some(Reason::VirtualDisplayDriver))
            } else {
                Unsupported(Some(Reason::VirtualDisplayOff))
            }
        }
        Feature::HardwareMetrics => Unsupported(Some(Reason::HardwareMetricsOff)),
        // No PyroWave encoder: ServerCodecModeSupport has no PyroWave profile.
        Feature::PyroWave | Feature::PyroWaveRecords => Unsupported(None),
        // The host has no cursor shape channel: it sends no CursorShapeVersion.
        Feature::CursorShape => Unsupported(None),
        // File transfer is off; listed by the native API only when on.
        Feature::FileUpload | Feature::FileDownload => Unsupported(None),
        // The 3.0 extensions are defined but offered by nothing yet.
        Feature::DolbyVision
        | Feature::Hlg
        | Feature::ClientFec
        | Feature::BandwidthProbe
        | Feature::UsbForwarding
        | Feature::TextInput
        | Feature::ClientKeyboardLayout
        | Feature::PenBarrelRoll
        | Feature::ImageClipboard
        | Feature::SharedFolders => Unsupported(None),
    }
}

/// Serverinfo and the native API's capabilities as Nightjar reads them: every field the host
/// sends is one Nightjar reads or knowingly ignores, and every Feature Nightjar knows has this
/// host's answer.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nightjar_knows_every_capability_the_host_advertises() -> Result<()> {
    let h = Harness::new(10_000, 1).await?;
    let suspends = Arc::new(Suspends::default());
    h.h.power.set_backend(suspends.clone());
    let client = device(&h, u32::MAX, &[("allow_sleep", true)])?;
    let unpaired = client.server_info_http(true)?;
    let info = client.server_info_https()?;
    assert!(!unpaired.paired && info.paired);
    assert_eq!(unpaired.capabilities.family(), Family::Rubylight);
    assert_eq!(info.capabilities.family(), Family::Rubylight);
    assert_eq!(
        info.capabilities.host_version(),
        Some(env!("CARGO_PKG_VERSION"))
    );

    let mut sent = fields(&unpaired.raw)?;
    sent.extend(fields(&info.raw)?);
    sent.sort();
    sent.dedup();
    for name in &sent {
        let known = SERVERINFO_FIELDS.iter().find(|(field, _)| field == name);
        let Some((_, reading_of)) = known else {
            panic!("serverinfo field {name} is new: decide how Nightjar reads it");
        };
        let mut read = false;
        for reply in [&unpaired, &info] {
            if fields(&reply.raw)?.contains(name) {
                read |= reads(&reply.raw, name, h.ports.http)?;
            }
        }
        match reading_of {
            Reading::Read => assert!(read, "Nightjar ignores {name}, listed as read"),
            Reading::Ignored(why) => {
                assert!(!read, "Nightjar reads {name}, listed as ignored ({why})")
            }
        }
    }
    for (field, _) in SERVERINFO_FIELDS {
        // ServerCommand needs a command; the rest the host always sends a paired device.
        assert!(
            *field == "ServerCommand" || sent.iter().any(|s| s == field),
            "the host no longer sends {field}"
        );
    }

    // Of the native API's features Nightjar reads only the file transfer ones, which
    // `expected` and the file transfer tests check.
    let native = client.native_capabilities_with_files()?;
    let capabilities = info.capabilities.with_native(&native);
    let offered: Vec<_> = Feature::ALL
        .into_iter()
        .map(|f| (f, capabilities.availability(f)))
        .collect();
    let wanted: Vec<_> = Feature::ALL.into_iter().map(|f| (f, expected(f))).collect();
    assert_eq!(offered, wanted);
    assert_eq!(suspends.0.load(Ordering::SeqCst), 0);
    Ok(())
}

/// The transfer folder and the setting naming it.
struct Folder(PathBuf);
impl Folder {
    fn new() -> Result<Self> {
        let path = std::env::temp_dir().join(format!(
            "butterpollo-interop-files-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir(&path)?;
        Ok(Self(path))
    }
    fn setting(&self) -> String {
        format!(
            "file_transfer=true\nfile_transfer_dir={}\n",
            self.0.display()
        )
    }
}
impl Drop for Folder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn upload(client: &HostClient, name: &str, bytes: &[u8]) -> Result<u64, FileError> {
    let uploaded = client.upload_file(
        name,
        &mut &bytes[..],
        bytes.len() as u64,
        &mut |_, _| {},
        &TransferCancel::new(),
    )?;
    assert_eq!(uploaded.name, name);
    Ok(uploaded.size)
}
fn download(client: &HostClient, name: &str) -> Result<Vec<u8>, FileError> {
    let mut bytes = vec![];
    let length = client.download_file(name, &mut bytes, &mut |_, _| {}, &TransferCancel::new())?;
    assert_eq!(length, bytes.len() as u64);
    Ok(bytes)
}
/// The status and error code of a refusal, as the host sent them.
fn refusal<T: std::fmt::Debug>(result: Result<T, FileError>) -> (u16, &'static str) {
    match result {
        Err(FileError::Refused {
            refusal, status, ..
        }) => (status, refusal.code()),
        other => panic!("not a refusal: {other:?}"),
    }
}

/// Files through the native API: an upload Nightjar lists and downloads again, and each
/// refusal's status and code as Nightjar reads them.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nightjar_transfers_files_and_reads_each_refusal() -> Result<()> {
    let folder = Folder::new()?;
    let h = Harness::configured(
        10_000,
        1,
        &format!("{}file_transfer_max_mb=1\n", folder.setting()),
    )
    .await?;
    let client = device(&h, u32::MAX, &[])?;
    let info = client.server_info(true)?;
    let native = client.native_capabilities_with_files()?;
    assert!(native.files.file_upload && native.files.file_download);
    let capabilities = info.capabilities.with_native(&native);
    assert!(capabilities.has(Feature::FileUpload) && capabilities.has(Feature::FileDownload));

    assert_eq!(upload(&client, "notes.txt", b"from Nightjar")?, 13);
    assert_eq!(std::fs::read(folder.0.join("notes.txt"))?, b"from Nightjar");
    let list = client.list_files()?;
    assert_eq!(list.max_file_bytes, 1 << 20);
    let listed: Vec<_> = list
        .files
        .iter()
        .map(|f| (f.name.as_str(), f.size))
        .collect();
    assert_eq!(listed, [("notes.txt", 13)]);
    assert_eq!(download(&client, "notes.txt")?, b"from Nightjar");

    assert_eq!(
        refusal(download(&client, "missing.txt")),
        (404, "not_found")
    );
    assert_eq!(refusal(upload(&client, "CON", b"x")), (400, "invalid_name"));
    assert_eq!(
        refusal(upload(&client, "big.bin", &vec![0; (1 << 20) + 1])),
        (413, "too_large")
    );
    // Free space down to the reserve the host keeps.
    h.h.file_transfers.fake_space(1 << 30, 10 << 30);
    assert_eq!(
        refusal(upload(&client, "full.txt", b"x")),
        (507, "insufficient_storage")
    );
    h.h.file_transfers.fake_space(100 << 30, 200 << 30);

    // Without the Upload files permission, downloads go on.
    paired(&h, |c| c.perm &= !FILE_UPLOAD);
    assert_eq!(
        refusal(upload(&client, "later.txt", b"x")),
        (403, "forbidden")
    );
    assert_eq!(download(&client, "notes.txt")?, b"from Nightjar");
    let capabilities = info
        .capabilities
        .with_native(&client.native_capabilities_with_files()?);
    assert_eq!(
        capabilities.availability(Feature::FileUpload),
        Availability::Unsupported(Some(Reason::FilePermission))
    );
    assert!(capabilities.has(Feature::FileDownload));
    paired(&h, |c| c.perm &= !FILE_DOWNLOAD);
    assert_eq!(refusal(client.list_files()), (403, "forbidden"));
    assert_eq!(refusal(download(&client, "notes.txt")), (403, "forbidden"));
    assert!(!folder.0.join("later.txt").exists() && !folder.0.join("big.bin").exists());
    Ok(())
}

/// With File transfer off every route answers `file_transfer_disabled`, and the native API
/// lists no file features.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nightjar_reads_file_transfer_turned_off() -> Result<()> {
    let h = Harness::new(10_000, 1).await?;
    let client = device(&h, u32::MAX, &[])?;
    let native = client.native_capabilities_with_files()?;
    assert!(
        !native
            .capabilities
            .features
            .iter()
            .any(|f| f == feature::FILES_UPLOAD || f == feature::FILES_DOWNLOAD)
    );
    let disabled = (403, "file_transfer_disabled");
    assert_eq!(refusal(client.list_files()), disabled);
    assert_eq!(refusal(upload(&client, "notes.txt", b"x")), disabled);
    assert_eq!(refusal(download(&client, "notes.txt")), disabled);
    Ok(())
}

/// A power backend with S3 that counts suspends in place of the PC's.
#[derive(Default)]
struct Suspends(AtomicUsize);
impl crate::power_action::Backend for Suspends {
    fn states(&self) -> Result<SleepStates> {
        Ok(SleepStates {
            s3: true,
            ..Default::default()
        })
    }
    fn suspend(&self) -> Result<()> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

fn power_refusal<T: std::fmt::Debug>(result: Result<T, PowerError>) -> (Refusal, u16) {
    match result {
        Err(PowerError::Refused { refusal, reply }) => (refusal, reply.status),
        other => panic!("not a refusal: {other:?}"),
    }
}

/// Sleep PC: refused until the console allows it for the device, then one token puts the
/// (fake) PC to sleep once and can't be spent again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nightjar_puts_the_host_to_sleep_once_allowed() -> Result<()> {
    let h = Harness::new(10_000, 1).await?;
    let suspends = Arc::new(Suspends::default());
    h.h.power.set_backend(suspends.clone());
    let client = device(&h, u32::MAX, &[])?;

    let info = client.server_info(true)?;
    assert_eq!(
        info.capabilities.availability(Feature::Sleep),
        Availability::Unsupported(Some(Reason::SleepPermission))
    );
    let offer = client.power_capabilities()?;
    assert!(offer.sleep_supported && !offer.sleep_allowed && !offer.sleep_available);
    assert_eq!(offer.token, None);
    assert_eq!(
        power_refusal(client.sleep(&"0".repeat(64))),
        (Refusal::Forbidden, 403)
    );

    paired(&h, |c| c.extra.insert("allow_sleep".into(), true.into()));
    assert!(client.server_info(true)?.capabilities.has(Feature::Sleep));
    // The harness runs the desktop app, so the PC is busy: allowed, but no token yet.
    let offer = client.power_capabilities()?;
    assert!(offer.sleep_supported && offer.sleep_allowed && !offer.sleep_available);
    assert_eq!(offer.token, None);

    *h.h.current_app.lock().unwrap() = None;
    let offer = client.power_capabilities()?;
    assert!(offer.sleep_supported && offer.sleep_allowed && offer.sleep_available);
    assert_eq!(offer.active_session_policy, "idle-only");
    let token = offer.token.expect("a token for an allowed, idle host");
    let reply = client.sleep(&token)?;
    assert!(reply.accepted);
    assert_eq!(reply.status, 202);
    until("the suspend after the answer", LOADED, || {
        suspends.0.load(Ordering::SeqCst) == 1
    })
    .await;
    assert_eq!(
        power_refusal(client.sleep(&token)),
        (Refusal::Conflict, 409)
    );
    assert_eq!(suspends.0.load(Ordering::SeqCst), 1);
    Ok(())
}
