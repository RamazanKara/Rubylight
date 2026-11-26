//! Nightjar pairing, QR pairing, webhook emission, serverinfo reading, and capability matrix interop tests.
mod common;

use anyhow::Result;
use butterpollo_session::power_action::SleepStates;
use butterpollo_stream::stream::reconnect_harness::{HOST_VERSION, Harness};
use common::{LOADED, device};
use nightjar_core::http::{
    Availability, ClientIdentity, Family, Feature, HostClient, PairResult, Reason, ServerInfo,
};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::{Duration, Instant};

/// A power backend with S3 that counts suspends in place of the PC's.
#[derive(Default)]
struct Suspends(AtomicUsize);
impl butterpollo_stream::power_action::Backend for Suspends {
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
    fn power_off(&self, action: butterpollo_session::power_action::Action) -> Result<()> {
        panic!("an interop test reached the {}", action.name())
    }
}

/// Nightjar pairs with the PIN the console enters, then reads serverinfo as a paired device
/// over HTTPS pinned to the certificate it got. A wrong PIN is caught on both ends.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nightjar_pairs_with_the_pin_entered_on_the_host() -> Result<()> {
    let h = Harness::configured_with(10_000, 1, "", Some(butterpollo_web::native_api::router()))
        .await?;
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
        pin.sender
            .send((entered.into(), pin.name.clone(), None, None))
            .unwrap();
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

/// Nightjar pairs from the console's QR pairing link: one-time PIN pairing checked against the
/// link's certificate fingerprint. A link with another fingerprint is refused before Nightjar
/// trusts the host, a used link pairs no one, and the pairing raises `pairing.paired`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nightjar_pairs_from_the_qr_link_and_checks_the_host_certificate() -> Result<()> {
    use rubylight_protocol::pairing_uri::PairingUri;
    let h = Harness::configured_with(10_000, 1, "", Some(butterpollo_web::native_api::router()))
        .await?;
    let (url, mut received) = butterpollo_stream::webhooks::test_support::receiver(vec![200]).await;
    {
        let mut config = h.h.config.write().unwrap();
        config.values.insert("qr_pairing".into(), "true".into());
        config.values.insert(
            "webhooks".into(),
            serde_json::json!([{"url": url, "events": ["pairing.paired"], "secret": "s"}])
                .to_string(),
        );
    }
    let link = |h: &Harness| {
        let answer =
            butterpollo_web::web::pairing_qr(&h.h, "", None, vec!["127.0.0.1".into()]).unwrap();
        let mut link = PairingUri::parse(answer["codes"][0]["uri"].as_str().unwrap()).unwrap();
        assert_eq!(link.host, "127.0.0.1");
        // The harness listens on its own port.
        link.port = h.ports.http;
        link
    };
    let identity = Arc::new(ClientIdentity::generate()?);
    let client = |link: &PairingUri| -> Result<Arc<HostClient>> {
        Ok(Arc::new(
            HostClient::new(&link.host, link.port, identity.clone())?
                .with_device_name("Nightjar QR"),
        ))
    };
    let pair = async |client: Arc<HostClient>, link: PairingUri| {
        tokio::task::spawn_blocking(move || {
            let info = client.server_info(true)?;
            client.pair_with_link(&info, &link)
        })
        .await
        .unwrap()
    };

    // Another host's fingerprint: refused, and nothing pinned.
    let mut forged = link(&h);
    forged.cert_sha256[0] ^= 1;
    let refused = client(&forged)?;
    let result = pair(refused.clone(), forged).await;
    assert!(
        matches!(result, Err(nightjar_core::http::Error::CertificateMismatch)),
        "{result:?}"
    );
    assert!(refused.server_certificate_pem().is_none());

    let good = link(&h);
    let paired = client(&good)?;
    let result = pair(paired.clone(), good.clone()).await?;
    let PairResult::Paired {
        server_certificate_pem,
    } = result
    else {
        panic!("{result:?}");
    };
    assert_eq!(server_certificate_pem, h.h.identity.certificate);
    assert!(paired.server_info(true)?.paired);
    let event: serde_json::Value = serde_json::from_slice(&received.recv().await.unwrap().body)?;
    assert_eq!(event["event"], "pairing.paired");
    assert_eq!(event["data"]["device"]["name"], "Nightjar QR");
    assert_eq!(event["data"]["transport"], "moonlight");

    // The PIN worked once.
    let again = client(&good)?;
    let result = pair(again, good).await;
    assert!(
        !matches!(result, Ok(PairResult::Paired { .. })),
        "{result:?}"
    );
    h.finish().await
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
    ("RustHostLastFailure", Reading::Read),
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
    ("Packed444Modes", Reading::Read),
    ("MaxVideoPacket", Reading::Read),
    ("HdrFormats", Reading::Read),
    ("FecControlVersion", Reading::Read),
    ("BandwidthProbeVersion", Reading::Read),
    ("AudioCodecs", Reading::Read),
    ("AudioLayouts", Reading::Read),
    ("PipelineRatesVersion", Reading::Read),
    ("DisplayScaleVersion", Reading::Read),
    ("TextInputVersion", Reading::Read),
    ("KeyboardLayoutVersion", Reading::Read),
    ("PenBarrelRollVersion", Reading::Read),
    ("SecondScreenVersion", Reading::Read),
    ("FrameWarpProtocol", Reading::Read),
    ("ControllerIdentityVersion", Reading::Read),
    ("HdrDynamicControl", Reading::Read),
    ("RustHostInternetPlayVersion", Reading::Read),
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
        | Feature::ClientFec
        | Feature::BandwidthProbe
        | Feature::WebUi
        | Feature::NativeApi
        | Feature::Sleep
        // 7.1.4 and PCM audio are on by default.
        | Feature::Surround714
        | Feature::AudioPassthrough
        // Dolby Vision 8.1 and 8.4 and HLG on the fixture's HEVC Main10 encoder
        // (hdr_dolby_vision and hdr_hlg are on).
        | Feature::DolbyVision
        | Feature::Hlg => Supported,
        // `rlSecondScreen` is read on every launch.
        Feature::SecondScreen => Supported,
        // `frame_warp` is on by default.
        Feature::FrameWarp => Supported,
        // Keyboard follows focus, the device's layout and pen barrel roll are on by default.
        Feature::TextInput | Feature::ClientKeyboardLayout | Feature::PenBarrelRoll | Feature::ControllerIdentity => Supported,
        Feature::ServerCommands => Unsupported(Some(Reason::NoCommands)),
        // Display modes are the virtual display's. Before Windows 11 (CI's Server 2022)
        // the default mode is disabled, so the host says it is off, not missing a driver.
        Feature::VirtualDisplay
        | Feature::VirtualDisplayHdr
        | Feature::HostDisplayModes
        | Feature::DisplayScale => {
            if butterpollo_win_display::display::virtual_display_os() {
                Unsupported(Some(Reason::VirtualDisplayDriver))
            } else {
                Unsupported(Some(Reason::VirtualDisplayOff))
            }
        }
        // On by default (`hardware_metrics`).
        Feature::HardwareMetrics => Supported,
        // No PyroWave encoder: ServerCodecModeSupport has no PyroWave profile.
        Feature::PyroWave | Feature::PyroWaveRecords => Unsupported(None),
        // The host has no cursor shape channel: it sends no CursorShapeVersion.
        Feature::CursorShape => Unsupported(None),
        // File transfer is off; listed by the native API only when on.
        Feature::FileUpload | Feature::FileDownload => Unsupported(None),
        // The 3.0 extensions are defined but offered by nothing yet.
        Feature::UsbForwarding | Feature::SharedFolders => Unsupported(None),
        // Nucleus Co-op runs only in the service host, which the fixture is not.
        Feature::Coop => Unsupported(None),
        // Play over the internet is off by default: RustHostInternetPlayVersion is 0.
        Feature::InternetPlay => Unsupported(None),
        // clipboard_image is on by default.
        Feature::ImageClipboard => Supported,
        // Always listed; the device has every permission, Launch apps among them.
        Feature::MicOnly => Supported,
        // Always listed; the device has every permission, View and Launch among them.
        Feature::ContinueHere => Supported,
        // Listed always; reading needs View streams, which every permission includes.
        Feature::NeuralFrameGeneration => Supported,
        // Restart/shutdown and pairing approval are offered, but this client has no grant.
        Feature::PcRestart | Feature::PcShutdown => {
            Unsupported(Some(Reason::RestartShutdownPermission))
        }
        Feature::PairingApprove => Unsupported(Some(Reason::PairingApprovePermission)),
        // Update installation requires Host control, which this client lacks.
        Feature::HostUpdate => Unsupported(None),
    }
}

/// Serverinfo and the native API's capabilities as Nightjar reads them: every field the host
/// sends is one Nightjar reads or knowingly ignores, and every Feature Nightjar knows has this
/// host's answer.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nightjar_knows_every_capability_the_host_advertises() -> Result<()> {
    let h = Harness::configured_with(10_000, 1, "", Some(butterpollo_web::native_api::router()))
        .await?;
    // Seed an AMD GPU so GPU-less CI runners exercise host neural frame generation advertisement.
    *h.h.neural.adapters.lock().unwrap() = Some(vec![butterpollo_session::neural::Adapter {
        name: "AMD Radeon 780M Graphics".into(),
        luid: 1,
        vendor: butterpollo_session::neural::AMD,
        device: 0x1900,
    }]);
    let suspends = Arc::new(Suspends::default());
    h.h.power.set_backend(suspends.clone());
    let client = device(&h, u32::MAX, &[("allow_sleep", true)])?;
    let unpaired = client.server_info_http(true)?;
    let info = client.server_info_https()?;
    assert!(!unpaired.paired && info.paired);
    assert_eq!(unpaired.capabilities.family(), Family::Rubylight);
    assert_eq!(info.capabilities.family(), Family::Rubylight);
    assert_eq!(info.capabilities.host_version(), Some(HOST_VERSION));

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

    // Of the native API's features Nightjar reads the file transfer ones and the
    // 3.0 extensions, which `expected` and the file transfer tests check.
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
