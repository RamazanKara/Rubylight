//! What one host can do, judged only from its serverinfo reply. Port of Nightjar's
//! HostCapabilities.java: the family names the host for labels and protocol quirks; features
//! alone decide what a client offers.

mod parse;
mod types;

pub use parse::parse_rubylight_version;
pub(crate) use parse::video_packet_offer;
pub use types::*;

use super::native_types::NativeCapabilities;

/// A host's capabilities. [`HostCapabilities::UNKNOWN`] is a host never reached.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct HostCapabilities {
    evidence: Option<Evidence>,
}

const HOST_DISPLAY_MODES_VERSION: [u32; 4] = [2, 2, 1, u32::MAX];

impl Evidence {

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
            Feature::DisplayScale => {
                if !rubylight {
                    self.absent(false)
                } else if self.display_scale_version < 1 {
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
                    if rubylight { update } else { self.absent(true) }
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
            // A host before 7.0 lists only `FrameWarpVersion`: its Frame Warp took the eight-byte
            // request this client no longer sends, so the player is told to update the PC.
            Feature::FrameWarp => {
                if self.frame_warp_protocol >= 2 {
                    Supported
                } else if self.frame_warp_version >= 1 {
                    update
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
                // Without the field, a Sunshine-family host may still offer Dolby in its
                // DESCRIBE (`a=x-ss-audio-codec-offer`), which only the stream's handshake sees.
                if self.audio_codecs < 0
                    && matches!(
                        self.family(),
                        Family::Sunshine | Family::Apollo | Family::Vibepollo
                    )
                {
                    Unknown
                } else {
                    self.serverinfo_bits(self.audio_codecs, CODEC_PCM | CODEC_AC3 | CODEC_EAC3)
                }
            }
            Feature::Surround714 => self.serverinfo_bits(
                self.audio_layouts,
                rubylight_protocol::audio_ext::LAYOUT_MASK_714,
            ),
            Feature::TextInput => self.serverinfo_version(self.text_input_version),
            Feature::ClientKeyboardLayout => self.serverinfo_version(self.keyboard_layout_version),
            Feature::PenBarrelRoll => self.serverinfo_version(self.pen_barrel_roll_version),
            Feature::ControllerIdentity => {
                self.serverinfo_version(self.controller_identity_version)
            }
            Feature::PipelineRates => self.serverinfo_version(self.pipeline_rates_version),
            Feature::SecondScreen => self.serverinfo_version(self.second_screen_version),
            Feature::InternetPlay => self.serverinfo_version(self.internet_play_version),
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
            // No permission reason: a device without Join co-op still sees the sessions and
            // returns to its own slot; each session says why it cannot take a free one.
            Feature::Coop => self.native_feature(self.coop_offered, None),
            Feature::NeuralFrameGeneration => {
                let refused = (!self.view_permission).then_some(Reason::ViewPermission);
                self.native_feature(self.neural_offered, refused)
            }
            Feature::MicOnly => self.native_feature(self.mic_only_offered, None),
            Feature::ContinueHere => self.native_feature(self.continue_here_offered, None),
            Feature::PcRestart => self.native_feature(
                self.restart_offered,
                (!self.restart_shutdown_permission).then_some(Reason::RestartShutdownPermission),
            ),
            Feature::PcShutdown => self.native_feature(
                self.shutdown_offered,
                (!self.restart_shutdown_permission).then_some(Reason::RestartShutdownPermission),
            ),
            Feature::HostUpdate => self.native_feature(self.update_offered, None),
            Feature::PairingApprove => self.native_feature(
                self.pairing_offered,
                (!self.pairing_permission).then_some(Reason::PairingApprovePermission),
            ),
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
    pub fn from_server_info_xml(xml: &str) -> Result<Self, super::Error> {
        Ok(Self::from_evidence(Evidence::parse_xml(xml)?))
    }

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
    /// transfer, USB, image clipboard, shared folder and co-op features and permissions. Read it
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
        evidence.coop_offered = listed("coop");
        evidence.neural_offered = listed("neural.frame_generation");
        evidence.view_permission = native.capabilities.permissions.view;
        // Starting one takes the launch permission, so it is offered only with that.
        evidence.mic_only_offered =
            listed("session.mic_only") && native.capabilities.permissions.launch;
        // The host refuses a press without View and Launch.
        let permissions = &native.capabilities.permissions;
        evidence.continue_here_offered =
            listed("session.continue_here") && permissions.view && permissions.launch;
        evidence.restart_offered = listed("power.restart");
        evidence.shutdown_offered = listed("power.shutdown");
        evidence.restart_shutdown_permission = native.files.restart_shutdown;
        evidence.update_offered = listed("update") && permissions.host_control;
        evidence.pairing_offered = listed("pairing.approve");
        evidence.pairing_permission = native.files.pairing_approve || permissions.host_control;
        Self::from_evidence(evidence)
    }

    pub fn native_api_version(&self) -> i32 {
        self.evidence.as_ref().map_or(-1, |e| e.native_api_version)
    }

    pub fn hdr_formats(&self) -> i32 {
        if self.has(Feature::DolbyVision) || self.has(Feature::Hlg) {
            self.evidence.as_ref().map_or(0, |e| {
                e.hdr_formats
                    & i32::from(
                        rubylight_protocol::hdr::ALL | rubylight_protocol::hdr::OFFER_FLAGS,
                    )
            })
        } else {
            0
        }
    }

    pub fn native_features(&self) -> u32 {
        (u32::from(self.has(Feature::PyroWaveRecords)))
            | (u32::from(self.has(Feature::ControllerIdentity)) << 1)
    }

    pub fn offers_enhancement(&self, bit: u8) -> bool {
        (self.nvidia_enhancements() & bit) != 0
    }

    pub fn enhancement_parameters(&self, rtx_hdr: Option<bool>, rtx_video: Option<bool>) -> String {
        let mut out = String::new();
        for (k, v) in rubylight_protocol::nvidia_enhance::launch_parameters(
            self.nvidia_enhancements(),
            rtx_hdr,
            rtx_video,
        ) {
            out.push('&');
            out.push_str(k);
            out.push('=');
            out.push_str(v);
        }
        out
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

    /// Whether the host grants at least one bit in `mask` (or reports no permissions at all).
    pub fn has_permission(&self, mask: u32) -> bool {
        self.evidence
            .as_ref()
            .is_none_or(|e| e.has_permission(mask))
    }

    /// The image enhancements the host offers to turn on or off per stream
    /// (`rubylight_protocol::nvidia_enhance` bits), 0 for a host without any.
    pub fn nvidia_enhancements(&self) -> u8 {
        self.evidence.as_ref().map_or(0, |e| e.nvidia_enhancements)
    }

    /// Whether the host takes `rlHdrDynamic` to leave HDR10+ and HDR Vivid out of a stream.
    pub fn hdr_dynamic_control(&self) -> bool {
        self.evidence
            .as_ref()
            .is_some_and(|e| e.hdr_dynamic_control)
    }

    /// `MaxVideoPacket`: the largest video packet this host accepts, including a client's 32-byte
    /// encryption header. 0 when it offers no more than stock Moonlight's 1400 bytes; a stream's
    /// DESCRIBE offer may be smaller still.
    pub fn max_video_packet(&self) -> u16 {
        self.evidence.as_ref().map_or(0, |e| e.max_video_packet)
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
#[path = "capabilities_tests.rs"]
mod tests;
