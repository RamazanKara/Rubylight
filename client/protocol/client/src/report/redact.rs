//! Log redaction shared with the Android app's `utils/RedactedLog.redact`: names and addresses of
//! the user's PCs, URLs, e-mail, IP and MAC addresses, UUIDs, keys, Android paths and PINs are
//! replaced by marks such as `[PC]` or `[address]`.
//!
//! The cases in `tests/fixtures/redact` are replayed against both (cargo `redact_contract` and
//! JUnit `RedactedLogContractTest`), so the two stay equal. The patterns use Java's ASCII
//! classes (`\w`, `\d`, `\s`, `\b`), not Unicode ones, except that e-mail addresses may hold any
//! character past ASCII. Message lengths count characters where Java counts UTF-16 units, which
//! differs only outside the Basic Multilingual Plane.

use std::sync::{LazyLock, RwLock};

use regex::Regex;

/// Longest message [`Secrets::redact`] keeps, in characters; longer ones are cut and get `…`.
pub const MAX_MESSAGE_CHARS: usize = 300;

/// Fixed event lines that pass unchanged (`RedactedLog.EVENTS`).
pub const EVENTS: &[&str] = &[
    "Stream starting",
    "Stream connected",
    "Stream stopping",
    "Stream failed",
    "Stream disconnected",
    "Stream activity destroyed",
    "Stream restored after process death",
    "Pairing started",
    "Pairing succeeded",
    "Pairing failed",
    "Pairing cancelled",
    "USB controller attached",
    "USB controller detached",
    "USB permission denied",
    "Bluetooth or Android controller detached",
    "Connection test started",
    "Connection test finished",
];

// Java's `\S`; `(?-u:\b)` is its ASCII word boundary.
const NOT_SPACE: &str = r"[^ \t\n\x0B\x0C\r]";
// The letters of an e-mail address: `\w` and any other character past ASCII, as in `RedactedLog`
// (Android's `\w` is Unicode, the JVM's is ASCII).
const MAIL_WORD: &str = r"A-Za-z0-9_\x{80}-\x{10FFFF}";

/// In order: URLs, e-mail addresses, IPv4, IPv6 and MAC addresses, UUIDs, long hex, keys and
/// other long tokens, Android paths. Each with the mark that replaces it.
static SENSITIVE: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    [
        (format!("[a-zA-Z][a-zA-Z0-9+.-]*://{NOT_SPACE}+"), "[url]"),
        (
            format!(r"[{MAIL_WORD}.+-]+@[{MAIL_WORD}-]+(?:\.[{MAIL_WORD}-]+)+"),
            "[email]",
        ),
        (r"(?-u:\b)[0-9]{1,3}(?:\.[0-9]{1,3}){3}(?::[0-9]+)?(?-u:\b)".into(), "[address]"),
        (r"[0-9a-fA-F]{0,4}(?::[0-9a-fA-F]{0,4}){2,7}(?:%[A-Za-z0-9_]+)?".into(), "[address]"),
        (
            r"(?-u:\b)[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}(?-u:\b)".into(),
            "[id]",
        ),
        (r"(?-u:\b)[0-9a-fA-F]{16,}(?-u:\b)".into(), "[hex]"),
        (r"[A-Za-z0-9+/=_-]{32,}".into(), "[data]"),
        (format!("(?:/storage|/sdcard|/data/user|/data/data|content:){NOT_SPACE}*"), "[path]"),
    ]
    .into_iter()
    .map(|(pattern, mark)| (Regex::new(&pattern).expect("redaction pattern"), mark))
    .collect()
});

/// A secret's keyword and up to three separators, then the secret itself.
static PIN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"(?-u:\b)((?i-u:pin|password|passphrase|passcode|secret|token|key))(?-u:\b)([^A-Za-z0-9_]{{0,3}}){NOT_SPACE}+"
    ))
    .expect("PIN pattern")
});

/// Pairing messages may carry the PIN as a bare number.
static SHORT_NUMBER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?-u:\b)[0-9]{4,8}(?-u:\b)").expect("number pattern"));

/// Names and addresses known to belong to the user's PCs, replaced by `[PC]` wherever they
/// appear (`RedactedLog.SECRETS`).
#[derive(Debug, Default, Clone)]
pub struct Secrets {
    /// Longest first, so a name inside a longer one does not leave the rest behind.
    list: Vec<String>,
}

impl Secrets {
    pub const fn new() -> Self {
        Self { list: Vec::new() }
    }

    /// Adds a PC name or address; blanks and single characters are ignored, as in Java.
    pub fn add(&mut self, secret: &str) {
        let secret = secret.trim();
        if secret.chars().count() < 2 || self.list.iter().any(|s| s == secret) {
            return;
        }
        self.list.push(secret.to_owned());
        self.list.sort_by(|a, b| {
            b.chars()
                .count()
                .cmp(&a.chars().count())
                .then_with(|| a.cmp(b))
        });
    }

    /// `RedactedLog.redact`: one message on one line, at most [`MAX_MESSAGE_CHARS`] long.
    pub fn redact(&self, message: &str) -> String {
        if EVENTS.contains(&message) {
            return message.to_owned();
        }
        let text = self.scrub(&message.replace(['\n', '\r'], " "));
        match text.char_indices().nth(MAX_MESSAGE_CHARS) {
            Some((cut, _)) => format!("{}…", &text[..cut]),
            None => text,
        }
    }

    /// The replacements of [`Secrets::redact`] alone, for a line of a log file: line breaks and
    /// the length stay as they are.
    pub fn scrub(&self, text: &str) -> String {
        let mut text = text.to_owned();
        for secret in &self.list {
            if text.contains(secret.as_str()) {
                text = text.replace(secret.as_str(), "[PC]");
            }
        }
        for (pattern, mark) in SENSITIVE.iter() {
            if let std::borrow::Cow::Owned(replaced) = pattern.replace_all(&text, *mark) {
                text = replaced;
            }
        }
        if let std::borrow::Cow::Owned(replaced) = PIN.replace_all(&text, "${1}${2}[omitted]") {
            text = replaced;
        }
        if text.to_lowercase().contains("pair") {
            if let std::borrow::Cow::Owned(replaced) = SHORT_NUMBER.replace_all(&text, "[omitted]")
            {
                text = replaced;
            }
        }
        text
    }
}

static SECRETS: RwLock<Secrets> = RwLock::new(Secrets::new());

/// Adds a PC name or address to the process-wide secrets that [`redact`] and [`scrub`] use.
pub fn add_secret(secret: &str) {
    SECRETS
        .write()
        .unwrap_or_else(|e| e.into_inner())
        .add(secret);
}

/// [`Secrets::redact`] with the process-wide secrets.
pub fn redact(message: &str) -> String {
    SECRETS
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .redact(message)
}

/// [`Secrets::scrub`] with the process-wide secrets.
pub fn scrub(text: &str) -> String {
    SECRETS
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .scrub(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_longest_secret_goes_first_and_short_ones_are_ignored() {
        let mut secrets = Secrets::new();
        secrets.add("desk");
        secrets.add(" desktop.lan ");
        secrets.add("x");
        secrets.add("desk");
        assert_eq!(secrets.list, ["desktop.lan", "desk"]);
        assert_eq!(secrets.redact("desktop.lan and desk"), "[PC] and [PC]");
    }

    #[test]
    fn scrub_keeps_line_breaks_and_length() {
        let secrets = Secrets::new();
        let long = format!("{}\r\n", "word ".repeat(100));
        assert_eq!(secrets.scrub(&long), long);
        assert_eq!(secrets.redact(&long).chars().count(), MAX_MESSAGE_CHARS + 1);
    }

    #[test]
    fn the_process_wide_secrets_are_used() {
        add_secret("Process-wide PC");
        assert_eq!(redact("Process-wide PC is offline"), "[PC] is offline");
        assert_eq!(scrub("Process-wide PC\n"), "[PC]\n");
    }
}
