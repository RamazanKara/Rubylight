//! What the host supports, from its serverinfo version and RTSP DESCRIBE answer.

/// Sunshine feature flags (`x-ss-general.featureFlags`), host to client.
pub mod feature {
    /// Touch and pen events.
    pub const PEN_TOUCH_EVENTS: u32 = 0x01;
    /// Controller touchpad and motion events.
    pub const CONTROLLER_TOUCH_EVENTS: u32 = 0x02;
}

/// Client feature flags (`x-ml-general.featureFlags`), client to host.
pub mod client_feature {
    /// The client sends `SS_FRAME_FEC_STATUS`.
    pub const FEC_STATUS: u32 = 0x01;
    /// The client takes `X-SS-Ping-Payload` and `X-SS-Connect-Data`.
    pub const SESSION_ID_V1: u32 = 0x02;
    /// The client plays Nonary's PCM controller haptics (0x5601). Offer it only when it is
    /// used: it switches a Vibepollo host's control loop to a 5 ms period.
    pub const HAPTICS_PCM: u32 = 0x04;
}

/// Sunshine encryption flags (`x-ss-general.encryption*`).
pub mod encryption {
    pub const CONTROL_V2: u32 = 0x01;
    pub const VIDEO: u32 = 0x02;
    pub const AUDIO: u32 = 0x04;
    /// Apollo's and Rubylight's microphone stream.
    pub const MICROPHONE: u32 = 0x08;
}

/// Most ids an `a=x-rl-control` line is read for.
pub const MAX_CONTROL_MESSAGES: usize = 32;

/// The negotiated facts the control and input streams depend on.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HostCapabilities {
    /// The serverinfo `appversion` quad; Sunshine-family hosts send a negative fourth part.
    pub app_version: [i32; 4],
    /// `x-ss-general.featureFlags`.
    pub feature_flags: u32,
    /// Encryption the session enabled (`encryption::*`).
    pub encryption_enabled: u32,
    /// Rubylight control messages from `a=x-rl-control`.
    pub control_messages: Vec<u16>,
}

impl HostCapabilities {
    pub fn version_at_least(&self, major: i32, minor: i32, patch: i32) -> bool {
        let [a, b, c, _] = self.app_version;
        a > major || (a == major && b > minor) || (a == major && b == minor && c >= patch)
    }

    /// Sunshine and its forks (Apollo, Vibepollo, Rubylight).
    pub fn is_sunshine(&self) -> bool {
        self.app_version[3] < 0
    }

    /// Whether the control stream is encrypted (GFE 3.22 and every Sunshine host).
    pub fn encrypted_control(&self) -> bool {
        self.version_at_least(7, 1, 431)
    }

    /// Whether control messages use 12-byte IVs.
    pub fn control_v2(&self) -> bool {
        self.encryption_enabled & encryption::CONTROL_V2 != 0
    }

    pub fn has_feature(&self, flag: u32) -> bool {
        self.feature_flags & flag != 0
    }

    /// Whether the host named `message_type` in `a=x-rl-control`.
    pub fn supports_control_message(&self, message_type: u16) -> bool {
        self.control_messages.contains(&message_type)
    }

    /// Whether a Sunshine extension message may be sent: a Sunshine host on the encrypted
    /// control stream, as the C checks for every 0x55xx message.
    pub fn sunshine_extensions(&self) -> bool {
        self.is_sunshine() && self.encrypted_control()
    }
}

/// Parses serverinfo's `appversion` ("7.1.431.-1"); missing parts are 0.
pub fn parse_app_version(text: &str) -> Option<[i32; 4]> {
    let mut quad = [0; 4];
    let mut parts = 0;
    for (index, part) in text.trim().split('.').enumerate() {
        if index >= 4 {
            break;
        }
        quad[index] = part.trim().parse().ok()?;
        parts += 1;
    }
    (parts > 0).then_some(quad)
}

/// The ids every `a=x-rl-control:` line of an SDP names (Rubylight 2.2.0+), in order,
/// without duplicates, at most [`MAX_CONTROL_MESSAGES`]. Ids are 16-bit, hexadecimal with
/// `0x` or decimal; malformed entries are skipped.
pub fn parse_control_messages(sdp: &str) -> Vec<u16> {
    const PREFIX: &str = "a=x-rl-control:";
    let mut ids = Vec::new();
    for line in sdp.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        let Some(list) = line.strip_prefix(PREFIX) else {
            continue;
        };
        for item in list.split(',') {
            if let Some(id) = parse_control_id(item) {
                if !ids.contains(&id) && ids.len() < MAX_CONTROL_MESSAGES {
                    ids.push(id);
                }
            }
        }
    }
    ids
}

fn parse_control_id(item: &str) -> Option<u16> {
    let item = item.trim_matches([' ', '\t']);
    let (digits, radix) = match item.strip_prefix("0x").or_else(|| item.strip_prefix("0X")) {
        Some(hex) if !hex.is_empty() => (hex, 16),
        _ => (item, 10),
    };
    if digits.is_empty() || !digits.chars().all(|c| c.is_digit(radix)) {
        return None;
    }
    u16::from_str_radix(digits, radix).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_message_list_parses_like_the_c() {
        let sdp = "v=0\r\na=x-rl-control:0x5530, 0x5531,21810\r\na=x-other:1\na=x-rl-control:0x5530,bogus,0x10000,0x,,0X5540\n";
        assert_eq!(
            parse_control_messages(sdp),
            vec![0x5530, 0x5531, 0x5532, 0x5540]
        );
        assert!(parse_control_messages("v=0\r\n").is_empty());
        assert!(parse_control_messages("a=x-rl-control:").is_empty());
        let many: Vec<String> = (0..40).map(|i| format!("{}", 0x5500 + i)).collect();
        let sdp = format!("a=x-rl-control:{}", many.join(","));
        assert_eq!(parse_control_messages(&sdp).len(), MAX_CONTROL_MESSAGES);
    }

    #[test]
    fn versions_and_flags() {
        let caps = HostCapabilities {
            app_version: parse_app_version("7.1.431.-1").unwrap(),
            encryption_enabled: encryption::CONTROL_V2,
            feature_flags: feature::PEN_TOUCH_EVENTS,
            control_messages: vec![0x5531],
        };
        assert!(caps.is_sunshine() && caps.encrypted_control() && caps.control_v2());
        assert!(caps.sunshine_extensions());
        assert!(caps.has_feature(feature::PEN_TOUCH_EVENTS));
        assert!(!caps.has_feature(feature::CONTROLLER_TOUCH_EVENTS));
        assert!(caps.supports_control_message(0x5531) && !caps.supports_control_message(0x5530));
        let gfe = HostCapabilities {
            app_version: parse_app_version("7.1.409.0").unwrap(),
            ..Default::default()
        };
        assert!(!gfe.is_sunshine() && !gfe.encrypted_control() && gfe.version_at_least(7, 1, 409));
        assert_eq!(parse_app_version("7.1"), Some([7, 1, 0, 0]));
        assert_eq!(parse_app_version("x"), None);
    }
}
