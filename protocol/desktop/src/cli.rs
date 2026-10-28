//! Start a stream from the command line, as game frontends (ES-DE, Pegasus, Playnite) do:
//!
//! `nightjar --host <name|uuid> [--app <name|uuid>] [--quit-on-exit]`
//!
//! The PC must be saved and paired. Without `--app` its app list opens; with it the app starts.
//! `--quit-on-exit` closes Nightjar when that stream ends, so the frontend gets its window back.
//! A PC or app that cannot be found, or a stream that cannot start, ends the process with an
//! error (exit code 1) instead of leaving the launcher open; a wrong command line exits with 64.
//! Cancelling the start closes Nightjar with exit code 0.

use std::ffi::OsString;
use std::time::{Duration, Instant};

use nightjar_core::http::{App, AppControl};

/// How long the PC gets to answer and list its apps.
const START_LIMIT: Duration = Duration::from_secs(60);
/// How long a launched stream gets to start: a backstop, the connection's own steps time out
/// sooner.
const LAUNCH_LIMIT: Duration = Duration::from_secs(180);

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

/// Whether an error that ends Nightjar is shown in a box too. A frontend start only writes it to
/// stderr and the log: a box could open behind the fullscreen frontend and keep Nightjar open.
pub fn shows_error_box(launch: Option<&Launch>) -> bool {
    launch.is_none()
}

/// Whether this build has a console for the usage text: a Windows release build has none.
pub const HAS_CONSOLE: bool = !cfg!(all(windows, not(debug_assertions)));

/// Whether the usage text (`--help`, or a wrong command line) is shown in a box too: only by a
/// build without a console, where printing it shows nothing, and never for a command line that
/// names a PC: that is a frontend's start, where a box could open behind it and keep Nightjar
/// open (see [`shows_error_box`]).
pub fn shows_usage_box<S: AsRef<std::ffi::OsStr>>(has_console: bool, args: &[S]) -> bool {
    let names_a_host = args.iter().any(|arg| {
        let arg = arg.as_ref().to_string_lossy();
        arg == "--host" || arg.starts_with("--host=")
    });
    !has_console && !names_a_host
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

/// The app with this UUID, or else the one with this name, ignoring case. Host actions
/// (disconnect, terminate) are no apps to stream.
pub fn find_app<'a>(apps: &'a [App], wanted: &str) -> Result<&'a App, String> {
    let found = match apps
        .iter()
        .find(|a| !a.uuid.is_empty() && a.uuid.eq_ignore_ascii_case(wanted))
    {
        Some(app) => app,
        None => {
            let named: Vec<&App> = apps
                .iter()
                .filter(|a| a.name.eq_ignore_ascii_case(wanted))
                .collect();
            match named.as_slice() {
                [] => {
                    return Err(format!(
                        "the PC has no app named {wanted} or with that UUID"
                    ))
                }
                [app] => *app,
                _ => {
                    return Err(format!(
                        "the PC has several apps named {wanted}; give its UUID instead"
                    ))
                }
            }
        }
    };
    if found.is_control_action() {
        return Err(format!(
            "{} is an action on the PC, not an app to stream",
            found.name
        ));
    }
    Ok(found)
}

/// Whether starting `app` resumes it, by the rules of a click on its poster (`ui.rs`). Another
/// app running is an error: the PC refuses a plain launch next to it unless it asks the user
/// itself.
fn resume(host: &HostSeen, apps: &[App], app: &App) -> Result<bool, String> {
    if app.control != AppControl::None {
        return Ok(false);
    }
    let Some((id, uuid)) = &host.running else {
        return Ok(false);
    };
    if app.matches_running_app(*id, uuid.as_deref()) {
        return Ok(true);
    }
    if host.confirms_replacement {
        return Ok(false);
    }
    let running = apps
        .iter()
        .find(|a| a.matches_running_app(*id, uuid.as_deref()))
        .map_or("another app", |a| a.name.as_str());
    Err(format!(
        "the PC is running {running}; quit it first or start that app instead"
    ))
}

pub enum AppsSeen {
    NotLoaded,
    Loading,
    Loaded(Vec<App>),
    Failed(String),
}

pub struct HostSeen {
    pub answered: bool,
    pub paired: bool,
    /// Why the PC could not be reached, when its last check failed.
    pub offline: Option<String>,
    /// The app the PC runs, as it reports it: its id and UUID.
    pub running: Option<(i32, Option<String>)>,
    pub confirms_replacement: bool,
    pub apps: AppsSeen,
}

pub struct Seen {
    pub identity: bool,
    /// None once the PC is no longer saved.
    pub host: Option<HostSeen>,
    /// The launcher's error message, if one is up.
    pub message: Option<String>,
    /// A launch is under way: the network check, the connection or the stream.
    pub busy: bool,
    /// The launcher asks the user to confirm closing the app the PC runs.
    pub confirming: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Step {
    Wait,
    OpenApps(u64),
    LoadApps(u64),
    /// Host, app, and whether to resume it.
    Launch(u64, i32, bool),
    Fail(String),
    /// The user cancelled the start: close without an error.
    Close,
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
    /// The app list was asked for and has not started loading since.
    apps_asked: bool,
    /// When the current phase began.
    started: Instant,
    /// The user cancelled the launch.
    cancelled: bool,
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
            cancelled: false,
            failed: None,
        }
    }

    pub fn host_seen(&self, hosts: &crate::hosts::Hosts) -> Option<HostSeen> {
        use crate::hosts::{Apps, Reach};
        let host = hosts.get(self.host)?;
        Some(HostSeen {
            answered: host.info.is_some(),
            paired: host.paired(),
            offline: (host.reach == Reach::Offline).then(|| host.error.clone().unwrap_or_default()),
            running: host.running_app().map(|id| {
                let uuid = host.info.as_ref().and_then(|i| i.current_game_uuid.clone());
                (id, uuid)
            }),
            confirms_replacement: host.confirms_replacement(),
            apps: match &host.apps {
                Apps::NotLoaded => AppsSeen::NotLoaded,
                Apps::Loading => AppsSeen::Loading,
                Apps::Loaded(apps) => AppsSeen::Loaded(apps.clone()),
                Apps::Failed(e) => AppsSeen::Failed(e.clone()),
            },
        })
    }

    /// What to do next, given what the launcher can see now. Each action is asked for once.
    pub fn next(&mut self, seen: &Seen) -> Step {
        match self.phase {
            Phase::Streaming | Phase::Idle => return Step::Wait,
            Phase::Launched => return self.launched(seen),
            Phase::Starting => {}
        }
        // An error up now stops the start. None is up when the launch goes out, so every
        // message in `Launched` is about the launch.
        if let Some(message) = &seen.message {
            return Step::Fail(message.clone());
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
        if let Some(why) = &host.offline {
            return Step::Fail(format!(
                "the PC {} cannot be reached: {why}",
                self.launch.host
            ));
        }
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
            AppsSeen::NotLoaded => self.in_time(Step::Wait),
            AppsSeen::Loading => {
                // A list that drops back to unloaded later is asked for again.
                self.apps_asked = false;
                self.in_time(Step::Wait)
            }
            AppsSeen::Failed(e) => Step::Fail(format!("could not load the apps: {e}")),
            AppsSeen::Loaded(apps) => {
                match find_app(apps, &wanted).and_then(|app| Ok((app.id, resume(host, apps, app)?)))
                {
                    Ok((id, resume)) => {
                        self.phase = Phase::Launched;
                        self.started = Instant::now();
                        Step::Launch(self.host, id, resume)
                    }
                    Err(why) => Step::Fail(why),
                }
            }
        }
    }

    /// After the launch was asked for: the stream starts, or the launch ends without one.
    fn launched(&mut self, seen: &Seen) -> Step {
        if let Some(message) = &seen.message {
            return Step::Fail(message.clone());
        }
        if seen.confirming {
            // The user takes their time; the launch that follows the answer gets the full limit.
            self.started = Instant::now();
            return Step::Wait;
        }
        if !seen.busy {
            // The launcher dropped the launch without saying why, or the user cancelled it.
            return if self.cancelled {
                Step::Close
            } else {
                Step::Fail("the stream did not start".into())
            };
        }
        if self.started.elapsed() > LAUNCH_LIMIT {
            return Step::Fail("the stream did not start in time".into());
        }
        Step::Wait
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

    /// The user cancelled the launch: Nightjar closes once it has stopped.
    pub fn cancel(&mut self) {
        if self.phase == Phase::Launched {
            self.cancelled = true;
        }
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

    fn app(id: i32, uuid: &str, name: &str) -> App {
        App {
            name: name.into(),
            id,
            hdr_supported: false,
            uuid: uuid.into(),
            host_index: None,
            art_version: String::new(),
            control: AppControl::None,
            monitor_resume: false,
        }
    }

    fn found(apps: &[App], wanted: &str) -> Result<i32, String> {
        find_app(apps, wanted).map(|a| a.id)
    }

    #[test]
    fn apps_match_by_uuid_then_name() {
        let apps = vec![
            app(1, "", "Desktop"),
            app(2, "5E1F-77", "Steam Big Picture"),
            app(3, "x", "Steam Big Picture"),
        ];
        assert_eq!(found(&apps, "desktop"), Ok(1));
        assert_eq!(found(&apps, "5e1f-77"), Ok(2));
        assert!(found(&apps, "steam big picture")
            .unwrap_err()
            .contains("UUID"));
        assert!(found(&apps, "Cyberpunk").unwrap_err().contains("Cyberpunk"));
    }

    #[test]
    fn host_actions_are_no_apps_to_stream() {
        let terminate = App {
            control: AppControl::Terminate,
            ..app(9, "E16C", "Terminate")
        };
        let apps = vec![app(1, "", "Desktop"), terminate];
        assert!(found(&apps, "terminate")
            .unwrap_err()
            .contains("not an app to stream"));
        assert!(found(&apps, "e16c").is_err());
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
                offline: None,
                running: None,
                confirms_replacement: false,
                apps,
            }),
            message: None,
            busy: false,
            confirming: false,
        }
    }

    /// The PC as `seen` with the apps loaded and `running` running.
    fn running(apps: Vec<App>, running: Option<(i32, Option<&str>)>, confirms: bool) -> Seen {
        let mut seen = seen(true, AppsSeen::Loaded(apps));
        let host = seen.host.as_mut().unwrap();
        host.running = running.map(|(id, uuid)| (id, uuid.map(Into::into)));
        host.confirms_replacement = confirms;
        seen
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
        assert_eq!(c.next(&seen(true, apps())), Step::Launch(7, 5, false));
        let busy = |s: Seen| Seen { busy: true, ..s };
        assert_eq!(c.next(&busy(seen(true, apps()))), Step::Wait);
    }

    #[test]
    fn a_list_that_drops_back_to_unloaded_is_asked_for_again() {
        let mut c = cli(Some("desktop"), false);
        assert_eq!(c.next(&seen(true, AppsSeen::NotLoaded)), Step::LoadApps(7));
        assert_eq!(c.next(&seen(true, AppsSeen::Loading)), Step::Wait);
        assert_eq!(c.next(&seen(true, AppsSeen::NotLoaded)), Step::LoadApps(7));
        assert_eq!(c.next(&seen(true, AppsSeen::NotLoaded)), Step::Wait);
    }

    #[test]
    fn the_running_app_is_resumed_and_another_one_is_an_error() {
        let apps = || vec![app(1, "", "Steam"), app(5, "D-5", "Desktop")];
        let mut c = cli(Some("desktop"), false);
        assert_eq!(
            c.next(&running(apps(), Some((5, None)), false)),
            Step::Launch(7, 5, true)
        );
        // A UUID decides when both sides have one.
        let mut c = cli(Some("desktop"), false);
        assert_eq!(
            c.next(&running(apps(), Some((1, Some("D-5"))), false)),
            Step::Launch(7, 5, true)
        );
        let mut c = cli(Some("desktop"), false);
        assert_eq!(
            c.next(&running(apps(), Some((1, None)), false)),
            Step::Fail("the PC is running Steam; quit it first or start that app instead".into())
        );
        // A PC that asks its user before replacing the running app gets a plain launch.
        let mut c = cli(Some("desktop"), false);
        assert_eq!(
            c.next(&running(apps(), Some((1, None)), true)),
            Step::Launch(7, 5, false)
        );
        // Resume and the other special tiles are never resumed themselves.
        let resume_tile = App {
            control: AppControl::Resume,
            ..app(8, "", "Resume")
        };
        let mut c = cli(Some("resume"), false);
        assert_eq!(
            c.next(&running(vec![resume_tile], Some((1, None)), false)),
            Step::Launch(7, 8, false)
        );
    }

    #[test]
    fn a_launch_the_launcher_drops_ends_the_start() {
        let apps = || AppsSeen::Loaded(vec![app(1, "", "Steam")]);
        let launch =
            |c: &mut CliLaunch| assert_eq!(c.next(&seen(true, apps())), Step::Launch(7, 1, false));
        // Dropped without a message (the app vanished, no identity): an error.
        let mut c = cli(Some("Steam"), false);
        launch(&mut c);
        assert_eq!(
            c.next(&seen(true, apps())),
            Step::Fail("the stream did not start".into())
        );
        // Cancelled by the user: closed without an error.
        let mut c = cli(Some("Steam"), false);
        launch(&mut c);
        c.cancel();
        assert_eq!(c.next(&seen(true, apps())), Step::Close);
        // Still connecting after the backstop.
        let mut c = cli(Some("Steam"), false);
        launch(&mut c);
        c.started = Instant::now() - LAUNCH_LIMIT - Duration::from_secs(1);
        let busy = Seen {
            busy: true,
            ..seen(true, apps())
        };
        assert_eq!(
            c.next(&busy),
            Step::Fail("the stream did not start in time".into())
        );
    }

    #[test]
    fn the_start_waits_for_the_replace_prompt_and_its_cancel_closes() {
        let apps = || vec![app(1, "", "Steam"), app(5, "", "Desktop")];
        let refused = || running(apps(), Some((1, None)), true);
        let confirming = || Seen { confirming: true, ..refused() };
        for confirm in [true, false] {
            let mut c = cli(Some("desktop"), false);
            assert_eq!(c.next(&refused()), Step::Launch(7, 5, false));
            // The PC refused the launch and the launcher asks, however long the user takes.
            c.started = Instant::now() - LAUNCH_LIMIT - Duration::from_secs(1);
            assert_eq!(c.next(&confirming()), Step::Wait);
            if confirm {
                // The launch with the answer gets the full limit.
                assert_eq!(c.next(&Seen { busy: true, ..refused() }), Step::Wait);
                c.streaming_began();
                assert_eq!(c.next(&refused()), Step::Wait);
            } else {
                // Cancelling the prompt cancels the start, without an error.
                c.cancel();
                assert_eq!(c.next(&refused()), Step::Close);
                assert!(c.failed.is_none());
            }
        }
    }

    #[test]
    fn a_message_or_an_unreachable_pc_fails_the_start_at_once() {
        let mut c = cli(Some("Steam"), false);
        let message = Seen {
            message: Some("Nightjar's identity: could not be read".into()),
            ..seen(false, AppsSeen::NotLoaded)
        };
        assert_eq!(
            c.next(&message),
            Step::Fail("Nightjar's identity: could not be read".into())
        );
        let mut c = cli(Some("Steam"), false);
        let mut offline = seen(false, AppsSeen::NotLoaded);
        offline.host.as_mut().unwrap().offline = Some("connection refused".into());
        assert_eq!(
            c.next(&offline),
            Step::Fail("the PC pc cannot be reached: connection refused".into())
        );
    }

    #[test]
    fn only_a_start_without_a_frontend_shows_an_error_box() {
        assert!(shows_error_box(None));
        let launch = Launch {
            host: "pc".into(),
            app: None,
            quit_on_exit: true,
        };
        assert!(!shows_error_box(Some(&launch)));
    }

    #[test]
    fn the_usage_is_boxed_only_without_a_console_and_never_for_a_frontend_start() {
        // A console prints it; a box on top would only be in the way.
        assert!(!shows_usage_box(true, &["--help"]));
        assert!(!shows_usage_box(true, &["--bogus"]));
        // Without one the box is the only place it shows.
        assert!(shows_usage_box(false, &["--help"]));
        assert!(shows_usage_box(false, &["--app", "Desktop"]));
        // A frontend's wrong command line ends at once, in stderr.
        assert!(!shows_usage_box(false, &["--host", "pc", "--bogus"]));
        assert!(!shows_usage_box(false, &["--bogus", "--host=pc"]));
        assert!(shows_usage_box(false, &["--hostname"]));
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
            Step::Launch(7, 1, false)
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
