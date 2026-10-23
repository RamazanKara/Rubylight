//! Measurement only: start one stream without the GUI and end it after a set time.
//!
//! `NIGHTJAR_AUTOSTREAM=address[:port]` names the PC, `NIGHTJAR_AUTOSTREAM_APP` the app (default
//! "Desktop") and `NIGHTJAR_AUTOSTREAM_SECONDS` the length (default 60). A PC that is not paired
//! yet is paired with `NIGHTJAR_AUTOSTREAM_PIN` (default 1234), which someone has to enter on the
//! PC. The process exits with 0 after a stream that ran its time and 2 when anything else
//! happened, so a script can tell.

use std::time::{Duration, Instant};

const START_LIMIT: Duration = Duration::from_secs(90);

pub struct Autostream {
    pub address: String,
    pub port: Option<u16>,
    pub app: String,
    pub seconds: u64,
    pub pin: String,
    host: Option<u64>,
    paired_asked: bool,
    apps_asked: bool,
    launched: bool,
    streaming: bool,
    started: Instant,
    pub failed: bool,
}

pub enum AppsSeen {
    NotLoaded,
    Loading,
    Loaded(Vec<(i32, String)>),
    Failed(String),
}

pub struct HostSeen {
    pub id: u64,
    pub answered: bool,
    pub paired: bool,
    pub apps: AppsSeen,
}

pub struct Seen {
    pub identity: bool,
    pub host: Option<HostSeen>,
    pub pair_error: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Step {
    Wait,
    AddHost,
    Pair(u64),
    LoadApps(u64),
    Launch(u64, i32),
    Fail(String),
}

impl Autostream {
    pub fn from_env() -> Option<Self> {
        let target = std::env::var("NIGHTJAR_AUTOSTREAM").ok().filter(|v| !v.trim().is_empty())?;
        let (address, port) = split_address(target.trim());
        Some(Self {
            address,
            port,
            app: std::env::var("NIGHTJAR_AUTOSTREAM_APP").ok().filter(|v| !v.is_empty()).unwrap_or_else(|| "Desktop".into()),
            seconds: std::env::var("NIGHTJAR_AUTOSTREAM_SECONDS").ok().and_then(|v| v.parse().ok()).filter(|s| *s > 0).unwrap_or(60),
            pin: std::env::var("NIGHTJAR_AUTOSTREAM_PIN").ok().filter(|v| !v.is_empty()).unwrap_or_else(|| "1234".into()),
            host: None,
            paired_asked: false,
            apps_asked: false,
            launched: false,
            streaming: false,
            started: Instant::now(),
            failed: false,
        })
    }

    pub fn host_seen(&self, hosts: &crate::hosts::Hosts) -> Option<HostSeen> {
        use crate::hosts::Apps;
        let host = hosts.get(self.host?)?;
        Some(HostSeen {
            id: host.id,
            answered: host.info.is_some(),
            paired: host.paired(),
            apps: match &host.apps {
                Apps::NotLoaded => AppsSeen::NotLoaded,
                Apps::Loading => AppsSeen::Loading,
                Apps::Loaded(apps) => AppsSeen::Loaded(apps.iter().map(|app| (app.id, app.name.clone())).collect()),
                Apps::Failed(e) => AppsSeen::Failed(e.clone()),
            },
        })
    }

    pub fn host_added(&mut self, id: u64) {
        self.host = Some(id);
    }

    pub fn streaming_began(&mut self) {
        self.streaming = true;
    }

    pub fn length(&self) -> Duration {
        Duration::from_secs(self.seconds)
    }

    /// What to do next, given what the launcher can see now. Each action is asked for once.
    pub fn next(&mut self, seen: &Seen) -> Step {
        if self.streaming {
            return Step::Wait;
        }
        if self.started.elapsed() > START_LIMIT {
            return Step::Fail(if self.launched { "the stream did not start in time" } else { "no paired PC with the app in time" }.into());
        }
        if self.launched {
            return Step::Wait;
        }
        let Some(host) = seen.host.as_ref().filter(|h| Some(h.id) == self.host) else {
            return if self.host.is_none() { Step::AddHost } else { Step::Wait };
        };
        if !host.answered || !seen.identity {
            return Step::Wait;
        }
        if !host.paired {
            if seen.pair_error {
                return Step::Fail("pairing failed".into());
            }
            if self.paired_asked {
                return Step::Wait;
            }
            self.paired_asked = true;
            return Step::Pair(host.id);
        }
        match &host.apps {
            AppsSeen::NotLoaded if !self.apps_asked => {
                self.apps_asked = true;
                Step::LoadApps(host.id)
            }
            AppsSeen::NotLoaded | AppsSeen::Loading => Step::Wait,
            AppsSeen::Failed(e) => Step::Fail(format!("could not load the apps: {e}")),
            AppsSeen::Loaded(apps) => match apps.iter().find(|(_, name)| name.eq_ignore_ascii_case(&self.app)) {
                Some((id, _)) => {
                    self.launched = true;
                    self.started = Instant::now();
                    Step::Launch(host.id, *id)
                }
                None => Step::Fail(format!("the PC has no app named {}", self.app)),
            },
        }
    }

    pub fn fail(&mut self, why: &str) {
        eprintln!("nightjar: autostream failed: {why}");
        self.failed = true;
    }
}

fn split_address(target: &str) -> (String, Option<u16>) {
    match target.rsplit_once(':') {
        Some((address, port)) if !address.contains(':') => match port.parse() {
            Ok(port) => (address.to_owned(), Some(port)),
            Err(_) => (target.to_owned(), None),
        },
        _ => (target.to_owned(), None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn auto() -> Autostream {
        Autostream {
            address: "127.0.0.1".into(), port: Some(48123), app: "Desktop".into(), seconds: 60, pin: "1234".into(),
            host: None, paired_asked: false, apps_asked: false, launched: false, streaming: false,
            started: Instant::now(), failed: false,
        }
    }

    fn seen(paired: bool, apps: AppsSeen) -> Seen {
        Seen { identity: true, host: Some(HostSeen { id: 7, answered: true, paired, apps }), pair_error: false }
    }

    #[test]
    fn addresses_split_into_host_and_port() {
        assert_eq!(split_address("127.0.0.1:48123"), ("127.0.0.1".into(), Some(48123)));
        assert_eq!(split_address("pc.local"), ("pc.local".into(), None));
        assert_eq!(split_address("::1"), ("::1".into(), None));
    }

    #[test]
    fn it_adds_pairs_loads_and_launches_each_step_once() {
        let mut a = auto();
        assert_eq!(a.next(&Seen { identity: true, host: None, pair_error: false }), Step::AddHost);
        a.host_added(7);
        assert_eq!(a.next(&seen(false, AppsSeen::NotLoaded)), Step::Pair(7));
        assert_eq!(a.next(&seen(false, AppsSeen::NotLoaded)), Step::Wait);
        assert_eq!(a.next(&seen(true, AppsSeen::NotLoaded)), Step::LoadApps(7));
        assert_eq!(a.next(&seen(true, AppsSeen::Loading)), Step::Wait);
        let apps = || AppsSeen::Loaded(vec![(1, "Steam".into()), (5, "desktop".into())]);
        assert_eq!(a.next(&seen(true, apps())), Step::Launch(7, 5));
        assert_eq!(a.next(&seen(true, apps())), Step::Wait);
    }

    #[test]
    fn it_fails_loudly_on_a_missing_app_a_failed_pairing_or_a_slow_start() {
        let mut a = auto();
        a.host_added(7);
        assert!(matches!(a.next(&seen(true, AppsSeen::Loaded(vec![(1, "Steam".into())]))), Step::Fail(_)));
        let mut a = auto();
        a.host_added(7);
        let mut broken = seen(false, AppsSeen::NotLoaded);
        broken.pair_error = true;
        assert!(matches!(a.next(&broken), Step::Fail(_)));
        let mut a = auto();
        a.host_added(7);
        a.launched = true;
        a.started = Instant::now() - START_LIMIT - Duration::from_secs(1);
        assert!(matches!(a.next(&seen(true, AppsSeen::NotLoaded)), Step::Fail(_)));
        a.streaming_began();
        assert_eq!(a.next(&seen(true, AppsSeen::NotLoaded)), Step::Wait);
    }
}
