//! Redaction of the host's logs and diagnostics for a support bundle. Out go what identifies the
//! person rather than the problem: IP addresses (loopback and unspecified ones stay, and a local
//! one is marked as such), MAC addresses, user folders on Windows, macOS and Linux, the Windows
//! user names, the PC's name, paired devices' names, the library's game titles, and the value after
//! a secret's name (`pin=`, `token=`, `Bearer`). In stay what support needs to diagnose a stream:
//! error codes, GPU and driver names and versions, codecs, timings, ports, stream IDs and log
//! levels. A known name becomes a mark that keeps lines comparable: `[user]`, `[pc]`,
//! `[device-2]`, `[app-3]`.
//!
//! A version written as four small numbers (`1.2.3.4`) can't be told from an IPv4 address and is
//! redacted as one; driver versions have a group above 255 and stay.

use regex::Regex;
use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    sync::LazyLock,
};

/// Shorter names are left alone: they would take ordinary words with them.
const MIN_NAME: usize = 3;
/// The default library's apps, which say nothing about anyone.
const PUBLIC_APPS: &[&str] = &["Desktop", "Steam Big Picture"];
/// Profile folders every Windows PC has.
const SHARED_PROFILES: &[&str] = &[
    "public",
    "default",
    "default user",
    "all users",
    "defaultapppool",
    "system",
    "systemprofile",
    "localservice",
    "networkservice",
];

/// A user's home folder in a path, up to the user's name: `C:\Users\` and `Documents and
/// Settings` (any case, either separator), `/Users/` and `/home/`. A name with spaces counts as
/// one only up to the separator after it.
static USER_PATH: LazyLock<Regex> = LazyLock::new(|| {
    let name = r#"[^\\/"'\s:*?<>|]+"#;
    Regex::new(&format!(
        r"(?i)((?:\b[a-z]:)?[\\/]+(?:users|home|documents and settings)[\\/]+)(?:{name}(?: +{name})*([\\/])|{name})"
    ))
    .unwrap()
});
static MAC_COLONS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b[0-9a-f]{2}(?::[0-9a-f]{2}){5}\b").unwrap());
static MAC_DASHES: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b[0-9a-f]{2}(?:-[0-9a-f]{2}){5}\b").unwrap());
static MAPPED_V4: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)::ffff:([0-9]{1,3}(?:\.[0-9]{1,3}){3})").unwrap());
static IPV4: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[0-9]{1,3}(?:\.[0-9]{1,3}){3}").unwrap());
/// Groups around a `::`, or five to eight groups without one, with an optional zone.
static IPV6: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:[0-9a-fA-F]{0,4}(?::[0-9a-fA-F]{0,4}){0,6}::[0-9a-fA-F]{0,4}(?::[0-9a-fA-F]{0,4}){0,6}|(?:[0-9a-fA-F]{1,4}:){4,7}[0-9a-fA-F]{1,4})(?:%[A-Za-z0-9_]+)?").unwrap()
});
static SECRET: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)\b(pin|otp|password|passphrase|passcode|secret|client_secret|token|access_token|refresh_token|api_token)(\s*[=:]\s*"?)[^\s",;}]+"#).unwrap()
});
static BEARER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b(bearer|basic)\s+[A-Za-z0-9+/=._~-]{8,}").unwrap());
static EMAIL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[A-Za-z0-9._%+-]+@[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)*\.[A-Za-z]{2,}").unwrap()
});

/// The mark for an address: loopback, unspecified and broadcast stay as they are (`None`).
fn address_mark(address: IpAddr) -> Option<&'static str> {
    let local = match address {
        IpAddr::V4(v4) => {
            if v4.is_loopback() || v4.is_unspecified() || v4.is_broadcast() {
                return None;
            }
            let [a, b, ..] = v4.octets();
            v4.is_private() || v4.is_link_local() || (a == 100 && (64..128).contains(&b))
        }
        IpAddr::V6(v6) => {
            if v6.is_loopback() || v6.is_unspecified() {
                return None;
            }
            let first = v6.segments()[0];
            (first & 0xfe00) == 0xfc00 || (first & 0xffc0) == 0xfe80
        }
    };
    Some(if local { "[lan-ip]" } else { "[ip]" })
}

/// `text` with each match of `pattern` that `replace` returns a replacement for, given the match
/// and the characters just before and after it, replaced.
fn replace_checked(
    text: &str,
    pattern: &Regex,
    replace: impl Fn(&str, Option<char>, Option<char>) -> Option<String>,
) -> String {
    let mut output = String::with_capacity(text.len());
    let mut last = 0;
    for found in pattern.find_iter(text) {
        let before = text[..found.start()].chars().next_back();
        let after = text[found.end()..].chars().next();
        if let Some(replacement) = replace(found.as_str(), before, after) {
            output.push_str(&text[last..found.start()]);
            output.push_str(&replacement);
            last = found.end();
        }
    }
    output.push_str(&text[last..]);
    output
}

fn word(c: Option<char>) -> bool {
    c.is_some_and(|c| c.is_alphanumeric() || c == '_')
}

/// A name the redactor was told and the mark that replaces it.
struct Name {
    text: String,
    lower: String,
    mark: String,
    ignore_case: bool,
}

/// Redacts lines with the names it was told and the patterns every line gets.
#[derive(Default)]
pub struct Redactor {
    /// Longest name first, so a name inside a longer one does not leave the rest behind.
    names: Vec<Name>,
    /// Every name in one pattern, so a line is searched once however long the library is; built
    /// at the first line after a name was added.
    pattern: std::sync::OnceLock<Regex>,
    devices: usize,
    apps: usize,
}

impl Redactor {
    pub fn new() -> Self {
        Self::default()
    }

    fn add(&mut self, name: &str, mark: String, ignore_case: bool) -> bool {
        let name = name.trim();
        let lower = name.to_lowercase();
        if name.chars().count() < MIN_NAME || self.names.iter().any(|known| known.lower == lower) {
            return false;
        }
        self.names.push(Name {
            text: name.to_owned(),
            lower,
            mark,
            ignore_case,
        });
        self.names
            .sort_by_key(|n| std::cmp::Reverse(n.text.chars().count()));
        self.pattern = std::sync::OnceLock::new();
        true
    }

    /// The names, each not inside a longer word, longest first; `None` without names.
    fn names_pattern(&self) -> Option<&Regex> {
        if self.names.is_empty() {
            return None;
        }
        Some(self.pattern.get_or_init(|| {
            let names: Vec<String> = self
                .names
                .iter()
                .map(|n| match n.ignore_case {
                    true => format!("(?i:{})", regex::escape(&n.text)),
                    false => regex::escape(&n.text),
                })
                .collect();
            regex::RegexBuilder::new(&format!(
                r"(^|[^\p{{L}}\p{{N}}_])({})($|[^\p{{L}}\p{{N}}_])",
                names.join("|")
            ))
            .size_limit(256 << 20)
            .build()
            .expect("escaped names form a pattern")
        }))
    }

    /// The mark of the name `found` matched.
    fn mark(&self, found: &str) -> &str {
        let lower = found.to_lowercase();
        self.names
            .iter()
            .find(|n| n.text == found)
            .or_else(|| {
                self.names
                    .iter()
                    .find(|n| n.ignore_case && n.lower == lower)
            })
            .map_or("[redacted]", |n| n.mark.as_str())
    }

    /// A Windows user name; a profile every PC has is left alone.
    pub fn user(&mut self, name: &str) {
        let lower = name.trim().to_lowercase();
        if !SHARED_PROFILES.contains(&lower.as_str()) && !lower.ends_with('$') {
            self.add(name, "[user]".into(), true);
        }
    }

    /// The PC's name, or the name clients show for it.
    pub fn pc(&mut self, name: &str) {
        self.add(name, "[pc]".into(), true);
    }

    /// A paired device's name.
    pub fn device(&mut self, name: &str) {
        let mark = format!("[device-{}]", self.devices + 1);
        if self.add(name, mark, false) {
            self.devices += 1;
        }
    }

    /// An app's title; the default library's apps are left alone.
    pub fn app(&mut self, name: &str) {
        if PUBLIC_APPS.contains(&name.trim()) {
            return;
        }
        let mark = format!("[app-{}]", self.apps + 1);
        if self.add(name, mark, false) {
            self.apps += 1;
        }
    }

    /// One line (or any text) with every name and pattern redacted.
    pub fn line(&self, line: &str) -> String {
        // Secrets and e-mail first: a name inside one would otherwise leave the rest behind.
        let mut text = EMAIL.replace_all(line, "[email]").into_owned();
        text = BEARER.replace_all(&text, "$1 [redacted]").into_owned();
        text = SECRET.replace_all(&text, "$1$2[redacted]").into_owned();
        if let Some(pattern) = self.names_pattern() {
            // Twice: a match takes the separator after it, which the next match may need.
            for _ in 0..2 {
                text = pattern
                    .replace_all(&text, |c: &regex::Captures| {
                        format!("{}{}{}", &c[1], self.mark(&c[2]), &c[3])
                    })
                    .into_owned();
            }
        }
        // After the names: a known name with a space goes whole before the path takes a word.
        text = USER_PATH.replace_all(&text, "${1}[user]${2}").into_owned();
        text = MAC_COLONS.replace_all(&text, "[mac]").into_owned();
        text = MAC_DASHES.replace_all(&text, "[mac]").into_owned();
        text = MAPPED_V4
            .replace_all(&text, |c: &regex::Captures| {
                match c[1].parse::<Ipv4Addr>().ok().map(IpAddr::V4) {
                    Some(address) => address_mark(address).map_or(c[0].to_owned(), str::to_owned),
                    None => c[0].to_owned(),
                }
            })
            .into_owned();
        text = replace_checked(&text, &IPV4, |found, before, after| {
            // Part of a longer dotted number, such as a version: not an address.
            if word(before) || before == Some('.') || word(after) {
                return None;
            }
            address_mark(IpAddr::V4(found.parse().ok()?)).map(str::to_owned)
        });
        replace_checked(&text, &IPV6, |found, before, after| {
            // `module::path` and `HH:MM:SS` sit next to letters or digits; an address doesn't.
            if word(before) || word(after) || before == Some(':') || after == Some(':') {
                return None;
            }
            let address = found.split('%').next()?.parse::<Ipv6Addr>().ok()?;
            address_mark(IpAddr::V6(address)).map(str::to_owned)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn redactor() -> Redactor {
        let mut r = Redactor::new();
        r.user("alice");
        r.user("Public");
        r.pc("GAMING-PC");
        r.device("Alice's Galaxy Fold");
        r.device("TV");
        r.app("Cyberpunk 2077");
        r.app("Desktop");
        r
    }

    #[test]
    fn addresses_names_paths_and_secrets_leave_and_diagnostics_stay() {
        let r = redactor();
        let line = r#"2026-10-10T12:00:47.149150Z  INFO butterpollo::nvhttp: launch client="Alice's Galaxy Fold" peer=192.168.4.20:50123 app="Cyberpunk 2077" from fd00:1:2:3::9 via 203.0.113.9 mac=AA:BB:CC:DD:EE:FF adapter=aa-bb-cc-dd-ee-01 dir=C:\Users\alice\AppData\Local\Rubylight user=ALICE pc=gaming-pc pin=1234 token="abcdef123456" Authorization: Bearer abcdefghijklmnop mail=alice@example.com"#;
        let out = r.line(line);
        for leaked in [
            "Galaxy",
            "192.168.4.20",
            "Cyberpunk",
            "fd00:1:2:3::9",
            "203.0.113.9",
            "AA:BB",
            "aa-bb",
            "alice",
            "ALICE",
            "gaming-pc",
            "1234",
            "abcdef123456",
            "abcdefghijklmnop",
            "example.com",
        ] {
            assert!(!out.contains(leaked), "{leaked} in {out}");
        }
        for kept in [
            "2026-10-10T12:00:47.149150Z  INFO butterpollo::nvhttp: launch",
            "client=\"[device-1]\"",
            "peer=[lan-ip]:50123",
            "app=\"[app-1]\"",
            "from [lan-ip] via [ip]",
            "mac=[mac] adapter=[mac]",
            r"dir=C:\Users\[user]\AppData\Local\Rubylight",
            "user=[user] pc=[pc] pin=[redacted] token=\"[redacted]\"",
            "Bearer [redacted]",
            "mail=[email]",
        ] {
            assert!(out.contains(kept), "{kept} not in {out}");
        }
    }

    #[test]
    fn what_support_needs_to_diagnose_a_stream_stays() {
        let r = redactor();
        for line in [
            "2026-10-10T12:00:47.149150Z  WARN butterpollo_win_graphics::amf: slow encoder call call=\"AMF SubmitInput\" elapsed_ms=34160",
            "2026-10-10T12:00:47.150002Z  WARN butterpollo_core::session: GPU device lost: DXGI_ERROR_DEVICE_RESET (0x887A0007). Rebuilding capture and encoder",
            "No video encoder available: NVENC could not start on NVIDIA GeForce RTX 4070. code=\"video_encoder\"",
            "adapter=\"AMD Radeon RX 7900 XT\" driver=32.0.12033.1030 codec=hevc 3840x2160@120 bitrate_kbps=80000 rtt_ms=2.4",
            "stream_id=9f986184-4b55-405e-8ead-be60f61e6943 loopback 127.0.0.1:47989 [::1]:47984 any 0.0.0.0 ::",
            "stream 00:01:23 chroma 4:4:4 Desktop capture via Desktop Duplication on display \\\\.\\DISPLAY1",
            "token expired; refresh token rotated; pairing PIN required in the web interface",
        ] {
            assert_eq!(r.line(line), line);
        }
    }

    #[test]
    fn macos_linux_and_forward_slash_user_paths_are_scrubbed() {
        let r = Redactor::new();
        assert_eq!(
            r.line("/Users/bob/Library/Logs and /home/carol/.config and C:/Users/Dave/x"),
            "/Users/[user]/Library/Logs and /home/[user]/.config and C:/Users/[user]/x"
        );
    }

    #[test]
    fn a_large_library_is_one_pattern_and_each_title_keeps_its_mark() {
        let mut r = Redactor::new();
        for n in 1..=3000 {
            r.app(&format!("Game Title {n}"));
        }
        r.user("alice");
        assert_eq!(
            r.line("launch app=\"Game Title 2999\" user=ALICE then Game Title 7"),
            "launch app=\"[app-2999]\" user=[user] then [app-7]"
        );
    }

    #[test]
    fn a_user_folder_with_a_space_goes_whole() {
        let mut r = Redactor::new();
        // Known, as every profile folder of this PC is, and unknown (another PC's path).
        r.user("John Smith");
        r.user("systemprofile");
        assert_eq!(
            r.line(r"open C:\Users\John Smith\Videos and D:\Documents and Settings\Ann Lee\x.mp4"),
            r"open C:\Users\[user]\Videos and D:\Documents and Settings\[user]\x.mp4"
        );
        assert_eq!(
            r.line(r"C:\Windows\system32\config\systemprofile\AppData"),
            r"C:\Windows\system32\config\systemprofile\AppData"
        );
    }

    #[test]
    fn a_name_inside_a_longer_word_and_a_short_name_stay() {
        let mut r = Redactor::new();
        r.app("Max");
        r.device("PC");
        assert_eq!(
            r.line("Maximum bitrate on PC for Max."),
            "Maximum bitrate on PC for [app-1]."
        );
    }

    #[test]
    fn ipv4_mapped_addresses_are_marked_once() {
        let r = Redactor::new();
        assert_eq!(
            r.line("peer=[::ffff:192.168.1.5]:5000 other=::ffff:8.8.8.8"),
            "peer=[[lan-ip]]:5000 other=[ip]"
        );
    }
}
