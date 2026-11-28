//! What one host can do, judged only from its serverinfo reply. Port of Nightjar's
//! HostCapabilities.java: the family names the host for labels and protocol quirks; features
//! alone decide what a client offers.

mod availability;
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

    pub(super) fn can_view(&self) -> bool {
        self.has_permission(PERMISSION_VIEW | PERMISSION_LAUNCH)
    }

    /// Rubylight, or the extension fields that Vibepollo added and Rubylight also sends.
    pub(super) fn vibepollo_extensions(&self) -> bool {
        self.rust_host_version.is_some() || self.frame_limiter.is_some()
    }

    /// No evidence: an unrecognized host rules nothing out; Apollo and Vibepollo hide some
    /// fields from unpaired clients.
    pub(super) fn absent(&self, hidden_until_paired: bool) -> Availability {
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

    pub(super) fn rubylight_version(&self) -> Option<[u32; 4]> {
        self.rust_host_version
            .as_deref()
            .and_then(parse_rubylight_version)
    }

    pub(super) fn virtual_display(&self) -> Availability {
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
        // Pausing or resuming takes Launch or Host control.
        evidence.quick_suspend_offered = listed(rubylight_native_api::feature::QUICK_SUSPEND)
            && (permissions.launch || permissions.host_control);
        evidence.stream_dvr_offered = listed(rubylight_native_api::feature::STREAM_DVR);
        evidence.wake_relay_offered =
            listed("wake.relay") && (permissions.launch || permissions.host_control);
        // Reading it takes View or Launch; a remedy takes Launch, which the host checks.
        evidence.stream_doctor_offered = listed(rubylight_native_api::feature::STREAM_DOCTOR_V2)
            && (permissions.view || permissions.launch);
        evidence.launch_curtain_offered = listed(rubylight_native_api::feature::LAUNCH_CURTAIN);
        evidence.hub_sync_offered = listed(rubylight_native_api::feature::HUB_SYNC);
        Self::from_evidence(evidence)
    }

    pub fn native_api_version(&self) -> i32 {
        self.evidence.as_ref().map_or(-1, |e| e.native_api_version)
    }

    pub fn hdr_formats(&self) -> i32 {
        if self.has(Feature::DolbyVision) || self.has(Feature::Hlg) {
            self.evidence.as_ref().map_or(0, |e| {
                e.hdr_formats
                    & i32::from(rubylight_protocol::hdr::ALL | rubylight_protocol::hdr::OFFER_FLAGS)
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
