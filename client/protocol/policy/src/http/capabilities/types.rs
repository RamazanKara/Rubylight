//! Core capability enums, permission masks, and parsed serverinfo [`Evidence`].

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
    /// The device sets its virtual display's Windows scale (`rlDisplayScale`,
    /// `DisplayScaleVersion`); older Rubylight hosts ignore it.
    DisplayScale,
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
    /// Frame Warp (0x5543 to 0x5546): the host learns each game's camera response and tags
    /// frames, so the client can shift pictures by input still in flight.
    FrameWarp,
    /// The Rubylight native API (`/api/native/v1`): library, session actions, Radeon state.
    NativeApi,
    /// Sleep PC (`/power-actions/v1`, Vibepollo's wire format): suspend an idle host.
    Sleep,
    /// Upload files to the host (`/api/native/v1/files`). Known only after
    /// [`super::HostCapabilities::with_native`]: gate the UI on [`super::HostCapabilities::has`],
    /// never `may_have`, which is true before the native API answered.
    FileUpload,
    /// List and download the host's files. Known only after [`super::HostCapabilities::with_native`];
    /// gate it like `FileUpload`.
    FileDownload,
    /// Dolby Vision profile 8.1, 8.4 or 10.1 (`HdrFormats`, `rubylight_protocol::hdr`).
    DolbyVision,
    /// HLG HDR (`HdrFormats`).
    Hlg,
    /// The client picks the video FEC (`rlFecPercentage`) and sends FEC reports (0x5539).
    ClientFec,
    /// `GET /bandwidth-probe` (`BandwidthProbeVersion`).
    BandwidthProbe,
    /// USB device forwarding (native `usb`). Known only after [`super::HostCapabilities::with_native`].
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
    /// Controller identities (input 0x55000080) and the C button: the host
    /// can give a Switch 2 Pro Controller its own pad.
    ControllerIdentity,
    /// Pipeline frame rates for the overlay (0x553D).
    PipelineRates,
    /// Dual screen mode (`SecondScreenVersion`): the host keeps a device's second stream
    /// (`rlSecondScreen=1`) beside its first. Any other host takes it for the first stream
    /// starting again and drops that one.
    SecondScreen,
    /// PNG clipboard (native `clipboard.image`). Known only after
    /// [`super::HostCapabilities::with_native`].
    ImageClipboard,
    /// Shared folders (native `folders`). Known only after [`super::HostCapabilities::with_native`].
    SharedFolders,
    /// Nucleus Co-op sessions to list and join (native `coop`). Known only after
    /// [`super::HostCapabilities::with_native`]. Whether this device may take a free slot is per session.
    Coop,
    /// A game's frame generation, turned from this device (native `neural.frame_generation`).
    /// Known only after [`super::HostCapabilities::with_native`]; reading needs View streams, a write
    /// Host control too, which the host's answer says.
    NeuralFrameGeneration,
    /// Microphone-only sessions (native `session.mic_only`): the PC hears this device's
    /// microphone and nothing else. Known only after [`super::HostCapabilities::with_native`], and
    /// only for a device with the launch permission.
    MicOnly,
    /// Moving the running game's stream to this device (`POST /session/continue-here`).
    ContinueHere,
    /// Play over the internet (`RustHostInternetPlayVersion`): the host publishes rendezvous
    /// records for its paired devices and answers hole punches (`/internet-play/v1`).
    InternetPlay,
    /// Restart PC (`power.restart` in native features). Known only after [`super::HostCapabilities::with_native`].
    PcRestart,
    /// Shut down PC (`power.shutdown` in native features). Known only after [`super::HostCapabilities::with_native`].
    PcShutdown,
    /// Install the host's pending update (`update` in native features, with Host control).
    HostUpdate,
    /// Devices waiting to pair, approved from here (`pairing.approve` in native features).
    PairingApprove,
    /// Frame Warp 2 (`FrameWarpVersion` 2 or more): a version 2 request, the gyro, and the
    /// per-frame confidence and fill hints (0x5549, `rubylight_protocol::frame_warp_v2`).
    FrameWarpV2,
}

impl Feature {
    pub const ALL: [Feature; 55] = [
        Feature::Permissions,
        Feature::OtpPairing,
        Feature::Clipboard,
        Feature::ServerCommands,
        Feature::AppUuids,
        Feature::LaunchByUuid,
        Feature::VirtualDisplay,
        Feature::VirtualDisplayHdr,
        Feature::HostDisplayModes,
        Feature::DisplayScale,
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
        Feature::FrameWarp,
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
        Feature::ControllerIdentity,
        Feature::PipelineRates,
        Feature::SecondScreen,
        Feature::ImageClipboard,
        Feature::SharedFolders,
        Feature::Coop,
        Feature::NeuralFrameGeneration,
        Feature::MicOnly,
        Feature::ContinueHere,
        Feature::InternetPlay,
        Feature::PcRestart,
        Feature::PcShutdown,
        Feature::HostUpdate,
        Feature::PairingApprove,
        Feature::FrameWarpV2,
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
    /// The host supports restart/shutdown but has not granted it to this paired client.
    RestartShutdownPermission,
    /// The host offers pairing approval but has not granted it to this paired client.
    PairingApprovePermission,
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
    pub(super) const NEVER: Availability = Availability::Unsupported(None);

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
    pub frame_warp_version: i32,
    /// -1 when the host has no native API, 0 when it has one but not for this request (plain
    /// HTTP or an unpaired certificate), 1 or more for the highest version it serves.
    pub native_api_version: i32,
    /// -1 when serverinfo has no `PowerActionVersion`: hosts send it only to paired clients
    /// over HTTPS.
    pub power_action_version: i32,
    pub sleep_supported: bool,
    /// The host lets this paired client put it to sleep.
    pub sleep_allowed: bool,
    /// The native API's capabilities were read (see [`super::HostCapabilities::with_native`]); the
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
    pub controller_identity_version: i32,
    pub pipeline_rates_version: i32,
    /// -1 without `DisplayScaleVersion`: the host ignores `rlDisplayScale`.
    pub display_scale_version: i32,
    /// -1 without `SecondScreenVersion`: the host ends the first stream when a second one of the
    /// device starts.
    pub second_screen_version: i32,
    /// The image enhancements the host runs before encoding (`NvidiaEnhancements`,
    /// `rubylight_protocol::nvidia_enhance` bits); 0 for none.
    pub nvidia_enhancements: u8,
    /// -1 without `RustHostInternetPlayVersion` (paired serverinfo only), 0 when the host has
    /// Play over the internet off.
    pub internet_play_version: i32,
    /// `usb`, `clipboard.image` and `folders` in the native features, and the device's `usb`
    /// and `shared_folders` permissions.
    pub usb_offered: bool,
    pub usb_permission: bool,
    pub image_clipboard_offered: bool,
    pub folders_offered: bool,
    pub folder_permission: bool,
    /// `coop` in the native features.
    pub coop_offered: bool,
    /// `neural.frame_generation` in the native features.
    pub neural_offered: bool,
    /// The View streams permission, which reading a game's switches needs.
    pub view_permission: bool,
    /// `session.mic_only` in the native features, with the launch permission.
    pub mic_only_offered: bool,
    pub continue_here_offered: bool,
    /// `power.restart` and `power.shutdown` in native features, and `restart_shutdown` grant.
    pub restart_offered: bool,
    pub shutdown_offered: bool,
    pub restart_shutdown_permission: bool,
    /// `update` in native features, with Host control.
    pub update_offered: bool,
    /// `pairing.approve` in native features, and `pairing_approve` or Host control grant.
    pub pairing_offered: bool,
    pub pairing_permission: bool,
    /// `MaxVideoPacket`: the largest video packet this host accepts, including a client's
    /// 32-byte encryption header. 0 when the host offers no more than stock Moonlight's 1400.
    pub max_video_packet: u16,
}
