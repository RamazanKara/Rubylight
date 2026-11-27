//! What live glass remembers between launches: the tier the last probe on this build and driver
//! settled on, and the crash guard around the probe (`nightjar_policy::glass_probe::Guard`). A
//! small text file beside the settings, `glass_probe.txt`; never a setting, never shown.

use std::path::PathBuf;

use nightjar_policy::glass_probe::{Guard, Tier};

/// The remembered state.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stored {
    /// The tier the last finished probe kept or stepped down to, for `guard.fingerprint`.
    pub tier: Option<Tier>,
    /// `probe_in_flight` and the build fingerprint it was armed for.
    pub guard: Guard,
    /// `probe_trips`: earlier probes in a row on `guard.fingerprint` that never finished, before
    /// the one armed now (`glass_probe::GUARD_TRIPS`).
    pub trips: u32,
}

impl Stored {
    fn parse(text: &str) -> Stored {
        let mut stored = Stored::default();
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else { continue };
            match key.trim() {
                "tier" => stored.tier = Tier::parse(value.trim()),
                "probe_in_flight" => stored.guard.in_flight = value.trim() == "1",
                "fingerprint" => stored.guard.fingerprint = value.trim().to_owned(),
                "probe_trips" => stored.trips = value.trim().parse().unwrap_or(0),
                _ => {}
            }
        }
        stored
    }

    fn text(&self) -> String {
        format!(
            "tier={}\nprobe_in_flight={}\nfingerprint={}\nprobe_trips={}\n",
            self.tier.map_or("", Tier::key),
            u8::from(self.guard.in_flight),
            self.guard.fingerprint.replace(['\r', '\n'], " "),
            self.trips
        )
    }
}

/// Where the state lives: a file in the app's settings folder, or (tests, screenshots, a folder
/// that cannot be found) only in memory.
#[derive(Debug, Default)]
pub struct Memory {
    path: Option<PathBuf>,
    stored: Stored,
}

/// The file's name in the settings folder.
pub const FILE: &str = "glass_probe.txt";

impl Memory {
    /// The app's: `glass_probe.txt` in the settings folder.
    pub fn in_config_dir() -> Memory {
        match crate::config::config_dir() {
            Ok(dir) => Memory::at(dir.join(FILE)),
            Err(why) => {
                eprintln!("nightjar: live glass remembers nothing between launches: {why}");
                Memory::default()
            }
        }
    }

    /// A file at `path`.
    pub fn at(path: PathBuf) -> Memory {
        let stored = std::fs::read_to_string(&path).map(|text| Stored::parse(&text)).unwrap_or_default();
        Memory { path: Some(path), stored }
    }

    /// Only in memory, starting from `stored`.
    pub fn in_memory(stored: Stored) -> Memory {
        Memory { path: None, stored }
    }

    pub fn stored(&self) -> &Stored {
        &self.stored
    }

    /// Commits `stored`: written through before the caller goes on, so a crash right after finds it.
    pub fn save(&mut self, stored: Stored) {
        if stored == self.stored {
            return;
        }
        self.stored = stored;
        if let Some(path) = &self.path
            && let Err(why) = std::fs::write(path, self.stored.text())
        {
            eprintln!("nightjar: live glass could not write {}: {why}", path.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_state_reads_back_as_written() {
        let stored = Stored { tier: Some(Tier::Live), guard: Guard::arm("7.0.0 windows direct3d11 10de:2c05 rev a1 driver 32.0.15.6614"), trips: 1 };
        assert_eq!(Stored::parse(&stored.text()), stored);
        // A file from before the trips were counted reads as none.
        assert_eq!(Stored::parse("tier=\nprobe_in_flight=1\nfingerprint=fp\n").trips, 0);
        assert_eq!(Stored::parse(""), Stored::default());
        assert_eq!(Stored::parse("tier=nonsense\nprobe_in_flight=1\n").tier, None);
    }

    #[test]
    fn a_file_keeps_the_guard_for_the_next_launch() {
        let dir = std::env::temp_dir().join(format!("nightjar-glass-memory-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(FILE);
        let mut memory = Memory::at(path.clone());
        assert_eq!(memory.stored(), &Stored::default(), "no file yet");
        memory.save(Stored { tier: None, guard: Guard::arm("fp"), trips: 0 });
        assert!(Memory::at(path.clone()).stored().guard.tripped("fp"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
