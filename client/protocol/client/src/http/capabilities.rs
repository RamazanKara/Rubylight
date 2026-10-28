//! What one host can do, judged only from its serverinfo reply. Port of Nightjar's
//! HostCapabilities.java: the family names the host for labels and protocol quirks; features
//! alone decide what a client offers.

use super::native::NativeCapabilities;
use super::xml::Element;

/// The host software, for labels and wire-format quirks. Never gates a feature.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Family {
    GameStream,
    Sunshine,
    Apollo,
    Vibepollo,
    Rubylight,
    Unknown,
}

/// Something a host can offer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Feature {
    Permissions,
    OtpPairing,
    Clipboard,
    ServerCommands,
    AppUuids,
    LaunchByUuid,
    VirtualDisplay,
    VirtualDisplayHdr,
    HostDisplayModes,
    FractionalRefresh,
    HostVrr,
    RuntimeBitrate,
    PyroWave,
    PyroWaveRecords,
    PyroWaveBandwidthProbe,
    HardwareMetrics,
    NetworkPath,
    FrameLimiterStatus,
    ControllerPreparation,
    RemoteSessionTiles,
    HostConfirmsReplacement,
    HostTiming,
    WebUi,
    CursorShape,
    /// The Rubylight native API (`/api/native/v1`): library, session actions, Radeon state.
    NativeApi,
    /// Sleep PC (`/power-actions/v1`, Vibepollo's wire format): suspend an idle host.
    Sleep,
    /// Upload files to the host (`/api/native/v1/files`). Known only after
    /// [`HostCapabilities::with_native`]: gate the UI on [`HostCapabilities::has`], never
    /// `may_have`, which is true before the native API answered.
    FileUpload,
    /// List and download the host's files. Known only after [`HostCapabilities::with_native`];
    /// gate it like `FileUpload`.
    FileDownload,
    /// Dolby Vision profile 8.1 or 8.4 (`HdrFormats`, `rubylight_protocol::hdr`).
    DolbyVision,
    /// HLG HDR (`HdrFormats`).
    Hlg,
    /// The client picks the video FEC (`rlFecPercentage`) and sends FEC reports (0x5539).
    ClientFec,
    /// `GET /bandwidth-probe` (`BandwidthProbeVersion`).
    BandwidthProbe,
    /// USB device forwarding (native `usb`). Known only after [`HostCapabilities::with_native`].
    UsbForwarding,
    /// PCM, AC3 or E-AC3 audio instead of Opus (`AudioCodecs`).
    AudioPassthrough,
    /// 12-channel 7.1.4 audio (`AudioLayouts`).
    Surround714,
    /// The host raises and lowers the keyboard with an IME context (0x553B).
    TextInput,
    /// The host takes this device's keyboard layout (0x553E).
    ClientKeyboardLayout,
    /// Pen barrel roll (input 0x5500000B).
    PenBarrelRoll,
    /// Pipeline frame rates for the overlay (0x553D).
    PipelineRates,
    /// PNG clipboard (native `clipboard.image`). Known only after
    /// [`HostCapabilities::with_native`].
    ImageClipboard,
    /// Shared folders (native `folders`). Known only after [`HostCapabilities::with_native`].
    SharedFolders,
}

impl Feature {
    pub const ALL: [Feature; 41] = [
        Feature::Permissions,
        Feature::OtpPairing,
        Feature::Clipboard,
        Feature::ServerCommands,
        Feature::AppUuids,
        Feature::LaunchByUuid,
        Feature::VirtualDisplay,
        Feature::VirtualDisplayHdr,
        Feature::HostDisplayModes,
        Feature::FractionalRefresh,
        Feature::HostVrr,
        Feature::RuntimeBitrate,
        Feature::PyroWave,
        Feature::PyroWaveRecords,
        Feature::PyroWaveBandwidthProbe,
        Feature::HardwareMetrics,
        Feature::NetworkPath,
        Feature::FrameLimiterStatus,
        Feature::ControllerPreparation,
        Feature::RemoteSessionTiles,
        Feature::HostConfirmsReplacement,
        Feature::HostTiming,
        Feature::WebUi,
        Feature::CursorShape,
        Feature::NativeApi,
        Feature::Sleep,
        Feature::FileUpload,
        Feature::FileDownload,
        Feature::DolbyVision,
        Feature::Hlg,
        Feature::ClientFec,
        Feature::BandwidthProbe,
        Feature::UsbForwarding,
        Feature::AudioPassthrough,
        Feature::Surround714,
        Feature::TextInput,
        Feature::ClientKeyboardLayout,
        Feature::PenBarrelRoll,
        Feature::PipelineRates,
        Feature::ImageClipboard,
        Feature::SharedFolders,
    ];
}

/// What would make an unsupported feature available.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Reason {
    /// The host lacks something a newer version has. For a Rubylight host without the native
    /// API this is a 2.6.x host, and NVIDIA and Intel users cannot update to 3.0 (it is
    /// AMD-only), so the UI must not say "update the host" for it. Word it as "this host
    /// doesn't offer it".
    UpdateHost,
    Pair,
    ViewPermission,
    ClipboardPermission,
    CommandPermission,
    NoCommands,
    VirtualDisplayOff,
    VirtualDisplayDriver,
    HardwareMetricsOff,
    /// The host supports sleep but has not allowed it for this paired client.
    SleepPermission,
    /// The host offers file transfer but has not allowed it for this device.
    FilePermission,
    /// The host forwards USB devices but has not allowed it for this device.
    UsbPermission,
    /// The host shares folders but has granted this device none.
    FolderPermission,
}

/// Whether a host offers a feature; an unsupported one may carry what would change that.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Availability {
    Supported,
    /// `None`: nothing the user could change on the host alters it.
    Unsupported(Option<Reason>),
    Unknown,
}

impl Availability {
    const NEVER: Availability = Availability::Unsupported(None);

    pub fn is_supported(self) -> bool {
        self == Availability::Supported
    }

    pub fn is_never(self) -> bool {
        self == Availability::NEVER
    }
}

/// ServerCodecModeSupport bits of the four PyroWave profiles.
pub const PYROWAVE_CODEC_BITS: u32 = 0x0080_0000 | 0x0100_0000 | 0x0200_0000 | 0x0400_0000;
/// Default bandwidth probe size; a host advertising another size gets no probe.
pub const PYROWAVE_BANDWIDTH_PROBE_BYTES: u32 = 32 * 1024 * 1024;

pub const PERMISSION_CONTROLLER: u32 = 1 << 8;
pub const PERMISSION_INPUT: u32 = 0x0000_1f00;
pub const PERMISSION_CLIPBOARD_SET: u32 = 1 << 16;
pub const PERMISSION_CLIPBOARD_READ: u32 = 1 << 17;
pub const PERMISSION_SERVER_COMMAND: u32 = 1 << 20;
pub const PERMISSION_LIST: u32 = 1 << 24;
pub const PERMISSION_VIEW: u32 = 1 << 25;
pub const PERMISSION_LAUNCH: u32 = 1 << 26;

/// The serverinfo fields capabilities are derived from.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Evidence {
    pub game_stream: bool,
    pub sunshine_version: bool,
    pub paired: bool,
    pub rust_host_version: Option<String>,
    /// -1 when the host reports no permissions.
    pub permission: i64,
    pub server_commands: u32,
    pub virtual_display_capable: Option<bool>,
    pub virtual_display_driver_ready: Option<bool>,
    pub virtual_display_hdr_capable: Option<bool>,
    pub frame_limiter: Option<bool>,
    pub pyrowave_fields: bool,
    pub pyrowave_probe_bytes: u32,
    pub pyrowave_codecs: u32,
    pub encoder_bitrate: bool,
    pub hardware_metrics_version: i32,
    pub network_path_version: i32,
    pub controller_preparation: bool,
    pub current_game_uuid: bool,
    pub cursor_shape_version: i32,
    /// -1 when the host has no native API, 0 when it has one but not for this request (plain
    /// HTTP or an unpaired certificate), 1 or more for the highest version it serves.
    pub native_api_version: i32,
    /// -1 when serverinfo has no `PowerActionVersion`: hosts send it only to paired clients
    /// over HTTPS.
    pub power_action_version: i32,
    pub sleep_supported: bool,
    /// The host lets this paired client put it to sleep.
    pub sleep_allowed: bool,
    /// The native API's capabilities were read (see [`HostCapabilities::with_native`]); the
    /// fields below come from them, not from serverinfo.
    pub native_capabilities: bool,
    /// `files.upload` and `files.download` in the native features: on only while the host's
    /// file transfer setting is.
    pub files_upload: bool,
    pub files_download: bool,
    /// The device's `file_upload` and `file_download` permissions.
    pub file_upload_permission: bool,
    pub file_download_permission: bool,
    /// The 3.0 serverinfo fields, -1 when absent: `HdrFormats`, `AudioCodecs` and
    /// `AudioLayouts` are bit masks, the rest versions.
    pub hdr_formats: i32,
    pub fec_control_version: i32,
    pub bandwidth_probe_version: i32,
    pub audio_codecs: i32,
    pub audio_layouts: i32,
    pub text_input_version: i32,
    pub keyboard_layout_version: i32,
    pub pen_barrel_roll_version: i32,
    pub pipeline_rates_version: i32,
    /// `usb`, `clipboard.image` and `folders` in the native features, and the device's `usb`
    /// and `shared_folders` permissions.
    pub usb_offered: bool,
    pub usb_permission: bool,
    pub image_clipboard_offered: bool,
    pub folders_offered: bool,
    pub folder_permission: bool,
}

/// A host's capabilities. [`HostCapabilities::UNKNOWN`] is a host never reached.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct HostCapabilities {
    evidence: Option<Evidence>,
}

fn unsigned(value: &str, max: u64) -> u64 {
    match value.trim().parse::<i64>() {
        Ok(v) if v >= 0 && (v as u64) <= max => v as u64,
        _ => 0,
    }
}

fn flag(value: &str) -> Option<bool> {
    let value = value.trim();
    if value == "1" || value.eq_ignore_ascii_case("true") {
        Some(true)
    } else if value == "0" || value.eq_ignore_ascii_case("false") {
        Some(false)
    } else {
        None
    }
}

/// "2.2.21", "2.0.0-rc.19" or "3.0.0-beta.1" as [major, minor, patch, pre-release], a release
/// after its pre-releases, which rank alpha, beta, rc (the stage in the millions of the last part).
pub fn parse_rubylight_version(version: &str) -> Option<[u32; 4]> {
    let mut rest = version;
    let number = |rest: &mut &str| -> Option<u32> {
        let len = rest.bytes().take_while(u8::is_ascii_digit).count();
        if len == 0 || (len > 1 && rest.starts_with('0')) {
            return None;
        }
        let value: u32 = rest[..len].parse().ok().filter(|&v| v <= 100_000)?;
        *rest = &rest[len..];
        Some(value)
    };
    let mut parts = [0, 0, 0, u32::MAX];
    for (i, part) in parts.iter_mut().take(3).enumerate() {
        *part = number(&mut rest)?;
        if i < 2 {
            rest = rest.strip_prefix('.')?;
        }
    }
    // The 3.0 hosts report "3.0.0-beta.N". The number stops at 100000, below one stage.
    const STAGE: u32 = 1_000_000;
    if let Some((stage, after)) = ["-alpha.", "-beta.", "-rc."]
        .iter()
        .enumerate()
        .find_map(|(stage, tag)| rest.strip_prefix(tag).map(|after| (stage as u32 + 1, after)))
    {
        rest = after;
        parts[3] = stage * STAGE + number(&mut rest).filter(|&pre| pre > 0)?;
    }
    if let Some(build) = rest.strip_prefix('+') {
        let len = build
            .bytes()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == b'.' || *c == b'-')
            .count();
        if len == 0 {
            return None;
        }
        rest = &build[len..];
    }
    rest.is_empty().then_some(parts)
}

const HOST_DISPLAY_MODES_VERSION: [u32; 4] = [2, 2, 1, u32::MAX];

impl Evidence {
    pub(crate) fn parse(root: &Element) -> Self {
        let mut e = Evidence {
            permission: -1,
            hardware_metrics_version: -1,
            network_path_version: -1,
            cursor_shape_version: -1,
            native_api_version: -1,
            power_action_version: -1,
            hdr_formats: -1,
            fec_control_version: -1,
            bandwidth_probe_version: -1,
            audio_codecs: -1,
            audio_layouts: -1,
            text_input_version: -1,
            keyboard_layout_version: -1,
            pen_barrel_roll_version: -1,
            pipeline_rates_version: -1,
            ..Default::default()
        };
        let (mut encoder, mut initial_encoder) = (false, false);
        for field in root.elements() {
            let text = field.text();
            match field.name.as_str() {
                // The MJOLNIR codename was used by GFE but never by any third-party server.
                "state" => e.game_stream = text.contains("MJOLNIR"),
                "appversion" => e.sunshine_version = text.trim().ends_with(".-1"),
                "PairStatus" => e.paired = text.trim() == "1",
                "RustHostVersion" => e.rust_host_version = Some(text.trim().to_owned()),
                "Permission" => e.permission = unsigned(&text, 0xFFFF_FFFF) as i64,
                "ServerCommand" => e.server_commands += 1,
                "VirtualDisplayCapable" => e.virtual_display_capable = flag(&text),
                "VirtualDisplayDriverReady" => e.virtual_display_driver_ready = flag(&text),
                "VirtualDisplayHDRCapable" => e.virtual_display_hdr_capable = flag(&text),
                "FrameLimiterSupported" => e.frame_limiter = Some(flag(&text) == Some(true)),
                "PyroWaveHostLinkMbps" => e.pyrowave_fields = true,
                "PyroWaveBandwidthProbeBytes" => {
                    e.pyrowave_fields = true;
                    e.pyrowave_probe_bytes =
                        unsigned(&text, PYROWAVE_BANDWIDTH_PROBE_BYTES as u64) as u32;
                }
                "ServerCodecModeSupport" => {
                    e.pyrowave_codecs = unsigned(&text, 0xFFFF_FFFF) as u32 & PYROWAVE_CODEC_BITS
                }
                "RustHostEncoderBitrateKbps" => encoder = true,
                "RustHostInitialEncoderBitrateKbps" => initial_encoder = true,
                "HardwareMetricsVersion" => {
                    e.hardware_metrics_version = i32::from(text.trim() == "1")
                }
                "NetworkPathVersion" => e.network_path_version = i32::from(text.trim() == "1"),
                "ControllerPreparation" => e.controller_preparation = text.trim() == "1",
                "CursorShapeVersion" => e.cursor_shape_version = unsigned(&text, 0xFFFF) as i32,
                "HdrFormats" => e.hdr_formats = unsigned(&text, 0xFF) as i32,
                "FecControlVersion" => e.fec_control_version = unsigned(&text, 0xFFFF) as i32,
                "BandwidthProbeVersion" => {
                    e.bandwidth_probe_version = unsigned(&text, 0xFFFF) as i32
                }
                "AudioCodecs" => e.audio_codecs = unsigned(&text, 0xFF) as i32,
                "AudioLayouts" => e.audio_layouts = unsigned(&text, 0xFF) as i32,
                "TextInputVersion" => e.text_input_version = unsigned(&text, 0xFFFF) as i32,
                "KeyboardLayoutVersion" => {
                    e.keyboard_layout_version = unsigned(&text, 0xFFFF) as i32
                }
                "PenBarrelRollVersion" => {
                    e.pen_barrel_roll_version = unsigned(&text, 0xFFFF) as i32
                }
                "PipelineRatesVersion" => e.pipeline_rates_version = unsigned(&text, 0xFFFF) as i32,
                "NativeApiVersion" => e.native_api_version = unsigned(&text, 0xFFFF) as i32,
                "PowerActionVersion" => e.power_action_version = unsigned(&text, 0xFFFF) as i32,
                "SleepSupported" => e.sleep_supported = flag(&text) == Some(true),
                "SleepAllowed" => e.sleep_allowed = flag(&text) == Some(true),
                "currentgameuuid" => e.current_game_uuid = true,
                _ => {}
            }
        }
        e.encoder_bitrate = encoder && initial_encoder;
        if e.rust_host_version.as_deref() == Some("") {
            e.rust_host_version = None;
        }
        e
    }

    fn family(&self) -> Family {
        if self.game_stream {
            Family::GameStream
        } else if self.rust_host_version.is_some() {
            Family::Rubylight
        } else if self.pyrowave_fields || self.frame_limiter.is_some() {
            Family::Vibepollo
        } else if self.permission != -1
            || self.server_commands > 0
            || self.virtual_display_capable.is_some()
        {
            Family::Apollo
        } else if self.sunshine_version {
            Family::Sunshine
        } else {
            Family::Unknown
        }
    }

    fn has_permission(&self, mask: u32) -> bool {
        self.permission == -1 || (self.permission as u32 & mask) != 0
    }

    fn can_view(&self) -> bool {
        self.has_permission(PERMISSION_VIEW | PERMISSION_LAUNCH)
    }

    /// Rubylight, or the extension fields that Vibepollo added and Rubylight also sends.
    fn vibepollo_extensions(&self) -> bool {
        self.rust_host_version.is_some() || self.frame_limiter.is_some()
    }

    /// No evidence: an unrecognized host rules nothing out; Apollo and Vibepollo hide some
    /// fields from unpaired clients.
    fn absent(&self, hidden_until_paired: bool) -> Availability {
        let family = self.family();
        if family == Family::Unknown
            || (hidden_until_paired
                && !self.paired
                && matches!(family, Family::Apollo | Family::Vibepollo))
        {
            Availability::Unknown
        } else {
            Availability::NEVER
        }
    }

    fn rubylight_version(&self) -> Option<[u32; 4]> {
        self.rust_host_version
            .as_deref()
            .and_then(parse_rubylight_version)
    }

    fn virtual_display(&self) -> Availability {
        match self.virtual_display_capable {
            None => self.absent(true),
            Some(false) if self.rust_host_version.is_some() => {
                Availability::Unsupported(Some(Reason::VirtualDisplayOff))
            }
            Some(false) => Availability::NEVER,
            Some(true) if self.virtual_display_driver_ready == Some(false) => {
                Availability::Unsupported(Some(Reason::VirtualDisplayDriver))
            }
            Some(true) => Availability::Supported,
        }
    }

    fn availability(&self, feature: Feature) -> Availability {
        use Availability::{Supported, Unknown, Unsupported};
        let rubylight = self.rust_host_version.is_some();
        let update = Unsupported(Some(Reason::UpdateHost));
        let pair = Unsupported(Some(Reason::Pair));
        match feature {
            Feature::Permissions => {
                if self.permission != -1 {
                    Supported
                } else {
                    self.absent(false)
                }
            }
            Feature::OtpPairing => {
                if self.permission != -1 || rubylight {
                    Supported
                } else {
                    self.absent(false)
                }
            }
            Feature::Clipboard => {
                if self.permission == -1 {
                    self.absent(false)
                } else if !self.paired {
                    pair
                } else if self.can_view()
                    && self.permission as u32
                        & (PERMISSION_CLIPBOARD_READ | PERMISSION_CLIPBOARD_SET)
                        != 0
                {
                    Supported
                } else {
                    Unsupported(Some(Reason::ClipboardPermission))
                }
            }
            Feature::ServerCommands => {
                if self.permission == -1 {
                    if self.server_commands > 0 {
                        Supported
                    } else {
                        self.absent(false)
                    }
                } else if !self.paired {
                    pair
                } else if self.permission as u32 & PERMISSION_SERVER_COMMAND == 0 {
                    Unsupported(Some(Reason::CommandPermission))
                } else if self.server_commands > 0 {
                    Supported
                } else {
                    Unsupported(Some(Reason::NoCommands))
                }
            }
            Feature::AppUuids => {
                if self.current_game_uuid {
                    Supported
                } else {
                    self.absent(false)
                }
            }
            // Rubylight and Vibepollo look an appuuid up before the ID.
            Feature::LaunchByUuid => {
                if self.vibepollo_extensions() {
                    Supported
                } else {
                    self.absent(false)
                }
            }
            Feature::VirtualDisplay => self.virtual_display(),
            Feature::VirtualDisplayHdr => {
                let display = self.virtual_display();
                if !display.is_supported() {
                    display
                } else {
                    match self.virtual_display_hdr_capable {
                        None => self.absent(false),
                        Some(true) => Supported,
                        Some(false) => Availability::NEVER,
                    }
                }
            }
            Feature::HostDisplayModes => {
                if !rubylight {
                    self.absent(false)
                } else if !self
                    .rubylight_version()
                    .is_some_and(|v| v >= HOST_DISPLAY_MODES_VERSION)
                {
                    update
                } else {
                    self.virtual_display()
                }
            }
            // Rubylight, Apollo and Vibepollo read "59.94"; Sunshine truncates it to 59.
            Feature::FractionalRefresh => {
                if self.vibepollo_extensions() || self.permission != -1 {
                    Supported
                } else {
                    self.absent(false)
                }
            }
            Feature::HostTiming => {
                if rubylight {
                    Supported
                } else {
                    self.absent(false)
                }
            }
            Feature::HostVrr | Feature::RemoteSessionTiles | Feature::HostConfirmsReplacement => {
                if self.vibepollo_extensions() {
                    Supported
                } else {
                    self.absent(false)
                }
            }
            Feature::RuntimeBitrate => {
                let missing = if rubylight {
                    !self.encoder_bitrate
                } else {
                    self.frame_limiter.is_none()
                };
                if missing {
                    if rubylight {
                        update
                    } else {
                        self.absent(false)
                    }
                } else if !self.paired {
                    pair
                } else if self.can_view() {
                    Supported
                } else {
                    Unsupported(Some(Reason::ViewPermission))
                }
            }
            Feature::PyroWave => {
                if !self.pyrowave_fields && !rubylight {
                    self.absent(true)
                } else if self.pyrowave_codecs != 0 {
                    Supported
                } else {
                    Availability::NEVER
                }
            }
            Feature::PyroWaveRecords => {
                let pyrowave = self.availability(Feature::PyroWave);
                if !pyrowave.is_supported() {
                    pyrowave
                } else if !rubylight || self.rubylight_version().is_some_and(|v| v[0] >= 2) {
                    // Rubylight frames records since 2.0.0-rc.1; Vibepollo since it offered PyroWave.
                    Supported
                } else {
                    Availability::NEVER
                }
            }
            Feature::PyroWaveBandwidthProbe => {
                if !self.pyrowave_fields {
                    if rubylight {
                        update
                    } else {
                        self.absent(true)
                    }
                } else if !self.paired {
                    pair
                } else if self.pyrowave_probe_bytes == PYROWAVE_BANDWIDTH_PROBE_BYTES {
                    Supported
                } else {
                    Availability::NEVER
                }
            }
            Feature::HardwareMetrics => {
                if self.hardware_metrics_version == -1 {
                    if rubylight {
                        update
                    } else {
                        self.absent(false)
                    }
                } else if !self.paired {
                    pair
                } else if !self.can_view() {
                    Unsupported(Some(Reason::ViewPermission))
                } else if self.hardware_metrics_version == 1 {
                    Supported
                } else {
                    Unsupported(Some(Reason::HardwareMetricsOff))
                }
            }
            Feature::NetworkPath => {
                if self.network_path_version == -1 {
                    if rubylight {
                        update
                    } else {
                        self.absent(false)
                    }
                } else if !self.paired {
                    pair
                } else if self.network_path_version == 1 {
                    Supported
                } else {
                    Unsupported(Some(Reason::ViewPermission))
                }
            }
            Feature::FrameLimiterStatus => {
                if self.frame_limiter == Some(true) {
                    Supported
                } else {
                    self.absent(false)
                }
            }
            Feature::ControllerPreparation => {
                if self.controller_preparation {
                    Supported
                } else if rubylight {
                    update
                } else {
                    self.absent(false)
                }
            }
            Feature::WebUi => {
                if self.game_stream {
                    Availability::NEVER
                } else if self.family() != Family::Unknown {
                    Supported
                } else {
                    Unknown
                }
            }
            Feature::CursorShape => {
                if self.cursor_shape_version >= 1 {
                    Supported
                } else {
                    self.absent(false)
                }
            }
            // The host reports 0 over plain HTTP and to an unpaired certificate.
            Feature::NativeApi => {
                if self.native_api_version == -1 {
                    if rubylight {
                        update
                    } else {
                        self.absent(false)
                    }
                } else if !self.paired || self.native_api_version < 1 {
                    pair
                } else {
                    Supported
                }
            }
            // Rubylight and Vibepollo send the power fields only to a paired HTTPS client.
            Feature::Sleep => {
                if self.power_action_version == -1 {
                    // A paired HTTPS serverinfo always carries the power fields when the
                    // host has them, so their absence there rules Sleep out.
                    if !rubylight {
                        if self.paired {
                            Availability::NEVER
                        } else {
                            self.absent(true)
                        }
                    } else if !self.paired {
                        pair
                    } else {
                        update
                    }
                } else if !self.paired {
                    pair
                } else if self.power_action_version < 1 || !self.sleep_supported {
                    Availability::NEVER
                } else if !self.sleep_allowed {
                    Unsupported(Some(Reason::SleepPermission))
                } else {
                    Supported
                }
            }
            // From the native capabilities: the host's features, then the device's permission.
            Feature::FileUpload | Feature::FileDownload => {
                let (offered, allowed) = if feature == Feature::FileUpload {
                    (self.files_upload, self.file_upload_permission)
                } else {
                    (self.files_download, self.file_download_permission)
                };
                let native = self.availability(Feature::NativeApi);
                if !native.is_supported() {
                    native
                } else if !self.native_capabilities {
                    Unknown
                } else if !offered {
                    Availability::NEVER
                } else if !allowed {
                    Unsupported(Some(Reason::FilePermission))
                } else {
                    Supported
                }
            }
            // A 3.0 host leaves a field out when a setting or its hardware rules the feature
            // out, so a missing field says "not offered", never "update the host".
            Feature::DolbyVision => {
                self.serverinfo_bits(self.hdr_formats, rubylight_protocol::hdr::DOLBY_VISION)
            }
            Feature::Hlg => self.serverinfo_bits(self.hdr_formats, rubylight_protocol::hdr::HLG),
            Feature::ClientFec => self.serverinfo_version(self.fec_control_version),
            Feature::BandwidthProbe => self.serverinfo_version(self.bandwidth_probe_version),
            Feature::AudioPassthrough => {
                use rubylight_protocol::audio_ext::{CODEC_AC3, CODEC_EAC3, CODEC_PCM};
                self.serverinfo_bits(self.audio_codecs, CODEC_PCM | CODEC_AC3 | CODEC_EAC3)
            }
            Feature::Surround714 => self.serverinfo_bits(
                self.audio_layouts,
                rubylight_protocol::audio_ext::LAYOUT_MASK_714,
            ),
            Feature::TextInput => self.serverinfo_version(self.text_input_version),
            Feature::ClientKeyboardLayout => self.serverinfo_version(self.keyboard_layout_version),
            Feature::PenBarrelRoll => self.serverinfo_version(self.pen_barrel_roll_version),
            Feature::PipelineRates => self.serverinfo_version(self.pipeline_rates_version),
            Feature::UsbForwarding => {
                let refused = (!self.usb_permission).then_some(Reason::UsbPermission);
                self.native_feature(self.usb_offered, refused)
            }
            Feature::ImageClipboard => {
                let native = self.native_feature(self.image_clipboard_offered, None);
                if native.is_supported() {
                    self.availability(Feature::Clipboard)
                } else {
                    native
                }
            }
            Feature::SharedFolders => {
                let refused = (!self.folder_permission).then_some(Reason::FolderPermission);
                self.native_feature(self.folders_offered, refused)
            }
        }
    }

    /// A serverinfo mask with any of `bits` set; a missing field is `absent(false)`.
    fn serverinfo_bits(&self, mask: i32, bits: u8) -> Availability {
        if mask >= 0 && mask & i32::from(bits) != 0 {
            Availability::Supported
        } else {
            self.absent(false)
        }
    }

    /// A serverinfo version of 1 or later; a missing field is `absent(false)`.
    fn serverinfo_version(&self, version: i32) -> Availability {
        if version >= 1 {
            Availability::Supported
        } else {
            self.absent(false)
        }
    }

    /// A native feature: the native API, then its capabilities, the feature, and `refused`
    /// when the device lacks its permission.
    fn native_feature(&self, offered: bool, refused: Option<Reason>) -> Availability {
        let native = self.availability(Feature::NativeApi);
        if !native.is_supported() {
            native
        } else if !self.native_capabilities {
            Availability::Unknown
        } else if !offered {
            Availability::NEVER
        } else if refused.is_some() {
            Availability::Unsupported(refused)
        } else {
            Availability::Supported
        }
    }
}

impl HostCapabilities {
    /// A host never reached from this device: nothing is known, so nothing is ruled out.
    pub const UNKNOWN: HostCapabilities = HostCapabilities { evidence: None };

    pub fn from_evidence(evidence: Evidence) -> Self {
        Self {
            evidence: Some(evidence),
        }
    }

    pub fn evidence(&self) -> Option<&Evidence> {
        self.evidence.as_ref()
    }

    /// These capabilities with what the native API's `GET /capabilities` adds: the file
    /// transfer, USB, image clipboard and shared folder features and permissions. Read it
    /// again after every serverinfo refresh; until then those features are unknown. A host
    /// never reached stays unknown.
    pub fn with_native(&self, native: &NativeCapabilities) -> Self {
        let Some(mut evidence) = self.evidence.clone() else {
            return Self::UNKNOWN;
        };
        let features = &native.capabilities.features;
        let listed = |name: &str| features.iter().any(|f| f == name);
        evidence.native_capabilities = true;
        evidence.files_upload = listed("files.upload");
        evidence.files_download = listed("files.download");
        evidence.file_upload_permission = native.files.file_upload;
        evidence.file_download_permission = native.files.file_download;
        evidence.usb_offered = listed("usb");
        evidence.usb_permission = native.files.usb;
        evidence.image_clipboard_offered = listed("clipboard.image");
        evidence.folders_offered = listed("folders");
        evidence.folder_permission = native.files.shared_folders;
        Self::from_evidence(evidence)
    }

    pub fn is_known(&self) -> bool {
        self.evidence.is_some()
    }

    pub fn family(&self) -> Family {
        self.evidence
            .as_ref()
            .map_or(Family::Unknown, Evidence::family)
    }

    /// The Rubylight version the host reports, or None for other hosts.
    pub fn host_version(&self) -> Option<&str> {
        self.evidence.as_ref()?.rust_host_version.as_deref()
    }

    pub fn availability(&self, feature: Feature) -> Availability {
        self.evidence
            .as_ref()
            .map_or(Availability::Unknown, |e| e.availability(feature))
    }

    pub fn has(&self, feature: Feature) -> bool {
        self.availability(feature).is_supported()
    }

    /// Supported or not known yet: for behavior that does no harm on a host without it.
    pub fn may_have(&self, feature: Feature) -> bool {
        !matches!(self.availability(feature), Availability::Unsupported(_))
    }

    pub fn supported(&self) -> Vec<Feature> {
        Feature::ALL.into_iter().filter(|&f| self.has(f)).collect()
    }

    /// The codec support to negotiate with: PyroWave bits count only from a host that shows
    /// PyroWave.
    pub fn trusted_codec_mode_support(&self, server_codec_mode_support: u32) -> u32 {
        if self.has(Feature::PyroWave) {
            server_codec_mode_support
        } else {
            server_codec_mode_support & !PYROWAVE_CODEC_BITS
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rubylight_versions() {
        assert_eq!(
            parse_rubylight_version("2.2.21"),
            Some([2, 2, 21, u32::MAX])
        );
        assert_eq!(parse_rubylight_version("2.0.0-rc.19"), Some([2, 0, 0, 3_000_019]));
        assert_eq!(parse_rubylight_version("3.0.0-beta.1"), Some([3, 0, 0, 2_000_001]));
        assert_eq!(parse_rubylight_version("3.0.0-alpha.2+abc.1"), Some([3, 0, 0, 1_000_002]));
        // alpha < beta < rc < release, whatever the numbers.
        let ascending = ["3.0.0-alpha.1", "3.0.0-alpha.99", "3.0.0-beta.1", "3.0.0-beta.12", "3.0.0-rc.1", "3.0.0-rc.30", "3.0.0"];
        for pair in ascending.windows(2) {
            assert!(parse_rubylight_version(pair[0]).unwrap() < parse_rubylight_version(pair[1]).unwrap(), "{pair:?}");
        }
        assert_eq!(
            parse_rubylight_version("2.2.1+abc.1"),
            Some([2, 2, 1, u32::MAX])
        );
        for bad in ["", "2.2", "02.1.1", "2.2.1-rc.0", "2.2.1+", "2.2.1x", "3.0.0-beta", "3.0.0-beta.0", "3.0.0-gamma.1"] {
            assert_eq!(parse_rubylight_version(bad), None, "{bad}");
        }
        assert!(parse_rubylight_version("2.2.1").unwrap() >= HOST_DISPLAY_MODES_VERSION);
        assert!(parse_rubylight_version("2.2.1-rc.3").unwrap() < HOST_DISPLAY_MODES_VERSION);
    }

    #[test]
    fn unknown_hosts_rule_nothing_out() {
        let caps = HostCapabilities::UNKNOWN;
        assert_eq!(caps.family(), Family::Unknown);
        assert!(caps.may_have(Feature::RemoteSessionTiles));
        assert!(!caps.has(Feature::PyroWave));
        assert_eq!(caps.trusted_codec_mode_support(0x0180_0101), 0x0000_0101);
    }

    #[test]
    fn file_transfer_follows_the_native_features_and_permissions() {
        let cases = include_str!("../../tests/fixtures/files/files-capabilities.txt");
        let availability = |name: &str| match name {
            "supported" => Availability::Supported,
            "never" => Availability::Unsupported(None),
            "unknown" => Availability::Unknown,
            "pair" => Availability::Unsupported(Some(Reason::Pair)),
            "update_host" => Availability::Unsupported(Some(Reason::UpdateHost)),
            "file_permission" => Availability::Unsupported(Some(Reason::FilePermission)),
            other => panic!("unknown availability {other}"),
        };
        let (mut count, mut invalid) = (0, 0);
        for line in cases.lines().filter(|l| !l.is_empty() && !l.starts_with('#')) {
            let fields: Vec<&str> = line.splitn(4, '|').collect();
            if fields[0] == "invalid" {
                let parsed = serde_json::from_str::<super::super::native::NativeCapabilities>(fields[2]);
                assert!(parsed.is_err(), "{line}");
                invalid += 1;
                continue;
            }
            let mut caps = super::super::serverinfo::ServerInfo::parse(fields[3], 47989)
                .unwrap()
                .capabilities;
            // Serverinfo alone never decides file transfer on a host with the native API.
            for feature in [Feature::FileUpload, Feature::FileDownload] {
                assert_ne!(caps.availability(feature), Availability::Supported, "{line}");
            }
            if fields[2] != "-" {
                caps = caps.with_native(&serde_json::from_str(fields[2]).unwrap());
            }
            for (feature, expected) in [(Feature::FileUpload, fields[0]), (Feature::FileDownload, fields[1])] {
                let expected = availability(expected);
                assert_eq!(caps.availability(feature), expected, "{feature:?} {line}");
                assert_eq!(caps.supported().contains(&feature), expected.is_supported());
            }
            count += 1;
        }
        assert_eq!((count, invalid), (13, 15));
        // Without serverinfo there is nothing to add to.
        let native = serde_json::from_str(cases.lines().find(|l| l.starts_with("supported|supported|")).unwrap().split('|').nth(2).unwrap()).unwrap();
        assert_eq!(HostCapabilities::UNKNOWN.with_native(&native), HostCapabilities::UNKNOWN);
    }

    #[test]
    fn the_30_extensions_follow_their_serverinfo_bits() {
        let xml =
            include_str!("../../../../app/src/test/resources/protocol/rubylight-30-serverinfo.xml");
        let caps = |field: &str, value: &str| {
            let re = format!("<{field}>{value}</{field}>");
            let start = xml.find(&format!("<{field}>")).unwrap();
            let end = xml[start..].find(&format!("</{field}>")).unwrap() + start + field.len() + 3;
            let xml = format!("{}{re}{}", &xml[..start], &xml[end..]);
            super::super::serverinfo::ServerInfo::parse(&xml, 47989).unwrap().capabilities
        };
        let never = Availability::Unsupported(None);
        for (field, feature, cases) in [
            (
                "HdrFormats",
                Feature::DolbyVision,
                &[("4", true), ("8", true), ("3", false), ("x", false)][..],
            ),
            // 131 and 143 add the PyroWave HLG offer flag to HLG; 128 is the flag alone.
            (
                "HdrFormats",
                Feature::Hlg,
                &[("2", true), ("131", true), ("143", true), ("13", false), ("128", false), ("256", false)],
            ),
            (
                "AudioCodecs",
                Feature::AudioPassthrough,
                &[("2", true), ("4", true), ("8", true), ("1", false)],
            ),
            ("AudioLayouts", Feature::Surround714, &[("8", true), ("7", false)]),
            ("FecControlVersion", Feature::ClientFec, &[("2", true), ("0", false), ("-1", false)]),
            ("PipelineRatesVersion", Feature::PipelineRates, &[("1", true), ("", false)]),
        ] {
            for &(value, supported) in cases {
                let expected = if supported { Availability::Supported } else { never };
                assert_eq!(caps(field, value).availability(feature), expected, "{field} {value}");
            }
        }
        // Before the native API answered, its features are unknown, never supported.
        let caps = caps("HdrFormats", "15");
        for feature in [Feature::UsbForwarding, Feature::ImageClipboard, Feature::SharedFolders] {
            assert_eq!(caps.availability(feature), Availability::Unknown, "{feature:?}");
        }
        assert!(caps.supported().contains(&Feature::TextInput));
    }

    #[test]
    fn reading_the_native_capabilities_again_drops_what_the_host_stopped_offering() {
        let cases = include_str!("../../tests/fixtures/files/files-capabilities.txt");
        let fields: Vec<&str> = cases.lines().find(|l| l.starts_with("supported|supported|")).unwrap().splitn(4, '|').collect();
        let caps = super::super::serverinfo::ServerInfo::parse(fields[3], 47989)
            .unwrap()
            .capabilities;
        let offered = caps.with_native(&serde_json::from_str(fields[2]).unwrap());
        assert!(offered.has(Feature::FileUpload) && offered.has(Feature::FileDownload));
        let withdrawn = fields[2]
            .replace(r#","files.upload","files.download""#, "")
            .replace(r#","file_upload":true,"file_download":true"#, "");
        let withdrawn = offered.with_native(&serde_json::from_str(&withdrawn).unwrap());
        for feature in [Feature::FileUpload, Feature::FileDownload] {
            assert_eq!(withdrawn.availability(feature), Availability::Unsupported(None));
        }
    }
}
