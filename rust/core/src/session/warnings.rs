//! The user-facing warnings a session raises.
use serde::Serialize;
use std::{
    collections::BTreeMap,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Warning {
    pub code: String,
    pub message: String,
}

/// How long a dropped packet, frame or input stays on the stream card after
/// it last happened.
pub const EVENT_PERIOD: Duration = Duration::from_secs(30);

/// Current warnings by condition code, each with an optional expiry.
type WarningEntries = BTreeMap<String, (String, Option<Instant>)>;

#[derive(Default)]
pub struct Warnings {
    entries: std::sync::Mutex<WarningEntries>,
    /// A bit per present code's hash: clearing an absent code, as per-frame
    /// recovery paths do on a healthy stream, takes no lock.
    pub(super) present: AtomicU64,
}
pub(super) fn warning_bit(code: &str) -> u64 {
    let hash = code.bytes().fold(0xcbf2_9ce4_8422_2325u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    });
    1 << (hash % 64)
}
fn warning_bits(entries: &WarningEntries) -> u64 {
    entries
        .keys()
        .fold(0, |bits, code| bits | warning_bit(code))
}
impl Warnings {
    /// Keep one current warning per condition, without flooding a retry loop.
    pub fn set(&self, code: &str, message: impl AsRef<str>) {
        self.insert(code, message.as_ref(), None);
    }
    /// A recurring event rather than a lasting condition: shown until
    /// `period` passes without it, and logged again only after that.
    pub fn event(&self, code: &str, message: impl AsRef<str>, period: Duration) {
        self.insert(code, message.as_ref(), Some(Instant::now() + period));
    }
    fn insert(&self, code: &str, message: &str, expires: Option<Instant>) {
        let now = Instant::now();
        let mut entries = self.entries.lock().unwrap();
        match entries.get_mut(code) {
            Some((current, until)) if current == message && until.is_none_or(|at| at > now) => {
                *until = expires;
            }
            _ => {
                tracing::warn!(code, "{message}");
                entries.insert(code.into(), (message.into(), expires));
                self.present.fetch_or(warning_bit(code), Ordering::Release);
            }
        }
    }
    pub fn clear(&self, code: &str) {
        if self.present.load(Ordering::Acquire) & warning_bit(code) == 0 {
            return;
        }
        let mut entries = self.entries.lock().unwrap();
        if entries.remove(code).is_some() {
            self.present
                .store(warning_bits(&entries), Ordering::Release);
        }
    }
    pub fn snapshot(&self) -> Vec<Warning> {
        let now = Instant::now();
        let mut entries = self.entries.lock().unwrap();
        let before = entries.len();
        entries.retain(|_, (_, until)| until.is_none_or(|at| at > now));
        if entries.len() != before {
            self.present
                .store(warning_bits(&entries), Ordering::Release);
        }
        entries
            .iter()
            .map(|(code, (message, _))| Warning {
                code: code.clone(),
                message: message.clone(),
            })
            .collect()
    }
}
