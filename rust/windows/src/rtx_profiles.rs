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
    elapsed: Duration,
}
pub struct Profiles {
    requests: mpsc::SyncSender<(u64, String)>,
    reply: Arc<Mutex<Option<Reply>>>,
    key: String,
    generation: u64,
    pending: bool,
    next: Instant,
    failures: u32,
    tuning: Tuning,
}
impl Profiles {
    pub fn new() -> Self {
        let (requests, receiver) = mpsc::sync_channel::<(u64, String)>(1);
        let reply = Arc::new(Mutex::new(None));
        let output = reply.clone();
        std::thread::spawn(move || {
            while let Ok((generation, key)) = receiver.recv() {
                let start = Instant::now();
                let tuning = lookup(&key);
                *output.lock().unwrap() = Some(Reply {
                    generation,
                    key,
                    tuning,
                    elapsed: start.elapsed(),
                });
            }
        });
        Self {
            requests,
            reply,
            key: String::new(),
            generation: 0,
            pending: false,
            next: Instant::now(),
            failures: 0,
            tuning: Tuning::new(),
        }
    }
    pub fn poll(&mut self, key: Option<&str>) -> &Tuning {
        let key = key.unwrap_or("");
        if key != self.key {
            self.key = key.into();
            self.generation = self.generation.wrapping_add(1);
            self.tuning.clear();
            self.pending = false;
            self.failures = 0;
            self.next = Instant::now();
        }
        if let Some(reply) = self.reply.lock().unwrap().take()
            && reply.generation == self.generation
            && reply.key == self.key
        {
            self.pending = false;
            if reply.tuning.is_none() || reply.elapsed > Duration::from_millis(100) {
                self.failures += 1;
            } else {
                self.failures = 0;
            }
            if let Some(tuning) = reply.tuning {
                self.tuning = tuning;
            }
            self.next = Instant::now()
                + Duration::from_secs(match self.failures {
                    0 => 5,
                    1 => 15,
                    _ => 30,
                });
        }
        if !key.is_empty()
            && !self.pending
            && Instant::now() >= self.next
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
}
