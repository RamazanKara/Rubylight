//! RTX HDR's tuning from the NVIDIA driver profile of the game in front, as
//! NVIDIA's own overlay sets it. Driver profile I/O stays on a disposable
//! background thread, never encode.
use crate::nvidia_profile::{Drs, NvApi, Value};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant},
};
type Tuning = BTreeMap<String, i64>;
/// The driver profile settings NVIDIA's own RTX HDR reads for a game, with
/// their range in the profile and the offset to Rubylight's values. The
/// profile's peak brightness (0x00dd48fc) is the PC monitor's; a stream uses
/// the device's screen (`hdr_peak_brightness`) instead.
const SETTINGS: [(&str, u32, u32, u32, i64); 3] = [
    ("rtx_hdr_middle_gray", 0x00dd_48fd, 10, 100, 0),
    ("rtx_hdr_contrast", 0x00dd_48fe, 0, 200, -100),
    ("rtx_hdr_saturation", 0x00dd_48ff, 0, 200, -100),
];
/// Rubylight's values from one profile's settings, read by `read`; only
/// values in range count, and `own_only` keeps those stored in the profile
/// itself rather than inherited.
fn values(read: &mut dyn FnMut(u32) -> Option<Value>, own_only: bool) -> Tuning {
    SETTINGS
        .into_iter()
        .filter_map(|(key, id, min, max, offset)| {
            let value = read(id)?;
            (!own_only || value.own)
                .then_some(value.value)
                .filter(|raw| (min..=max).contains(raw))
                .map(|raw| (key.to_owned(), i64::from(raw) + offset))
        })
        .collect()
}
fn lookup(executable: &str) -> Option<Tuning> {
    let mut api = NvApi::open().ok()??;
    let global = api.global_profile().ok()?;
    let mut tuning = values(&mut |id| api.setting(global, id).ok().flatten(), false);
    let name = std::path::Path::new(executable)
        .file_name()?
        .to_string_lossy()
        .into_owned();
    for path in [executable.to_owned(), executable.replace('\\', "/"), name] {
        if let Ok(Some(app)) = api.application_profile(&path) {
            tuning.extend(values(&mut |id| api.setting(app, id).ok().flatten(), true));
            break;
        }
    }
    Some(tuning)
}
struct Reply {
    generation: u64,
    key: String,
    tuning: Option<Tuning>,
}
/// Retry delay after a failed DRS lookup; a successful lookup is kept for the
/// foreground executable until the foreground executable changes, avoiding
/// periodic full DRS reloads during streaming.
fn retry_delay(failures: u32) -> Duration {
    Duration::from_secs(match failures {
        0 | 1 => 15,
        2 => 30,
        _ => 60,
    })
}
pub struct Profiles {
    requests: mpsc::SyncSender<(u64, String)>,
    reply: Arc<Mutex<Option<Reply>>>,
    key: String,
    generation: u64,
    pending: bool,
    next: Option<Instant>,
    failures: u32,
    tuning: Tuning,
}
impl Profiles {
    pub fn new() -> Self {
        Self::with_lookup(lookup)
    }
    fn with_lookup(mut read: impl FnMut(&str) -> Option<Tuning> + Send + 'static) -> Self {
        let (requests, receiver) = mpsc::sync_channel::<(u64, String)>(1);
        let reply = Arc::new(Mutex::new(None));
        let output = reply.clone();
        std::thread::spawn(move || {
            while let Ok((generation, key)) = receiver.recv() {
                let tuning = read(&key);
                *output.lock().unwrap() = Some(Reply {
                    generation,
                    key,
                    tuning,
                });
            }
        });
        Self {
            requests,
            reply,
            key: String::new(),
            generation: 0,
            pending: false,
            next: None,
            failures: 0,
            tuning: Tuning::new(),
        }
    }
    pub fn poll(&mut self, key: Option<&str>) -> &Tuning {
        self.poll_at(key, Instant::now())
    }
    fn poll_at(&mut self, key: Option<&str>, now: Instant) -> &Tuning {
        let key = key.unwrap_or("");
        if key != self.key {
            self.key = key.into();
            self.generation = self.generation.wrapping_add(1);
            self.tuning.clear();
            self.pending = false;
            self.failures = 0;
            self.next = (!key.is_empty()).then_some(now);
        }
        if let Some(reply) = self.reply.lock().unwrap().take()
            && reply.generation == self.generation
            && reply.key == self.key
        {
            self.pending = false;
            if let Some(tuning) = reply.tuning {
                self.failures = 0;
                self.tuning = tuning;
                self.next = None;
            } else {
                self.failures = self.failures.saturating_add(1);
                self.next = Some(now + retry_delay(self.failures));
            }
        }
        if !key.is_empty()
            && !self.pending
            && self.next.is_some_and(|due| now >= due)
            && self
                .requests
                .try_send((self.generation, key.into()))
                .is_ok()
        {
            self.pending = true;
        }
        &self.tuning
    }
}
impl Default for Profiles {
    fn default() -> Self {
        Self::new()
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    /// A driver profile: setting id to its value and whether the profile stores it.
    fn store(settings: &[(u32, u32, bool)]) -> impl FnMut(u32) -> Option<Value> + '_ {
        move |id| {
            settings
                .iter()
                .find(|(setting, _, _)| *setting == id)
                .map(|&(_, value, own)| Value { value, own })
        }
    }

    #[test]
    fn profile_values_convert_to_rubylight_ranges_and_skip_the_rest() {
        let settings = [
            (0x00dd_48fe, 127, true),
            (0x00dd_48ff, 300, true),
            (0x00dd_48fd, 60, false),
            (0x00dd_48fc, 1500, true),
        ];
        let global = values(&mut store(&settings), false);
        assert_eq!(global.get("rtx_hdr_contrast"), Some(&27));
        assert_eq!(global.get("rtx_hdr_saturation"), None, "out of range");
        assert_eq!(global.get("rtx_hdr_middle_gray"), Some(&60));
        assert!(!global.contains_key("hdr_peak_brightness"));
        // A game's profile counts only what it stores itself.
        let app = values(&mut store(&settings), true);
        assert_eq!(app.get("rtx_hdr_contrast"), Some(&27));
        assert!(!app.contains_key("rtx_hdr_middle_gray"));
        assert!(values(&mut store(&[]), false).is_empty());
    }

    #[test]
    fn profiles_queries_on_executable_change_without_periodic_reload_churn() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let calls = Arc::new(AtomicUsize::new(0));
        let fail_next = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let c = calls.clone();
        let f = fail_next.clone();
        let mut profiles = Profiles::with_lookup(move |exe| {
            c.fetch_add(1, Ordering::SeqCst);
            if f.swap(false, Ordering::SeqCst) {
                return None;
            }
            Some(BTreeMap::from([(
                "rtx_hdr_contrast".to_owned(),
                if exe.ends_with("b.exe") { 40 } else { 20 },
            )]))
        });
        let wait_reply = |profiles: &mut Profiles, key: Option<&str>, at: Instant| {
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                let tuning = profiles.poll_at(key, at).clone();
                if !profiles.pending || Instant::now() >= deadline {
                    return tuning;
                }
                std::thread::sleep(Duration::from_millis(2));
            }
        };
        let t0 = Instant::now();
        assert!(profiles.poll_at(None, t0).is_empty());
        assert_eq!(calls.load(Ordering::SeqCst), 0);

        // First foreground game triggers one lookup and keeps its tuning without 5 s reload churn.
        let tuning = wait_reply(&mut profiles, Some(r"C:\Games\a.exe"), t0);
        assert_eq!(tuning.get("rtx_hdr_contrast"), Some(&20));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        for secs in [5, 15, 60, 3600] {
            let kept = profiles.poll_at(Some(r"C:\Games\a.exe"), t0 + Duration::from_secs(secs));
            assert_eq!(kept.get("rtx_hdr_contrast"), Some(&20));
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // Switching games re-queries immediately.
        let tuning = wait_reply(
            &mut profiles,
            Some(r"C:\Games\b.exe"),
            t0 + Duration::from_secs(10),
        );
        assert_eq!(tuning.get("rtx_hdr_contrast"), Some(&40));
        assert_eq!(calls.load(Ordering::SeqCst), 2);

        // Leaving and returning to the game re-queries (e.g. after NVIDIA App overlay edits).
        assert!(
            profiles
                .poll_at(None, t0 + Duration::from_secs(11))
                .is_empty()
        );
        let tuning = wait_reply(
            &mut profiles,
            Some(r"C:\Games\b.exe"),
            t0 + Duration::from_secs(12),
        );
        assert_eq!(tuning.get("rtx_hdr_contrast"), Some(&40));
        assert_eq!(calls.load(Ordering::SeqCst), 3);

        // A failed lookup backs off before retrying rather than spinning.
        fail_next.store(true, Ordering::SeqCst);
        let t1 = t0 + Duration::from_secs(20);
        let empty = wait_reply(&mut profiles, Some(r"C:\Games\c.exe"), t1);
        assert!(empty.is_empty());
        assert_eq!(calls.load(Ordering::SeqCst), 4);
        profiles.poll_at(Some(r"C:\Games\c.exe"), t1 + Duration::from_secs(5));
        assert_eq!(calls.load(Ordering::SeqCst), 4);
        let recovered = wait_reply(&mut profiles, Some(r"C:\Games\c.exe"), t1 + retry_delay(1));
        assert_eq!(recovered.get("rtx_hdr_contrast"), Some(&20));
        assert_eq!(calls.load(Ordering::SeqCst), 5);
    }
}
