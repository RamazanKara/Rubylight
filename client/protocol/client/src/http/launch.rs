//! The `/launch` and `/resume` query, with each extension only for a host that shows it
//! (NvHTTP.getLaunchQuery), and the reply.

use super::apps::AppRole;
use super::capabilities::{Family, Feature, HostCapabilities};
use super::luminance::DisplayLuminance;
use super::transport::encode_query;
use super::xml;
use super::{der, Error};
use crate::sdp::AudioConfiguration;
use rubylight_protocol::clipboard_sync;
use rubylight_protocol::fec_report::{FecChoice, LAUNCH_PARAMETER};
use rubylight_protocol::rate_change;

/// What the PC's displays do during the stream (Rubylight's `hostDisplay`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum HostDisplay {
    /// The host's own settings decide; nothing is sent.
    #[default]
    HostDefault,
    Exclusive,
    ExtendedPrimary,
    Extended,
    ExtendedIsolated,
    ExtendedPrimaryIsolated,
    Physical,
}

impl HostDisplay {
    pub fn wire_value(self) -> Option<&'static str> {
        match self {
            Self::HostDefault => None,
            Self::Exclusive => Some("exclusive"),
            Self::ExtendedPrimary => Some("extended_primary"),
            Self::Extended => Some("extended"),
            Self::ExtendedIsolated => Some("extended_isolated"),
            Self::ExtendedPrimaryIsolated => Some("extended_primary_isolated"),
            Self::Physical => Some("physical"),
        }
    }

    /// Only a stream chooses; Remote Monitor and Remote Input keep the host's settings.
    pub fn for_role(self, role: AppRole) -> Self {
        if role == AppRole::Stream {
            self
        } else {
            Self::HostDefault
        }
    }

    /// Whether the launch asks for a virtual display; the host default keeps the setting.
    pub fn requests_virtual_display(self, virtual_display_setting: bool) -> bool {
        match self {
            Self::HostDefault => virtual_display_setting,
            Self::Physical => false,
            _ => true,
        }
    }
}

/// Everything Nightjar sends to start or resume a session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaunchRequest {
    pub app_id: i32,
    pub app_uuid: String,
    pub role: AppRole,
    /// Input-only by streaming the game at a small size (hosts without an input-only mode).
    pub small_stream_input_only: bool,
    /// An input-only session that carries only this device's microphone (`rlMicOnly=1`), for a
    /// host that lists [`Feature::MicOnly`].
    pub mic_only: bool,
    pub width: u32,
    pub height: u32,
    /// The integer launch rate.
    pub fps: u32,
    /// An exact rate such as 5994 for 59.94 Hz, sent to hosts that read fractions; 0 for none.
    pub launch_refresh_rate_x100: u32,
    /// The display's rate x100; used as the fractional rate when it rounds to `fps`.
    pub client_refresh_rate_x100: u32,
    /// The "Host virtual display" setting.
    pub virtual_display: bool,
    /// The PC's render resolution, in percent of the stream resolution (`scaleFactor`): the
    /// virtual display is made this much larger or smaller and scaled to the stream size.
    pub virtual_display_scale: u32,
    /// The Windows scale (text and interface size) of the virtual display, in percent
    /// (`rlDisplayScale`); 0 keeps the PC's own. Sent only to hosts with
    /// [`Feature::DisplayScale`].
    pub virtual_display_dpi: u32,
    pub host_display: HostDisplay,
    pub vrr: bool,
    pub sops: bool,
    pub play_local_audio: bool,
    pub hdr: bool,
    /// The stream may negotiate a 10-bit format for SDR: with `hdr` off this sends `hdrMode=0`.
    /// The protocol has no flag for 10-bit SDR, and a host reads a 10-bit format without it as
    /// HDR, as stock clients mean it.
    pub ten_bit_sdr: bool,
    pub audio: AudioConfiguration,
    /// The remote input key and ID; the same values go into the stream configuration.
    pub ri_key: [u8; 16],
    pub ri_key_id: i32,
    /// Rubylight's prepared controller list (`rlControllers`), empty for none.
    pub prepared_controllers: String,
    pub gamepad_mask: u32,
    pub persist_gamepads: bool,
    /// The video FEC to ask for (`rlFecPercentage`); `None`, and any host without
    /// `Feature::ClientFec`, keep the host's own.
    pub fec: Option<FecChoice>,
    /// Asks for mid-stream rate changes (`rlRateChange=1`, control message 0x553C). No host
    /// offers them before the launch, and every host ignores a parameter it does not know,
    /// so this goes to any host; the stream's DESCRIBE says whether it took it.
    pub rate_change: bool,
    /// The user confirmed closing the running app for this launch ([`CONFIRM_REPLACE`]).
    pub confirm_replace: bool,
    /// Asks a Rubylight host for clipboard sync (`rlClipboardSync=1`); it offers 0x5508 on
    /// DESCRIBE when the device's clipboard permissions allow it.
    pub clipboard_sync: bool,
    /// The display's luminance, sent at launch to hosts other than Rubylight (which gets it in
    /// the DisplayCaps control message); `None` leaves it to the host.
    pub display_luminance: Option<DisplayLuminance>,
    /// RTX HDR for this stream (`rlRtxHdr`): `Some` turns it on or off over the app's choice,
    /// `None` follows the app. Sent only for an enhancement the host offers
    /// ([`HostCapabilities::nvidia_enhancements`]).
    pub rtx_hdr: Option<bool>,
    /// RTX Video enhancement for this stream (`rlRtxVideo`), as `rtx_hdr`.
    pub rtx_video: Option<bool>,
}

impl Default for LaunchRequest {
    fn default() -> Self {
        Self {
            app_id: 0,
            app_uuid: String::new(),
            role: AppRole::Stream,
            small_stream_input_only: false,
            mic_only: false,
            width: 1920,
            height: 1080,
            fps: 60,
            launch_refresh_rate_x100: 0,
            client_refresh_rate_x100: 0,
            virtual_display: false,
            virtual_display_scale: 100,
            virtual_display_dpi: 0,
            host_display: HostDisplay::HostDefault,
            vrr: false,
            sops: true,
            play_local_audio: false,
            hdr: false,
            ten_bit_sdr: false,
            audio: AudioConfiguration::STEREO,
            ri_key: [0; 16],
            ri_key_id: 0,
            prepared_controllers: String::new(),
            gamepad_mask: 0,
            persist_gamepads: false,
            fec: None,
            rate_change: false,
            confirm_replace: false,
            clipboard_sync: false,
            display_luminance: None,
            rtx_hdr: None,
            rtx_video: None,
        }
    }
}

/// moonlight-common-c's LiGetLaunchUrlQueryParameters: video and control encryption (v0) and
/// encrypted RTSP (v1).
pub const CORE_LAUNCH_PARAMETERS: &str = "&corever=1";

/// Rubylight's launch parameter: 1 on the launch that answers a confirmed replace prompt, 0 on
/// every other, so a remembered launch or a retry never closes the running app by itself.
pub const CONFIRM_REPLACE: &str = "rlConfirmReplace";

/// The 410 a host that confirms replacement (Rubylight, Vibepollo) refuses a launch with while
/// another app runs; any other 410 is not a prompt.
const REPLACE_PROMPT: &str = "An app is already running. Launch this app again within 60 seconds to confirm that you want to close it.";

/// Whether a refused launch asks the user to confirm closing the running app.
pub fn asks_to_confirm_replace(error: &Error) -> bool {
    matches!(error, Error::Status { code: 410, message } if message == REPLACE_PROMPT)
}

impl LaunchRequest {
    fn host_display_choice(&self, caps: &HostCapabilities) -> HostDisplay {
        if caps.has(Feature::HostDisplayModes) {
            self.host_display.for_role(self.role)
        } else {
            HostDisplay::HostDefault
        }
    }

    /// Whether this stream asks for a virtual display, whether or not the host has one.
    pub fn wants_virtual_display(&self, caps: &HostCapabilities) -> bool {
        self.role != AppRole::InputOnly
            && self
                .host_display_choice(caps)
                .requests_virtual_display(self.virtual_display)
    }

    fn fractional_rate_x100(&self, caps: &HostCapabilities, launch_fps: u32) -> u32 {
        if !caps.has(Feature::FractionalRefresh) {
            return 0;
        }
        let explicit = self.launch_refresh_rate_x100;
        if (100..=100_000).contains(&explicit) {
            return explicit;
        }
        let display = self.client_refresh_rate_x100;
        if (100..=100_000).contains(&display) && (display + 50) / 100 == launch_fps {
            display
        } else {
            0
        }
    }

    /// The exact stream cadence in mHz, without overriding a deliberately lower capped rate.
    pub fn stream_frame_rate_millihz(&self, caps: &HostCapabilities, stream_fps: u32) -> u32 {
        let fractional = self.fractional_rate_x100(caps, self.fps);
        if fractional > 0 && (fractional + 50) / 100 == stream_fps {
            fractional * 10
        } else {
            stream_fps * 1000
        }
    }

    /// The query for `/launch` or `/resume`, including [`CORE_LAUNCH_PARAMETERS`].
    /// `nvidia` is GameStream (ServerInfo::nvidia_server).
    pub fn query(&self, caps: &HostCapabilities, nvidia: bool) -> String {
        let input_only = self.role == AppRole::InputOnly;
        let gamepads = if self.role == AppRole::RemoteMonitor {
            0
        } else {
            self.gamepad_mask
        };
        // GFE falls back to 720p60 for SOPS when asked for more than 60 FPS.
        let fps = if nvidia && self.fps > 60 { 0 } else { self.fps };
        // Sunshine reads the rate as an integer and would truncate 59.94 to 59.
        let fractional = self.fractional_rate_x100(caps, fps);
        let rate = if fractional > 0 {
            format!("{}.{:02}", fractional / 100, fractional % 100)
        } else {
            fps.to_string()
        };
        let host_display = self.host_display_choice(caps);
        let virtual_display = caps.has(Feature::VirtualDisplay) && self.wants_virtual_display(caps);
        let vrr = if input_only || !self.vrr || !caps.has(Feature::HostVrr) {
            ""
        } else if caps.family() == Family::Rubylight {
            // Older Rubylight versions read only vrr; Vibepollo reads clientVrrRequested.
            "&clientVrrRequested=1&vrr=1"
        } else {
            "&clientVrrRequested=1"
        };
        let mut sops = self.sops;
        if nvidia {
            // GFE forces SOPS to 720p60 for resolutions other than 720p, 1080p and 4K.
            let pixels = self.width as u64 * self.height as u64;
            if pixels > 1280 * 720 && pixels != 1920 * 1080 && pixels != 3840 * 2160 {
                sops = false;
            }
        }

        let mut q = format!("appid={}", self.app_id);
        if !self.app_uuid.is_empty() {
            q.push_str(&format!("&appuuid={}", encode_query(&self.app_uuid)));
        }
        if self.role == AppRole::RemoteMonitor {
            q.push_str("&remote_monitor=1");
        } else if input_only && !self.small_stream_input_only {
            q.push_str("&input_only=1");
            if self.mic_only {
                q.push_str("&rlMicOnly=1");
            }
        }
        q.push_str(&format!("&mode={}x{}x{rate}", self.width, self.height));
        if virtual_display {
            q.push_str(&format!(
                "&virtualDisplay=1&scaleFactor={}",
                self.virtual_display_scale
            ));
        }
        // Also without virtualDisplay=1: the host makes a virtual display of its own by default
        // and ignores the scale for a stream on a physical one.
        if !input_only && self.virtual_display_dpi != 0 && caps.has(Feature::DisplayScale) {
            q.push_str(&format!("&rlDisplayScale={}", self.virtual_display_dpi));
        }
        if let Some(value) = host_display.wire_value() {
            q.push_str(&format!("&hostDisplay={value}"));
        }
        q.push_str(vrr);
        q.push_str(&format!("&additionalStates=1&sops={}", u8::from(sops)));
        q.push_str(&format!(
            "&rikey={}&rikeyid={}",
            der::hex_encode(&self.ri_key),
            self.ri_key_id
        ));
        if self.hdr && !input_only {
            q.push_str("&hdrMode=1&clientHdrCapVersion=0&clientHdrCapSupportedFlagsInUint32=0&clientHdrCapMetaDataId=NV_STATIC_METADATA_TYPE_1&clientHdrCapDisplayData=0x0x0x0x0x0x0x0x0x0x0");
        } else if self.ten_bit_sdr && !input_only {
            q.push_str("&hdrMode=0");
        }
        q.push_str(&format!(
            "&localAudioPlayMode={}",
            u8::from(self.play_local_audio)
        ));
        q.push_str(&format!(
            "&surroundAudioInfo={}",
            self.audio.surround_audio_info()
        ));
        if !self.prepared_controllers.is_empty() {
            q.push_str(&format!("&rlControllers={}", self.prepared_controllers));
        }
        q.push_str(&format!(
            "&remoteControllersBitmap={gamepads}&gcmap={gamepads}"
        ));
        let persist = self.role != AppRole::RemoteMonitor && self.persist_gamepads;
        q.push_str(&format!("&gcpersist={}", u8::from(persist)));
        // The display's luminance as Moonlight V+ sends it to Sunshine-family hosts; a Rubylight
        // host gets it in the DisplayCaps control message once the stream runs.
        if let Some(luminance) = self.display_luminance.filter(|_| !input_only && caps.family() != Family::Rubylight) {
            q.push_str(&luminance.query_parameters());
        }
        if caps.family() == Family::Rubylight {
            q.push_str(&format!(
                "&{CONFIRM_REPLACE}={}",
                u8::from(self.confirm_replace)
            ));
        }
        if let Some(fec) = self.fec.filter(|_| caps.has(Feature::ClientFec)) {
            q.push_str(&format!("&{LAUNCH_PARAMETER}={}", fec.as_param()));
        }
        if self.clipboard_sync && caps.family() == Family::Rubylight {
            q.push_str(&format!("&{}=1", clipboard_sync::LAUNCH_PARAMETER));
        }
        if self.rate_change {
            q.push_str(&format!("&{}=1", rate_change::LAUNCH_PARAMETER));
        }
        if !input_only {
            for (name, value) in rubylight_protocol::nvidia_enhance::launch_parameters(
                caps.nvidia_enhancements(),
                self.rtx_hdr,
                self.rtx_video,
            ) {
                q.push_str(&format!("&{name}={value}"));
            }
        }
        q.push_str(CORE_LAUNCH_PARAMETERS);
        q
    }
}

/// A launch or resume the host accepted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Launched {
    /// `sessionUrl0`: the RTSP URL (rtsp:// or rtspenc://); missing on old GFE.
    pub rtsp_session_url: Option<String>,
}

/// Reads a launch (`resume` false) or resume reply; None when the host answered 0.
pub fn parse_launch_reply(text: &str, resume: bool) -> Result<Option<Launched>, Error> {
    let root = xml::parse_reply(text)?;
    let field = if resume { "resume" } else { "gamesession" };
    if root.required_text(field)? == "0" {
        return Ok(None);
    }
    Ok(Some(Launched {
        rtsp_session_url: root.find_text("sessionUrl0"),
    }))
}

#[cfg(test)]
mod tests;
