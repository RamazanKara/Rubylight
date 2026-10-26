//! Start a stream from the command line, as game frontends (ES-DE, Pegasus, Playnite) do:
//!
//! `nightjar --host <name|uuid> [--app <name|uuid>] [--quit-on-exit]`
//!
//! The PC must be saved and paired. Without `--app` its app list opens; with it the app starts.
//! `--quit-on-exit` closes Nightjar when that stream ends, so the frontend gets its window back.
//! A PC or app that cannot be found, or a stream that cannot start, ends the process with an
//! error (exit code 1) instead of leaving the launcher open; a wrong command line exits with 64.

use std::ffi::OsString;
use std::time::{Duration, Instant};

/// How long the PC gets to answer and list its apps.
const START_LIMIT: Duration = Duration::from_secs(60);

pub const USAGE: &str = "usage: nightjar [--host <name|uuid> [--app <name|uuid>] [--quit-on-exit]]";

#[derive(Debug, PartialEq, Eq)]
pub struct Launch {
    pub host: String,
    pub app: Option<String>,
    pub quit_on_exit: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Parsed {
    /// No arguments: the launcher as usual.
    Gui,
    Help,
    Launch(Launch),
}

/// Reads the arguments after the program name. `--name value` and `--name=value` both work.
pub fn parse<I: IntoIterator<Item = OsString>>(args: I) -> Result<Parsed, String> {
    let mut host = None;
    let mut app = None;
    let mut quit_on_exit = false;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let arg = arg
            .into_string()
            .map_err(|a| format!("{} is not valid text", a.to_string_lossy()))?;
        let (flag, inline) = match arg.split_once('=') {
            Some((flag, value)) if flag.starts_with("--") => {
                (flag.to_owned(), Some(value.to_owned()))
            }
            _ => (arg, None),
        };
        match flag.as_str() {
            "--help" | "-h" if inline.is_none() => return Ok(Parsed::Help),
            "--quit-on-exit" if inline.is_none() => quit_on_exit = true,
            "--host" | "--app" => {
                let value = match inline {
                    Some(value) => value,
                    None => args
                        .next()
                        .and_then(|v| v.into_string().ok())
                        .filter(|v| !v.starts_with("--"))
                        .ok_or_else(|| format!("{flag} needs a value"))?,
                };
                let value = value.trim().to_owned();
                if value.is_empty() {
                    return Err(format!("{flag} needs a value"));
                }
                let slot = if flag == "--host" {
                    &mut host
                } else {
                    &mut app
                };
                if slot.replace(value).is_some() {
                    return Err(format!("{flag} is given twice"));
                }
            }
            _ => return Err(format!("unknown argument {flag}")),
        }
    }
    match host {
        Some(host) => Ok(Parsed::Launch(Launch {
            host,
            app,
            quit_on_exit,
        })),
        None if app.is_some() => Err("--app needs --host".into()),
        None if quit_on_exit => Err("--quit-on-exit needs --host".into()),
        None => Ok(Parsed::Gui),
    }
}

/// A saved PC as `find_host` sees it.
pub struct HostEntry<'a> {
    pub id: u64,
    pub unique_id: &'a str,
    pub name: &'a str,
    pub alias: Option<&'a str>,
    pub paired: bool,
}

/// The saved PC with this unique id, or else the one with this name (its own or the one given
/// on this device), ignoring case. Unknown, ambiguous and unpaired PCs are errors.
pub fn find_host<'a>(
    hosts: impl IntoIterator<Item = HostEntry<'a>>,
    wanted: &str,
) -> Result<u64, String> {
    let hosts: Vec<HostEntry> = hosts.into_iter().collect();
    let found = match hosts
        .iter()
        .find(|h| !h.unique_id.is_empty() && h.unique_id.eq_ignore_ascii_case(wanted))
    {
        Some(host) => host,
        None => {
            let named: Vec<&HostEntry> = hosts
                .iter()
                .filter(|h| {
                    h.name.eq_ignore_ascii_case(wanted)
                        || h.alias.is_some_and(|a| a.eq_ignore_ascii_case(wanted))
                })
                .collect();
            match named.as_slice() {
                [] => return Err(format!("no saved PC is named {wanted} or has that UUID")),
                [host] => *host,
                _ => {
                    return Err(format!(
                        "several saved PCs are named {wanted}; give its UUID instead"
                    ))
                }
            }
        }
    };
    if !found.paired {
        return Err(format!(
            "the PC {wanted} is not paired; pair it in Nightjar first"
        ));
    }
    Ok(found.id)
}

#[derive(Debug, Clone)]
pub struct AppEntry {
    pub id: i32,
    pub uuid: String,
    pub name: String,
}

/// The app with this UUID, or else the one with this name, ignoring case.
pub fn find_app(apps: &[AppEntry], wanted: &str) -> Result<i32, String> {
    if let Some(app) = apps
        .iter()
        .find(|a| !a.uuid.is_empty() && a.uuid.eq_ignore_ascii_case(wanted))
    {
        return Ok(app.id);
    }
    let named: Vec<&AppEntry> = apps
        .iter()
        .filter(|a| a.name.eq_ignore_ascii_case(wanted))
        .collect();
    match named.as_slice() {
        [] => Err(format!(
            "the PC has no app named {wanted} or with that UUID"
        )),
        [app] => Ok(app.id),
        _ => Err(format!(
            "the PC has several apps named {wanted}; give its UUID instead"
        )),
    }
}

pub enum AppsSeen {
    NotLoaded,
    Loading,
    Loaded(Vec<AppEntry>),
    Failed(String),
}

pub struct HostSeen {
    pub answered: bool,
    pub paired: bool,
    pub apps: AppsSeen,
}

pub struct Seen {
    pub identity: bool,
    /// None once the PC is no longer saved.
    pub host: Option<HostSeen>,
    /// The launcher's error message, if one is up.
    pub message: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Step {
    Wait,
    OpenApps(u64),
    LoadApps(u64),
    Launch(u64, i32),
    Fail(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Starting,
    /// The launch was asked for; waiting for the stream.
    Launched,
    Streaming,
    /// Nothing more to drive (the app list is open, or the stream ended without quitting).
    Idle,
}

/// Drives one command-line start through the launcher, one step per frame.
pub struct CliLaunch {
    pub launch: Launch,
    pub host: u64,
    phase: Phase,
    apps_asked: bool,
    started: Instant,
    /// Why the start failed; the process then exits with an error.
    pub failed: Option<String>,
}

impl CliLaunch {
    pub fn new(launch: Launch, hosts: &crate::hosts::Hosts) -> Result<Self, String> {
        let entries = hosts.list.iter().map(|h| HostEntry {
            id: h.id,
            unique_id: &h.record.unique_id,
            name: &h.record.name,
            alias: h.record.alias.as_deref().filter(|a| !a.is_empty()),
            paired: h.record.server_certificate.is_some(),
        });
        let host = find_host(entries, &launch.host)?;
        Ok(Self::resolved(launch, host))
    }

    fn resolved(launch: Launch, host: u64) -> Self {
        Self {
            launch,
            host,
            phase: Phase::Starting,
            apps_asked: false,
            started: Instant::now(),
            failed: None,
        }
    }

    pub fn host_seen(&self, hosts: &crate::hosts::Hosts) -> Option<HostSeen> {
        use crate::hosts::Apps;
        let host = hosts.get(self.host)?;
        Some(HostSeen {
            answered: host.info.is_some(),
            paired: host.paired(),
            apps: match &host.apps {
                Apps::NotLoaded => AppsSeen::NotLoaded,
                Apps::Loading => AppsSeen::Loading,
                Apps::Loaded(apps) => AppsSeen::Loaded(
                    apps.iter()
                        .map(|a| AppEntry {
                            id: a.id,
                            uuid: a.uuid.clone(),
                            name: a.name.clone(),
                        })
                        .collect(),
                ),
                Apps::Failed(e) => AppsSeen::Failed(e.clone()),
            },
        })
    }

    /// What to do next, given what the launcher can see now. Each action is asked for once.
    pub fn next(&mut self, seen: &Seen) -> Step {
        match self.phase {
            Phase::Streaming | Phase::Idle => return Step::Wait,
            Phase::Launched => {
                return match &seen.message {
                    Some(message) => Step::Fail(message.clone()),
                    None => Step::Wait,
                }
            }
            Phase::Starting => {}
        }
        let Some(host) = &seen.host else {
            return Step::Fail(format!("the PC {} is no longer saved", self.launch.host));
        };
        if !seen.identity {
            return self.in_time(Step::Wait);
        }
        let Some(wanted) = self.launch.app.clone() else {
            self.phase = Phase::Idle;
            return Step::OpenApps(self.host);
        };
        if !host.answered {
            return self.in_time(Step::Wait);
        }
        if !host.paired {
            return Step::Fail(format!(
                "the PC {} is not paired; pair it in Nightjar first",
                self.launch.host
            ));
        }
        match &host.apps {
            AppsSeen::NotLoaded if !self.apps_asked => {
                self.apps_asked = true;
                Step::LoadApps(self.host)
            }
            AppsSeen::NotLoaded | AppsSeen::Loading => self.in_time(Step::Wait),
            AppsSeen::Failed(e) => Step::Fail(format!("could not load the apps: {e}")),
            AppsSeen::Loaded(apps) => match find_app(apps, &wanted) {
                Ok(id) => {
                    self.phase = Phase::Launched;
                    Step::Launch(self.host, id)
                }
                Err(why) => Step::Fail(why),
            },
        }
    }

    fn in_time(&self, step: Step) -> Step {
        if self.started.elapsed() > START_LIMIT {
            Step::Fail(format!(
                "the PC {} did not answer in time",
                self.launch.host
            ))
        } else {
            step
        }
    }

    pub fn streaming_began(&mut self) {
        self.phase = Phase::Streaming;
    }

    /// The stream is over (no reconnect follows). True when Nightjar should close now;
    /// `message` is the error the launcher shows for it, if any.
    pub fn stream_ended(&mut self, message: Option<String>) -> bool {
        if self.phase != Phase::Streaming {
            return false;
        }
        self.phase = Phase::Idle;
        if !self.launch.quit_on_exit {
            return false;
        }
        if let Some(message) = message {
            self.fail(&message);
        }
        true
    }

    pub fn fail(&mut self, why: &str) {
        eprintln!("nightjar: --host start failed: {why}");
        self.failed = Some(why.to_owned());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Result<Parsed, String> {
        parse(list.iter().map(OsString::from))
    }

    fn launch(host: &str, app: Option<&str>, quit_on_exit: bool) -> Parsed {
        Parsed::Launch(Launch {
            host: host.into(),
            app: app.map(Into::into),
            quit_on_exit,
        })
    }

    #[test]
    fn no_arguments_open_the_launcher() {
        assert_eq!(args(&[]), Ok(Parsed::Gui));
    }

    #[test]
    fn host_app_and_quit_parse_in_any_order_and_both_forms() {
        assert_eq!(
            args(&["--host", "Gaming PC"]),
            Ok(launch("Gaming PC", None, false))
        );
        assert_eq!(
            args(&[
                "--quit-on-exit",
                "--app",
                "Steam Big Picture",
                "--host",
                "pc"
            ]),
            Ok(launch("pc", Some("Steam Big Picture"), true))
        );
        assert_eq!(
            args(&["--host=pc", "--app=Desktop"]),
            Ok(launch("pc", Some("Desktop"), false))
        );
        assert_eq!(args(&["--host", " pc "]), Ok(launch("pc", None, false)));
        assert_eq!(args(&["--help"]), Ok(Parsed::Help));
    }

    #[test]
    fn wrong_command_lines_are_errors() {
        assert!(args(&["--host"]).is_err());
        assert!(args(&["--host", "--app", "x"]).is_err());
        assert!(args(&["--host="]).is_err());
        assert!(args(&["--host", "a", "--host", "b"]).is_err());
        assert!(args(&["--app", "Desktop"]).is_err());
        assert!(args(&["--quit-on-exit"]).is_err());
        assert!(args(&["--host", "pc", "--fullscreen"]).is_err());
        assert!(args(&["--quit-on-exit=yes", "--host", "pc"]).is_err());
        assert!(args(&["pc"]).is_err());
    }

    fn entry<'a>(
        id: u64,
        unique_id: &'a str,
        name: &'a str,
        alias: Option<&'a str>,
        paired: bool,
    ) -> HostEntry<'a> {
        HostEntry {
            id,
            unique_id,
            name,
            alias,
            paired,
        }
    }

    #[test]
    fn hosts_match_by_uuid_then_name_or_alias() {
        let hosts = || {
            vec![
                entry(1, "AAAA-1111", "DESKTOP-1", Some("Living room"), true),
                entry(2, "BBBB-2222", "aaaa-1111", None, true),
                entry(3, "", "Office", None, true),
            ]
        };
        assert_eq!(
            find_host(hosts(), "aaaa-1111"),
            Ok(1),
            "a UUID wins over a name"
        );
        assert_eq!(find_host(hosts(), "desktop-1"), Ok(1));
        assert_eq!(find_host(hosts(), "LIVING ROOM"), Ok(1));
        assert_eq!(find_host(hosts(), "office"), Ok(3));
        assert!(
            find_host(hosts(), "").is_err(),
            "an empty UUID matches nothing"
        );
        assert!(find_host(hosts(), "Garage").unwrap_err().contains("Garage"));
    }

    #[test]
    fn ambiguous_and_unpaired_hosts_are_errors() {
        let twins = vec![
            entry(1, "a", "PC", None, true),
            entry(2, "b", "pc", None, true),
        ];
        assert!(find_host(twins, "pc").unwrap_err().contains("UUID"));
        let unpaired = vec![entry(1, "a", "PC", None, false)];
        assert!(find_host(unpaired, "PC")
            .unwrap_err()
            .contains("not paired"));
    }

    fn app(id: i32, uuid: &str, name: &str) -> AppEntry {
        AppEntry {
            id,
            uuid: uuid.into(),
            name: name.into(),
        }
    }

    #[test]
    fn apps_match_by_uuid_then_name() {
        let apps = vec![
            app(1, "", "Desktop"),
            app(2, "5E1F-77", "Steam Big Picture"),
            app(3, "x", "Steam Big Picture"),
        ];
        assert_eq!(find_app(&apps, "desktop"), Ok(1));
        assert_eq!(find_app(&apps, "5e1f-77"), Ok(2));
        assert!(find_app(&apps, "steam big picture")
            .unwrap_err()
            .contains("UUID"));
        assert!(find_app(&apps, "Cyberpunk")
            .unwrap_err()
            .contains("Cyberpunk"));
    }

    fn cli(app: Option<&str>, quit_on_exit: bool) -> CliLaunch {
        CliLaunch::resolved(
            Launch {
                host: "pc".into(),
                app: app.map(Into::into),
                quit_on_exit,
            },
            7,
        )
    }

    fn seen(answered: bool, apps: AppsSeen) -> Seen {
        Seen {
            identity: true,
            host: Some(HostSeen {
                answered,
                paired: true,
                apps,
            }),
            message: None,
        }
    }

    #[test]
    fn an_app_is_loaded_and_launched_once() {
        let mut c = cli(Some("desktop"), false);
        assert_eq!(
            c.next(&Seen {
                identity: false,
                ..seen(false, AppsSeen::NotLoaded)
            }),
            Step::Wait
        );
        assert_eq!(c.next(&seen(false, AppsSeen::NotLoaded)), Step::Wait);
        assert_eq!(c.next(&seen(true, AppsSeen::NotLoaded)), Step::LoadApps(7));
        assert_eq!(c.next(&seen(true, AppsSeen::NotLoaded)), Step::Wait);
        assert_eq!(c.next(&seen(true, AppsSeen::Loading)), Step::Wait);
        let apps = || AppsSeen::Loaded(vec![app(1, "", "Steam"), app(5, "", "Desktop")]);
        assert_eq!(c.next(&seen(true, apps())), Step::Launch(7, 5));
        assert_eq!(c.next(&seen(true, apps())), Step::Wait);
    }

    #[test]
    fn without_an_app_the_app_list_opens_once() {
        let mut c = cli(None, false);
        assert_eq!(c.next(&seen(false, AppsSeen::NotLoaded)), Step::OpenApps(7));
        assert_eq!(c.next(&seen(true, AppsSeen::NotLoaded)), Step::Wait);
    }

    #[test]
    fn a_missing_app_a_failed_list_a_silent_pc_or_a_failed_start_are_errors() {
        let mut c = cli(Some("Cyberpunk"), false);
        assert!(matches!(
            c.next(&seen(true, AppsSeen::Loaded(vec![app(1, "", "Steam")]))),
            Step::Fail(_)
        ));
        let mut c = cli(Some("Steam"), false);
        assert!(matches!(
            c.next(&seen(true, AppsSeen::Failed("timeout".into()))),
            Step::Fail(_)
        ));
        let mut c = cli(Some("Steam"), false);
        assert!(matches!(
            c.next(&Seen {
                host: None,
                ..seen(true, AppsSeen::NotLoaded)
            }),
            Step::Fail(_)
        ));
        let mut c = cli(Some("Steam"), false);
        c.started = Instant::now() - START_LIMIT - Duration::from_secs(1);
        assert!(matches!(
            c.next(&seen(false, AppsSeen::NotLoaded)),
            Step::Fail(_)
        ));
        let mut c = cli(Some("Steam"), false);
        assert_eq!(
            c.next(&seen(true, AppsSeen::Loaded(vec![app(1, "", "Steam")]))),
            Step::Launch(7, 1)
        );
        let failed = Seen {
            message: Some("The stream could not start: no video".into()),
            ..seen(true, AppsSeen::NotLoaded)
        };
        assert_eq!(
            c.next(&failed),
            Step::Fail("The stream could not start: no video".into())
        );
    }

    #[test]
    fn quit_on_exit_closes_after_the_stream_and_keeps_its_error() {
        let mut c = cli(Some("Steam"), true);
        assert!(!c.stream_ended(None), "no stream ran yet");
        c.streaming_began();
        assert!(c.stream_ended(None));
        assert_eq!(c.failed, None);
        let mut c = cli(Some("Steam"), true);
        c.streaming_began();
        assert!(c.stream_ended(Some("The stream ended: host error".into())));
        assert_eq!(c.failed.as_deref(), Some("The stream ended: host error"));
        let mut c = cli(Some("Steam"), false);
        c.streaming_began();
        assert!(
            !c.stream_ended(None),
            "without --quit-on-exit the launcher stays"
        );
    }
}
