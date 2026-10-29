//! The `/launch` and `/resume` query, with each extension only for a host that shows it
//! (NvHTTP.getLaunchQuery), and the reply.

use super::apps::AppRole;
use super::capabilities::{Family, Feature, HostCapabilities};
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
}

impl Default for LaunchRequest {
    fn default() -> Self {
        Self {
            app_id: 0,
            app_uuid: String::new(),
            role: AppRole::Stream,
            small_stream_input_only: false,
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
        }
        q.push_str(&format!("&mode={}x{}x{rate}", self.width, self.height));
        if virtual_display {
            q.push_str(&format!(
                "&virtualDisplay=1&scaleFactor={}",
                self.virtual_display_scale
            ));
            if self.virtual_display_dpi != 0 && caps.has(Feature::DisplayScale) {
                q.push_str(&format!("&rlDisplayScale={}", self.virtual_display_dpi));
            }
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
mod tests {
    use super::super::capabilities::{Availability, Reason};
    use super::super::serverinfo::{tests::RUBYLIGHT, ServerInfo};
    use super::*;

    fn rubylight() -> HostCapabilities {
        rubylight_host(true)
    }

    /// The Rubylight fixture, with or without `DisplayScaleVersion`.
    fn rubylight_host(display_scale: bool) -> HostCapabilities {
        // The fixture's virtual display driver is not ready; make it ready.
        let mut xml = RUBYLIGHT.replace(
            "<VirtualDisplayDriverReady>false</VirtualDisplayDriverReady>",
            "<VirtualDisplayDriverReady>true</VirtualDisplayDriverReady>",
        );
        if display_scale {
            xml = xml.replace("</root>", "<DisplayScaleVersion>1</DisplayScaleVersion></root>");
        }
        ServerInfo::parse(&xml, 47989).unwrap().capabilities
    }

    fn request() -> LaunchRequest {
        LaunchRequest {
            app_id: 881_448_767,
            ri_key: core::array::from_fn(|i| i as u8),
            ri_key_id: -2_147_483_525,
            ..Default::default()
        }
    }

    const KEY: &str = "000102030405060708090A0B0C0D0E0F";

    #[test]
    fn plain_sunshine_launch() {
        let caps = HostCapabilities::UNKNOWN;
        assert_eq!(
            request().query(&caps, false),
            format!(
                "appid=881448767&mode=1920x1080x60&additionalStates=1&sops=1&rikey={KEY}&rikeyid=-2147483525\
                 &localAudioPlayMode=0&surroundAudioInfo=196610&remoteControllersBitmap=0&gcmap=0&gcpersist=0&corever=1"
            )
        );
    }

    #[test]
    fn ten_bit_sdr_says_hdr_mode_zero_and_hdr_still_wins() {
        // Without hdrMode=0 a host streams the 10-bit format as HDR (hardware lane 4a: every
        // 10-bit SDR stream to desktop Nightjar ended at once on an SDR display).
        for caps in [HostCapabilities::UNKNOWN, rubylight()] {
            let sdr = LaunchRequest {
                ten_bit_sdr: true,
                ..request()
            };
            assert_eq!(
                sdr.query(&caps, false),
                request()
                    .query(&caps, false)
                    .replace("&localAudioPlayMode=", "&hdrMode=0&localAudioPlayMode=")
            );
            let hdr = LaunchRequest { hdr: true, ..sdr.clone() };
            let q = hdr.query(&caps, false);
            assert!(q.contains("&hdrMode=1&") && !q.contains("hdrMode=0"), "{q}");
            let input = LaunchRequest {
                role: AppRole::InputOnly,
                ..sdr
            };
            assert!(!input.query(&caps, false).contains("hdrMode"));
        }
        assert!(!request().query(&rubylight(), false).contains("hdrMode"));
    }

    #[test]
    fn the_display_scale_goes_only_with_a_virtual_display_and_only_when_chosen() {
        let query = |virtual_display, dpi| {
            LaunchRequest { virtual_display, virtual_display_dpi: dpi, ..request() }.query(&rubylight(), false)
        };
        assert!(!query(true, 0).contains("rlDisplayScale"), "0 keeps the PC's own scale");
        assert!(query(true, 200).contains("&scaleFactor=100&rlDisplayScale=200"), "the render resolution stays 100 %");
        assert!(!query(false, 200).contains("rlDisplayScale"), "no virtual display, nothing to scale");
    }

    #[test]
    fn hosts_without_display_scale_version_get_no_display_scale() {
        // Older Rubylight hosts (2.6.x, 3.0.0-rc.4) ignore rlDisplayScale.
        let request = LaunchRequest { virtual_display: true, virtual_display_dpi: 200, ..request() };
        let older = rubylight_host(false);
        assert_eq!(
            older.availability(Feature::DisplayScale),
            Availability::Unsupported(Some(Reason::UpdateHost))
        );
        let query = request.query(&older, false);
        assert!(query.contains("&virtualDisplay=1&scaleFactor=100&"), "{query}");
        assert!(!query.contains("rlDisplayScale"), "{query}");
        assert!(request.query(&rubylight_host(true), false).contains("&rlDisplayScale=200&"));
    }

    #[test]
    fn rubylight_launch_with_every_extension() {
        let request = LaunchRequest {
            app_uuid: "u 1".into(),
            width: 2560,
            height: 1440,
            fps: 60,
            client_refresh_rate_x100: 5994,
            virtual_display: true,
            virtual_display_scale: 150,
            virtual_display_dpi: 175,
            host_display: HostDisplay::ExtendedPrimary,
            vrr: true,
            hdr: true,
            play_local_audio: true,
            audio: AudioConfiguration::SURROUND_71,
            prepared_controllers: "0:dualsense".into(),
            gamepad_mask: 3,
            persist_gamepads: true,
            ..request()
        };
        assert_eq!(
            request.query(&rubylight(), false),
            format!(
                "appid=881448767&appuuid=u%201&mode=2560x1440x59.94&virtualDisplay=1&scaleFactor=150\
                 &rlDisplayScale=175&hostDisplay=extended_primary&clientVrrRequested=1&vrr=1&additionalStates=1&sops=1\
                 &rikey={KEY}&rikeyid=-2147483525&hdrMode=1&clientHdrCapVersion=0\
                 &clientHdrCapSupportedFlagsInUint32=0&clientHdrCapMetaDataId=NV_STATIC_METADATA_TYPE_1\
                 &clientHdrCapDisplayData=0x0x0x0x0x0x0x0x0x0x0&localAudioPlayMode=1\
                 &surroundAudioInfo=104792072&rlControllers=0:dualsense&remoteControllersBitmap=3&gcmap=3\
                 &gcpersist=1&rlConfirmReplace=0&corever=1"
            )
        );
        assert_eq!(request.stream_frame_rate_millihz(&rubylight(), 60), 59_940);
        assert_eq!(request.stream_frame_rate_millihz(&rubylight(), 30), 30_000);
    }

    #[test]
    fn clipboard_sync_is_asked_only_of_a_rubylight_host() {
        let asked = LaunchRequest {
            clipboard_sync: true,
            ..request()
        };
        assert_eq!(
            asked.query(&rubylight(), false),
            request()
                .query(&rubylight(), false)
                .replace("&corever=1", "&rlClipboardSync=1&corever=1")
        );
        // Other hosts get exactly today's query.
        let caps = HostCapabilities::UNKNOWN;
        assert_eq!(asked.query(&caps, false), request().query(&caps, false));
        assert!(!request()
            .query(&rubylight(), false)
            .contains("rlClipboardSync"));
    }

    #[test]
    fn the_fec_choice_goes_only_to_a_host_that_takes_it() {
        let xml = RUBYLIGHT.replace("</root>", "<FecControlVersion>1</FecControlVersion></root>");
        assert_ne!(xml, RUBYLIGHT);
        let fec_host = ServerInfo::parse(&xml, 47989).unwrap().capabilities;
        assert!(fec_host.has(Feature::ClientFec));
        let plain = request().query(&fec_host, false);
        assert!(!plain.contains("rlFecPercentage"), "no choice, no parameter");
        for (choice, value) in [
            (FecChoice::Host, "host"),
            (FecChoice::Auto, "auto"),
            (FecChoice::Fixed(0), "0"),
            (FecChoice::Fixed(255), "255"),
        ] {
            let asked = LaunchRequest {
                fec: Some(choice),
                ..request()
            };
            assert_eq!(
                asked.query(&fec_host, false),
                plain.replace("&corever=1", &format!("&rlFecPercentage={value}&corever=1"))
            );
            // Hosts without the extension get exactly today's query.
            for caps in [HostCapabilities::UNKNOWN, rubylight()] {
                assert_eq!(asked.query(&caps, false), request().query(&caps, false));
            }
        }
    }

    #[test]
    fn rate_changes_are_asked_for_at_launch_only_when_wanted() {
        for caps in [HostCapabilities::UNKNOWN, rubylight()] {
            let plain = request().query(&caps, false);
            assert!(!plain.contains("rlRateChange"));
            let asked = LaunchRequest {
                rate_change: true,
                fec: Some(FecChoice::Auto),
                ..request()
            };
            let fec = LaunchRequest {
                fec: Some(FecChoice::Auto),
                ..request()
            };
            assert_eq!(
                asked.query(&caps, false),
                fec.query(&caps, false)
                    .replace("&corever=1", "&rlRateChange=1&corever=1")
            );
        }
    }

    #[test]
    fn only_a_rubylight_host_hears_whether_the_launch_confirms_the_replace() {
        let confirmed = LaunchRequest {
            confirm_replace: true,
            ..request()
        };
        let plain = request().query(&rubylight(), false);
        assert!(
            plain.contains("&gcpersist=0&rlConfirmReplace=0&corever=1"),
            "{plain}"
        );
        assert_eq!(
            confirmed.query(&rubylight(), false),
            plain.replace("rlConfirmReplace=0", "rlConfirmReplace=1")
        );
        // Vibepollo and the rest keep their 60-second relaunch rule and today's query.
        let caps = HostCapabilities::UNKNOWN;
        assert_ne!(caps.family(), Family::Rubylight);
        assert_eq!(confirmed.query(&caps, false), request().query(&caps, false));
        assert!(!request().query(&caps, false).contains(CONFIRM_REPLACE));
    }

    #[test]
    fn only_the_replace_prompt_asks_to_confirm() {
        let status = |code, message: &str| Error::Status {
            code,
            message: message.into(),
        };
        assert!(asks_to_confirm_replace(&status(410, REPLACE_PROMPT)));
        // 410 also reports finished actions; other codes are refusals.
        assert!(!asks_to_confirm_replace(&status(
            410,
            "Application terminated"
        )));
        assert!(!asks_to_confirm_replace(&status(409, REPLACE_PROMPT)));
        assert!(!asks_to_confirm_replace(&Error::NotFound(
            REPLACE_PROMPT.into()
        )));
    }

    #[test]
    fn roles_and_gamestream_quirks() {
        let monitor = LaunchRequest {
            role: AppRole::RemoteMonitor,
            gamepad_mask: 1,
            persist_gamepads: true,
            vrr: true,
            ..request()
        };
        let q = monitor.query(&rubylight(), false);
        assert!(q.contains("&remote_monitor=1&mode="));
        assert!(q.contains("&remoteControllersBitmap=0&gcmap=0&gcpersist=0"));
        let input = LaunchRequest {
            role: AppRole::InputOnly,
            hdr: true,
            vrr: true,
            ..request()
        };
        let q = input.query(&rubylight(), false);
        assert!(q.contains("&input_only=1") && !q.contains("hdrMode") && !q.contains("Vrr"));
        let small = LaunchRequest {
            small_stream_input_only: true,
            ..input
        };
        assert!(!small.query(&rubylight(), false).contains("input_only"));

        let gfe = LaunchRequest {
            width: 2560,
            height: 1440,
            fps: 120,
            ..request()
        };
        let q = gfe.query(&HostCapabilities::UNKNOWN, true);
        assert!(q.contains("&mode=2560x1440x0&") && q.contains("&sops=0&"));
    }

    #[test]
    fn launch_replies() {
        let ok = "<root status_code=\"200\"><gamesession>1</gamesession><sessionUrl0>rtspenc://10.0.0.2:48010</sessionUrl0></root>";
        assert_eq!(
            parse_launch_reply(ok, false).unwrap(),
            Some(Launched {
                rtsp_session_url: Some("rtspenc://10.0.0.2:48010".into())
            })
        );
        assert_eq!(
            parse_launch_reply("<root status_code=\"200\"><resume>0</resume></root>", true)
                .unwrap(),
            None
        );
        assert!(parse_launch_reply(ok, true).is_err());
        assert!(matches!(
            parse_launch_reply("<root status_code=\"503\" status_message=\"busy\"><gamesession>0</gamesession></root>", false),
            Err(Error::Status { code: 503, .. })
        ));
    }
}
