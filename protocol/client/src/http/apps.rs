//! The host's `/applist`, and which entries are control tiles rather than games
//! (NvHTTP.getAppListByReader and NvApp.java).

use super::capabilities::{Family, Feature, HostCapabilities};
use super::xml;
use super::Error;

/// What starting a session for this app does to the stream's role.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AppRole {
    Stream,
    RemoteMonitor,
    InputOnly,
}

/// Rubylight and Vibepollo control tiles, and Apollo's Remote Input and Terminate apps.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AppControl {
    None,
    Resume,
    DisconnectMonitor,
    DisconnectInput,
    Terminate,
    Monitor,
    Input,
    RunningGame,
}

const CONTROLS: [AppControl; 8] = [
    AppControl::None,
    AppControl::Resume,
    AppControl::DisconnectMonitor,
    AppControl::DisconnectInput,
    AppControl::Terminate,
    AppControl::Monitor,
    AppControl::Input,
    AppControl::RunningGame,
];

/// Apollo src/process.h.
const APOLLO_REMOTE_INPUT_UUID: &str = "8CB5C136-DA67-4F99-B4A1-F9CD35005CF4";
const APOLLO_TERMINATE_UUID: &str = "E16CBE1B-295D-4632-9A76-EC4180C857D3";

/// One app the host lists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct App {
    pub name: String,
    pub id: i32,
    pub hdr_supported: bool,
    /// Trimmed; empty when the host sends none.
    pub uuid: String,
    /// The host's display order, when it gives one.
    pub host_index: Option<u32>,
    pub art_version: String,
    pub control: AppControl,
    /// The catalogue has a Disconnect Monitor tile, so Resume rejoins as a monitor.
    pub monitor_resume: bool,
}

impl App {
    /// The role a session for this app takes. `small_stream_input_only` joins an ordinary game
    /// as input only on a host without its own input-only mode.
    pub fn role(&self, small_stream_input_only: bool) -> AppRole {
        if small_stream_input_only && self.control == AppControl::None {
            return AppRole::InputOnly;
        }
        match self.control {
            AppControl::Monitor | AppControl::DisconnectMonitor => AppRole::RemoteMonitor,
            AppControl::Input | AppControl::DisconnectInput => AppRole::InputOnly,
            AppControl::Resume if self.monitor_resume => AppRole::RemoteMonitor,
            _ => AppRole::Stream,
        }
    }

    pub fn is_control_action(&self) -> bool {
        matches!(
            self.control,
            AppControl::DisconnectMonitor | AppControl::DisconnectInput | AppControl::Terminate
        )
    }

    /// Whether this is the game the host reports running; a UUID decides when both have one.
    pub fn matches_running_app(&self, running_id: i32, running_uuid: Option<&str>) -> bool {
        match running_uuid {
            Some(uuid) if !self.uuid.is_empty() && !uuid.is_empty() => self.uuid == uuid,
            _ => running_id != 0 && self.id == running_id,
        }
    }
}

/// Reserved identities mean something only on hosts that give them that meaning.
pub fn app_control(id: i32, uuid: &str, caps: &HostCapabilities) -> AppControl {
    let family = caps.family();
    let apollo_apps = matches!(family, Family::Apollo | Family::Vibepollo | Family::Unknown);
    if apollo_apps && uuid.eq_ignore_ascii_case(APOLLO_REMOTE_INPUT_UUID) {
        return AppControl::Input;
    }
    if apollo_apps && uuid.eq_ignore_ascii_case(APOLLO_TERMINATE_UUID) {
        return AppControl::Terminate;
    }
    if !caps.may_have(Feature::RemoteSessionTiles) {
        return AppControl::None;
    }
    // Rubylight rust/core/src/remote.rs and Vibepollo remote_session.h keep these stable.
    for offset in 1..=7 {
        if id == 2_147_483_500 + offset
            || id == 2_147_483_600 + offset
            || (matches!(offset, 1 | 4 | 5 | 6) && id == 2_147_483_510 + offset)
            || uuid == format!("9a1c5a25-58fe-40e0-b9aa-7d3f0000000{offset}")
        {
            return CONTROLS[offset as usize];
        }
    }
    AppControl::None
}

/// Parses an app list reply; incomplete entries (no valid ID) are dropped.
pub fn parse_app_list(text: &str, caps: &HostCapabilities) -> Result<Vec<App>, Error> {
    let root = xml::parse_reply(text)?;
    let mut apps = Vec::new();
    for element in root.descendants().into_iter().filter(|e| e.name == "App") {
        let mut name = String::new();
        let mut id = None;
        let mut hdr_supported = false;
        let mut uuid = String::new();
        let mut host_index = None;
        let mut art_version = String::new();
        for field in element.descendants() {
            let text = field.text();
            if text.is_empty() {
                continue;
            }
            match field.name.as_str() {
                "AppTitle" => name = text,
                "ID" => id = text.parse::<i32>().ok().or(id),
                "IsHdrSupported" => hdr_supported = text == "1",
                "UUID" => uuid = text.trim().to_owned(),
                "IDX" => {
                    host_index = text
                        .trim()
                        .parse::<i32>()
                        .ok()
                        .filter(|&i| i >= 0)
                        .map(|i| i as u32)
                }
                "ArtVersion" => art_version = text,
                _ => {}
            }
        }
        if let Some(id) = id {
            let control = app_control(id, &uuid, caps);
            apps.push(App {
                name,
                id,
                hdr_supported,
                uuid,
                host_index,
                art_version,
                control,
                monitor_resume: false,
            });
        }
    }
    let monitor_catalogue = apps
        .iter()
        .any(|a| a.control == AppControl::DisconnectMonitor);
    for app in &mut apps {
        app.monitor_resume = monitor_catalogue;
    }
    Ok(apps)
}

#[cfg(test)]
mod tests {
    use super::super::serverinfo::{tests::RUBYLIGHT, ServerInfo};
    use super::*;

    #[test]
    fn rubylight_app_list_with_control_tiles() {
        let caps = ServerInfo::parse(RUBYLIGHT, 47989).unwrap().capabilities;
        let list = "<?xml version=\"1.0\"?><root status_code=\"200\">\
            <App><AppTitle>Desktop &amp; Co</AppTitle><UUID> u-1 </UUID><IDX>0</IDX><ID>881448767</ID>\
            <ArtVersion>881448767</ArtVersion><IsHdrSupported>1</IsHdrSupported></App>\
            <App><AppTitle>Resume</AppTitle><UUID></UUID><IDX>1</IDX><ID>2147483501</ID></App>\
            <App><AppTitle>Disconnect Monitor</AppTitle><ID>2147483502</ID></App>\
            <App><AppTitle>Broken</AppTitle><ID>x</ID></App></root>";
        let apps = parse_app_list(list, &caps).unwrap();
        assert_eq!(apps.len(), 3);
        assert_eq!(apps[0].name, "Desktop & Co");
        assert_eq!(
            (apps[0].id, apps[0].uuid.as_str(), apps[0].host_index),
            (881_448_767, "u-1", Some(0))
        );
        assert!(apps[0].hdr_supported);
        assert_eq!(apps[0].control, AppControl::None);
        assert_eq!(apps[1].control, AppControl::Resume);
        assert_eq!(
            apps[1].role(false),
            AppRole::RemoteMonitor,
            "a monitor catalogue resumes as a monitor"
        );
        assert_eq!(apps[2].control, AppControl::DisconnectMonitor);
        assert!(apps[2].is_control_action());
        assert_eq!(apps[0].role(true), AppRole::InputOnly);
        assert!(apps[0].matches_running_app(5, Some("u-1")));
        assert!(!apps[0].matches_running_app(881_448_767, Some("other")));
    }

    #[test]
    fn reserved_ids_are_ordinary_on_sunshine() {
        let sunshine =
            "<root status_code=\"200\"><appversion>7.1.431.-1</appversion><uniqueid>a</uniqueid>\
            <PairStatus>1</PairStatus><state>SUNSHINE_SERVER_FREE</state></root>";
        let caps = ServerInfo::parse(sunshine, 1).unwrap().capabilities;
        assert_eq!(app_control(2_147_483_502, "", &caps), AppControl::None);
        assert_eq!(
            app_control(1, APOLLO_TERMINATE_UUID, &caps),
            AppControl::None
        );
        assert_eq!(
            app_control(
                1,
                &APOLLO_TERMINATE_UUID.to_lowercase(),
                &HostCapabilities::UNKNOWN
            ),
            AppControl::Terminate
        );
        assert_eq!(
            app_control(2_147_483_606, "", &HostCapabilities::UNKNOWN),
            AppControl::Input
        );
        assert!(parse_app_list("<root status_code=\"200\"><App>", &caps).is_err());
    }
}
