//! Which processes of a running game Quick Suspend pauses, and in which order.
//!
//! The candidates are the app's verified game processes (pid and creation
//! time, as the host tracks them) and every process they started; nothing
//! outside that tree. A tree with an anti-cheat in it, or an anti-cheat that
//! started with the game anywhere on the PC, is refused whole, as is one with a
//! system process or the host. Launchers and console hosts in the tree are
//! left running. The rest is suspended children first, so that a parent
//! watching its child never sees it hang while the parent still runs.
use crate::steam::Process;
use rubylight_protocol::quick_suspend::Reason;
use std::collections::BTreeMap;

/// One process to suspend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub pid: u32,
    pub started: u64,
    pub name: String,
}

/// Why nothing is suspended, and the program that decided it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refused {
    pub reason: Reason,
    /// The program that caused it, or the anti-cheat's name.
    pub program: Option<String>,
}

impl Refused {
    fn new(reason: Reason, program: Option<&str>) -> Self {
        Self {
            reason,
            program: program.map(str::to_owned),
        }
    }
}

/// What a suspend would pause, and what it leaves running.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// Children before their parents.
    pub targets: Vec<Target>,
    /// Launchers and helpers in the tree that stay running.
    pub left_running: Vec<String>,
}

/// Processes Windows needs to keep running, which no game tree may hold: a
/// tree with one of them is refused.
const SYSTEM: &[&str] = &[
    "csrss.exe",
    "smss.exe",
    "wininit.exe",
    "winlogon.exe",
    "services.exe",
    "lsass.exe",
    "svchost.exe",
    "dwm.exe",
    "explorer.exe",
    "fontdrvhost.exe",
    "sihost.exe",
    "taskhostw.exe",
    "runtimebroker.exe",
    "audiodg.exe",
    "butterpollo.exe",
    "butterpollo-service.exe",
];

/// Helpers a game's tree may hold that stay running: suspending a console
/// host would freeze its window, a crash reporter or store helper belongs to
/// no game.
const HELPERS: &[&str] = &[
    "conhost.exe",
    "openconsole.exe",
    "werfault.exe",
    "steamwebhelper.exe",
    "steamservice.exe",
    "steamerrorreporter.exe",
    "steamerrorreporter64.exe",
    "gameoverlayui.exe",
    "gameoverlayui64.exe",
    "epicwebhelper.exe",
    "socialclubhelper.exe",
    // A launcher's own browser and helper processes: its window hangs without them.
    "unrealcefsubprocess.exe",
    "qtwebengineprocess.exe",
    "galaxyclient helper.exe",
    "battle.net helper.exe",
    "uplaywebcore.exe",
];

/// The anti-cheat `name` belongs to, if any.
pub fn anti_cheat(name: &str) -> Option<&'static str> {
    crate::neural::anti_cheat(name).or_else(|| {
        let lower = name.to_ascii_lowercase();
        let stem = lower.strip_suffix(".exe").unwrap_or(&lower);
        match stem {
            "vgtray" | "vgc" => Some("Riot Vanguard"),
            "bedaisy" | "bedaisy.sys" => Some("BattlEye"),
            _ if stem.starts_with("gamemon") => Some("nProtect GameGuard"),
            "atvi-brynhildr" => Some("Ricochet"),
            "hoyokprotect" => Some("HoYoKProtect"),
            _ if stem.starts_with("eaanticheat") => Some("EA Javelin"),
            _ => None,
        }
    })
}

fn listed(list: &[&str], name: &str) -> bool {
    list.iter().any(|entry| entry.eq_ignore_ascii_case(name))
}

/// A launcher or store client, never a game.
fn launcher(name: &str) -> bool {
    crate::neural::classify_non_game_exe(name) == Some("a launcher")
}

/// The processes to suspend for the game whose verified processes are `owned`
/// (pid to creation time), among every running process (`all`, any session),
/// for the host `host_pid`.
pub fn plan(owned: &BTreeMap<u32, u64>, all: &[Process], host_pid: u32) -> Result<Plan, Refused> {
    // The tree, each process with its depth below its root.
    let mut tree: Vec<(usize, &Process)> = Vec::new();
    for (&pid, &started) in owned {
        let Some(root) = all.iter().find(|p| p.pid == pid && p.started == started) else {
            continue;
        };
        let mut at = tree.len();
        if !tree.iter().any(|(_, p)| p.pid == root.pid) {
            tree.push((0, root));
        } else {
            continue;
        }
        while let Some(&(depth, parent)) = tree.get(at) {
            at += 1;
            for child in all.iter().filter(|p| {
                p.parent == parent.pid
                    && p.pid != parent.pid
                    && p.started != 0
                    && p.started >= parent.started
            }) {
                if !tree.iter().any(|(_, p)| p.pid == child.pid) {
                    tree.push((depth + 1, child));
                }
            }
        }
    }
    if tree.is_empty() {
        return Err(Refused::new(Reason::NoOwnedProcess, None));
    }
    for (_, process) in &tree {
        if let Some(name) = anti_cheat(&process.name) {
            return Err(Refused::new(Reason::AntiCheatDetected, Some(name)));
        }
        if process.pid <= 4 || process.pid == host_pid || listed(SYSTEM, &process.name) {
            return Err(Refused::new(Reason::DenylistedProcess, Some(&process.name)));
        }
    }
    // An anti-cheat anywhere on the PC, whenever it started: a service runs in
    // another session and descends from no game, and a launcher or the PC's
    // start may have started it long before the game it watches.
    if let Some(name) = all.iter().find_map(|p| anti_cheat(&p.name)) {
        return Err(Refused::new(Reason::AntiCheatDetected, Some(name)));
    }
    let (kept, mut targets): (Vec<_>, Vec<_>) = tree
        .into_iter()
        .partition(|(_, p)| launcher(&p.name) || listed(HELPERS, &p.name));
    if targets.is_empty() {
        return Err(Refused::new(
            Reason::NoOwnedProcess,
            kept.first().map(|(_, p)| p.name.as_str()),
        ));
    }
    // Deepest first; a stable sort keeps siblings in the order found.
    targets.sort_by_key(|target| std::cmp::Reverse(target.0));
    Ok(Plan {
        targets: targets
            .into_iter()
            .map(|(_, p)| Target {
                pid: p.pid,
                started: p.started,
                name: p.name.clone(),
            })
            .collect(),
        left_running: kept.into_iter().map(|(_, p)| p.name.clone()).collect(),
    })
}

/// The plain-English refusal a client can show as it is.
pub fn message(refused: &Refused) -> String {
    let program = refused.program.as_deref().unwrap_or("a process");
    match refused.reason {
        Reason::None => String::new(),
        Reason::DisabledForApp => "Quick Suspend is off on this PC. Turn it on in the host's settings, under App behavior, to pause games.".into(),
        Reason::NoOwnedProcess => match &refused.program {
            Some(launcher) => format!("Only {launcher} runs for this app, and Quick Suspend never pauses a launcher."),
            None => "Quick Suspend works only for games Rubylight launched, not Desktop sessions.".into(),
        },
        Reason::AntiCheatDetected => format!("Anti-cheat protection ({program}) runs on this PC, so no game is paused while it does."),
        Reason::ProtectedProcess => format!("Windows blocked pausing {program}, so the game was left running."),
        Reason::DenylistedProcess => format!("The game's processes include {program}, which Quick Suspend never pauses."),
        Reason::GuestsActive => "The game cannot be suspended while guests are watching or playing.".into(),
        Reason::CoopSession => "Quick Suspend is unavailable during co-op sessions.".into(),
        Reason::NotOwner => "Only the device that started the game, or one with host control, can suspend or resume it.".into(),
        Reason::SessionBusy => "A launch or a session end is under way; try again in a moment.".into(),
        Reason::NoSession => "No game is running.".into(),
        Reason::IoOrOsError => format!("Windows could not pause or resume {program}. Rubylight resumes whatever it paused as soon as it can."),
    }
}

#[cfg(test)]
mod tests;
