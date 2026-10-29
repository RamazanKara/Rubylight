//! Whether a session's Nucleus Co-op runs elevated, from the host's copy, or
//! from the signed-in user's own copy with their standard rights.
//!
//! Only Rubylight's own one-player handler ever runs without administrator
//! rights, and only for a game that asks Windows for none: a hub handler
//! always runs elevated, whatever it says, and so does a game whose .exe
//! needs administrator rights. On an account Windows gives no elevated
//! token ([`Account`]: a standard account, or Windows' Administrator
//! protection) those are refused, saying what to do or what still runs,
//! never started another way.

/// What a game's .exe asks of Windows to start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Need {
    /// Its user's rights as they are (`asInvoker`, or no level at all).
    Nothing,
    /// The most its user has (`highestAvailable`, `RUNASHIGHEST`):
    /// elevated for an administrator, standard rights otherwise.
    Highest,
    /// Administrator rights, for `Why`.
    Administrator(Why),
}

/// Why a game needs administrator rights.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    /// Its manifest asks for them (`requireAdministrator`).
    Manifest,
    /// Its compatibility settings run it as administrator (`RUNASADMIN`).
    Compatibility,
    /// Windows takes a 32-bit .exe named like an installer that asks for no
    /// level for one, and elevates it.
    Installer,
    /// Nucleus Co-op clears the attributes of every file in the game's
    /// folder before it starts it, and the user may not change one of them
    /// (a game installed under Program Files).
    Folder,
}
impl std::fmt::Display for Why {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Manifest => "its manifest asks for them",
            Self::Compatibility => "its compatibility settings run it as administrator",
            Self::Installer => "Windows takes it for an installer by its name",
            Self::Folder => {
                "Nucleus Co-op changes the files in its folder, which only administrators may change"
            }
        })
    }
}

/// How a session's Nucleus Co-op runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Run {
    /// The host's copy, as the signed-in user elevated.
    Elevated,
    /// The signed-in user's own copy, with their standard rights.
    User,
}

/// The handler a session starts with, as far as [`choose`] cares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Script {
    /// A hub or user handler: always elevated.
    Hub,
    /// Rubylight's own one-player handler, and whether it needs Nucleus
    /// Co-op elevated ([`super::handler::needs_admin`]).
    Own { needs_admin: bool },
}

/// What Windows gives the account signed in on the PC to start a program
/// elevated with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Account {
    /// An elevated token of its own: an administrator Windows elevates.
    Elevates,
    /// No administrator rights: a standard account.
    Standard,
    /// An administrator whom Windows' Administrator protection elevates only
    /// into a separate account after a Windows Hello prompt, which Rubylight
    /// never starts Nucleus Co-op as.
    Protected,
}

impl Account {
    /// Whether it has an elevated token Nucleus Co-op can start with.
    pub fn elevates(self) -> bool {
        self == Self::Elevates
    }

    /// Why it has none, and what to do or what still runs: the end of every
    /// refusal of `refused` for want of administrator rights ("{why}, and
    /// {this}"), in the console too. Empty for [`Account::Elevates`].
    pub fn no_elevation(self, refused: Refused) -> String {
        match (self, refused) {
            (Self::Elevates, _) => String::new(),
            (Self::Standard, _) => STANDARD.to_string(),
            (Self::Protected, Refused::Game) => {
                format!("{PROTECTED}, so Nucleus Co-op can't run this game on this PC")
            }
            (Self::Protected, Refused::Hub) => format!(
                "{PROTECTED}, so Nucleus Co-op can't run this game on this PC; {STILL_RUNS}"
            ),
            (Self::Protected, Refused::Open) => {
                format!("{PROTECTED}, so Nucleus Co-op can't open on this PC; {STILL_RUNS}")
            }
        }
    }
}

/// What a refusal for want of administrator rights keeps from starting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    /// The game: it, or Rubylight's own handler for it, needs them.
    Game,
    /// The game's hub handler: a game without one may still run.
    Hub,
    /// Open Nucleus Co-op on this PC.
    Open,
}

/// Why a standard account is refused, and what to do.
pub const STANDARD: &str =
    "Windows doesn't give this account any: sign in to Windows on this PC as an administrator";
/// Why an administrator under Windows' Administrator protection is refused.
/// Rubylight never asks anyone to turn a Windows security feature off.
pub const PROTECTED: &str = "Windows' Administrator protection keeps them from Rubylight";
/// What Nucleus Co-op still runs under Windows' Administrator protection:
/// Rubylight's own handler, as the user ([`Run::User`]).
pub const STILL_RUNS: &str = "it still runs a game with no hub handler that needs no administrator rights, from this account's own copy";

/// How a session of `script` for a game that needs `game` runs for the
/// signed-in `account`. Rubylight's own handler for a game that needs
/// nothing runs as the user even when they could elevate: least privilege.
/// An `Err` is the refusal's text.
pub fn choose(script: Script, game: Need, account: Account) -> Result<Run, String> {
    let elevates = account.elevates();
    let elevated = |why: String, refused: Refused| {
        if elevates {
            Ok(Run::Elevated)
        } else {
            Err(format!("{why}, and {}", account.no_elevation(refused)))
        }
    };
    match (script, game) {
        (Script::Hub, _) => elevated(
            "Nucleus Co-op needs administrator rights for a hub handler".into(),
            Refused::Hub,
        ),
        (Script::Own { needs_admin: true }, _) => elevated(
            "Rubylight's Nucleus Co-op handler for this game needs administrator rights".into(),
            Refused::Game,
        ),
        (Script::Own { .. }, Need::Administrator(why)) => elevated(
            format!("The game needs administrator rights ({why}), so Nucleus Co-op runs elevated"),
            Refused::Game,
        ),
        // As Windows starts it: elevated where the user can be.
        (Script::Own { .. }, Need::Highest) if elevates => Ok(Run::Elevated),
        (Script::Own { .. }, Need::Highest | Need::Nothing) => Ok(Run::User),
    }
}

/// The level a manifest's `requestedExecutionLevel` asks for: None when it
/// names none, so Windows' installer detection may still apply. Comments
/// are skipped; a namespace prefix is allowed.
pub fn manifest_need(manifest: &str) -> Option<Need> {
    let text = without_comments(manifest);
    let mut rest = text.as_str();
    while let Some(start) = rest.find('<') {
        rest = &rest[start + 1..];
        let end = rest.find('>').unwrap_or(rest.len());
        let tag = &rest[..end];
        rest = &rest[end..];
        let name_end = tag
            .find(|c: char| c.is_whitespace() || c == '/')
            .unwrap_or(tag.len());
        let name = &tag[..name_end];
        let local = name.rsplit(':').next().unwrap_or(name);
        if local != "requestedExecutionLevel" {
            continue;
        }
        let level = attribute(&tag[name_end..], "level")?;
        return Some(match level.trim() {
            "requireAdministrator" => Need::Administrator(Why::Manifest),
            "highestAvailable" => Need::Highest,
            "asInvoker" => Need::Nothing,
            // Windows refuses to start an .exe with another level.
            _ => Need::Administrator(Why::Manifest),
        });
    }
    None
}

fn without_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("<!--") {
        out.push_str(&rest[..start]);
        rest = match rest[start + 4..].find("-->") {
            Some(end) => &rest[start + 4 + end + 3..],
            None => "",
        };
    }
    out.push_str(rest);
    out
}

/// The value of `name="..."` (or `'...'`) among a tag's attributes.
fn attribute<'a>(attributes: &'a str, name: &str) -> Option<&'a str> {
    let mut rest = attributes;
    loop {
        rest = rest.trim_start_matches(|c: char| c.is_whitespace() || c == '/');
        let equals = rest.find('=')?;
        let key = rest[..equals].trim();
        let value = rest[equals + 1..].trim_start();
        let quote = value.chars().next().filter(|c| *c == '"' || *c == '\'')?;
        let close = value[1..].find(quote)?;
        if key == name {
            return Some(&value[1..1 + close]);
        }
        rest = &value[1 + close + 1..];
    }
}

/// What an `AppCompatFlags\Layers` value (`~ RUNASADMIN HIGHDPIAWARE`)
/// asks for. `RUNASINVOKER` doesn't lower what the manifest asks: a game
/// that needs administrator rights runs elevated.
pub fn layers_need(value: &str) -> Need {
    let mut need = Need::Nothing;
    for token in value.split_whitespace() {
        if token.eq_ignore_ascii_case("RUNASADMIN") {
            return Need::Administrator(Why::Compatibility);
        }
        if token.eq_ignore_ascii_case("RUNASHIGHEST") {
            need = Need::Highest;
        }
    }
    need
}

/// Whether Windows' installer detection takes an .exe named `file_name` for
/// an installer (its name holds install, setup, update or patch). It only
/// applies to a 32-bit .exe whose manifest names no level.
pub fn installer_name(file_name: &str) -> bool {
    let name = file_name.to_ascii_lowercase();
    ["install", "setup", "update", "patch"]
        .iter()
        .any(|word| name.contains(word))
}

/// The most of `needs`: administrator over highest over nothing, the first
/// administrator reason kept.
pub fn most(needs: impl IntoIterator<Item = Need>) -> Need {
    needs
        .into_iter()
        .fold(Need::Nothing, |most, need| match (most, need) {
            (Need::Administrator(_), _) => most,
            (_, Need::Administrator(_)) => need,
            (Need::Highest, _) | (_, Need::Highest) => Need::Highest,
            _ => Need::Nothing,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    const OWN: Script = Script::Own { needs_admin: false };
    const OWN_ADMIN: Script = Script::Own { needs_admin: true };
    const ADMIN: Need = Need::Administrator(Why::Manifest);

    const ELEVATES: Account = Account::Elevates;
    const STANDARD_ACCOUNT: Account = Account::Standard;

    #[test]
    fn a_hub_handler_always_runs_elevated() {
        for game in [Need::Nothing, Need::Highest, ADMIN] {
            assert_eq!(choose(Script::Hub, game, ELEVATES), Ok(Run::Elevated));
            let refused = choose(Script::Hub, game, STANDARD_ACCOUNT).unwrap_err();
            assert_eq!(
                refused,
                "Nucleus Co-op needs administrator rights for a hub handler, and Windows doesn't give this account any: sign in to Windows on this PC as an administrator"
            );
        }
    }

    #[test]
    fn administrator_protection_is_told_what_can_and_cannot_run_never_to_turn_it_off() {
        // A hub handler's game: what still runs is named.
        assert_eq!(
            choose(Script::Hub, Need::Nothing, Account::Protected).unwrap_err(),
            "Nucleus Co-op needs administrator rights for a hub handler, and Windows' Administrator protection keeps them from Rubylight, so Nucleus Co-op can't run this game on this PC; it still runs a game with no hub handler that needs no administrator rights, from this account's own copy"
        );
        // A game that needs them itself: nothing else runs it.
        for (script, game) in [(OWN_ADMIN, Need::Nothing), (OWN, ADMIN)] {
            let refused = choose(script, game, Account::Protected).unwrap_err();
            assert!(
                refused.ends_with(", and Windows' Administrator protection keeps them from Rubylight, so Nucleus Co-op can't run this game on this PC"),
                "{refused}"
            );
        }
        for refused in [Refused::Game, Refused::Hub, Refused::Open] {
            let words = Account::Protected.no_elevation(refused);
            // Never to sign in as an administrator, which doesn't help, nor
            // to turn a Windows security feature off.
            for advice in ["sign in", "turn", "off", "restart", "disable"] {
                assert!(!words.contains(advice), "{words}");
            }
            assert_eq!(Account::Elevates.no_elevation(refused), "");
            assert_eq!(Account::Standard.no_elevation(refused), STANDARD);
        }
        assert_eq!(
            Account::Protected.no_elevation(Refused::Open),
            "Windows' Administrator protection keeps them from Rubylight, so Nucleus Co-op can't open on this PC; it still runs a game with no hub handler that needs no administrator rights, from this account's own copy"
        );
        // A game that needs nothing still runs as the user.
        assert_eq!(
            choose(OWN, Need::Nothing, Account::Protected),
            Ok(Run::User)
        );
        assert_eq!(
            choose(OWN, Need::Highest, Account::Protected),
            Ok(Run::User)
        );
        assert!(!Account::Protected.elevates() && !Account::Standard.elevates());
    }

    #[test]
    fn our_handler_runs_as_the_user_for_a_game_that_needs_nothing() {
        // Least privilege: also on an account that could elevate.
        assert_eq!(choose(OWN, Need::Nothing, ELEVATES), Ok(Run::User));
        assert_eq!(choose(OWN, Need::Nothing, STANDARD_ACCOUNT), Ok(Run::User));
    }

    #[test]
    fn our_handler_that_needs_admin_runs_elevated_or_is_refused() {
        assert_eq!(
            choose(OWN_ADMIN, Need::Nothing, ELEVATES),
            Ok(Run::Elevated)
        );
        let refused = choose(OWN_ADMIN, Need::Nothing, STANDARD_ACCOUNT).unwrap_err();
        assert!(
            refused.contains("handler for this game needs administrator rights"),
            "{refused}"
        );
        assert!(refused.ends_with(STANDARD), "{refused}");
    }

    #[test]
    fn a_game_that_needs_admin_runs_elevated_or_is_refused() {
        for why in [
            Why::Manifest,
            Why::Compatibility,
            Why::Installer,
            Why::Folder,
        ] {
            let game = Need::Administrator(why);
            assert_eq!(choose(OWN, game, ELEVATES), Ok(Run::Elevated));
            let refused = choose(OWN, game, STANDARD_ACCOUNT).unwrap_err();
            assert!(
                refused.starts_with(&format!("The game needs administrator rights ({why})")),
                "{refused}"
            );
            assert!(refused.contains(STANDARD), "{refused}");
        }
    }

    #[test]
    fn a_game_that_asks_for_the_most_gets_what_the_account_has() {
        assert_eq!(choose(OWN, Need::Highest, ELEVATES), Ok(Run::Elevated));
        assert_eq!(choose(OWN, Need::Highest, STANDARD_ACCOUNT), Ok(Run::User));
    }

    #[test]
    fn the_manifest_level_is_read() {
        let manifest = |level: &str| {
            format!(
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel {level} uiAccess="false"/>
      </requestedPrivileges>
    </security>
  </trustInfo>
</assembly>"#
            )
        };
        assert_eq!(
            manifest_need(&manifest(r#"level="asInvoker""#)),
            Some(Need::Nothing)
        );
        assert_eq!(
            manifest_need(&manifest(r#"level='highestAvailable'"#)),
            Some(Need::Highest)
        );
        assert_eq!(
            manifest_need(&manifest(r#"level = "requireAdministrator""#)),
            Some(ADMIN)
        );
        // The order of attributes doesn't matter, nor a prefix.
        assert_eq!(
            manifest_need(
                r#"<ms_asmv3:requestedExecutionLevel uiAccess="false" level="requireAdministrator"></ms_asmv3:requestedExecutionLevel>"#
            ),
            Some(ADMIN)
        );
        // Windows refuses an unknown level rather than running it lower.
        assert_eq!(manifest_need(&manifest(r#"level="root""#)), Some(ADMIN));
        assert_eq!(manifest_need(&manifest("")), None);
        assert_eq!(manifest_need("<assembly><dependency/></assembly>"), None);
        assert_eq!(manifest_need(""), None);
    }

    #[test]
    fn a_level_in_a_comment_is_no_level() {
        let manifest = r#"<assembly>
  <!-- <requestedExecutionLevel level="requireAdministrator"/> -->
  <requestedExecutionLevel level="asInvoker"/>
</assembly>"#;
        assert_eq!(manifest_need(manifest), Some(Need::Nothing));
        assert_eq!(
            manifest_need(r#"<!-- <requestedExecutionLevel level="requireAdministrator"/> -->"#),
            None
        );
        assert_eq!(
            manifest_need(
                "<!-- unclosed <requestedExecutionLevel level=\"requireAdministrator\"/>"
            ),
            None
        );
        // A similar name is another element.
        assert_eq!(
            manifest_need(r#"<requestedExecutionLevels level="requireAdministrator"/>"#),
            None
        );
    }

    #[test]
    fn compatibility_flags_that_elevate_are_read() {
        assert_eq!(
            layers_need("~ RUNASADMIN"),
            Need::Administrator(Why::Compatibility)
        );
        assert_eq!(
            layers_need("~ HIGHDPIAWARE runasadmin WIN7RTM"),
            Need::Administrator(Why::Compatibility)
        );
        assert_eq!(layers_need("~ RUNASHIGHEST"), Need::Highest);
        assert_eq!(layers_need("~ HIGHDPIAWARE"), Need::Nothing);
        assert_eq!(layers_need("~ RUNASINVOKER"), Need::Nothing);
        assert_eq!(layers_need(""), Need::Nothing);
        // A token, not a part of one.
        assert_eq!(layers_need("~ NORUNASADMINX"), Need::Nothing);
    }

    #[test]
    fn installer_names_are_the_ones_windows_detects() {
        for name in [
            "Setup.exe",
            "game_installer.exe",
            "UPDATE.EXE",
            "patch1.exe",
            "GameUpdater.exe",
        ] {
            assert!(installer_name(name), "{name}");
        }
        for name in ["lotrbfme.exe", "Puzzle.exe", "game.exe"] {
            assert!(!installer_name(name), "{name}");
        }
    }

    #[test]
    fn the_most_need_wins() {
        assert_eq!(most([]), Need::Nothing);
        assert_eq!(most([Need::Nothing, Need::Highest]), Need::Highest);
        assert_eq!(
            most([
                Need::Highest,
                Need::Administrator(Why::Compatibility),
                ADMIN
            ]),
            Need::Administrator(Why::Compatibility)
        );
        assert_eq!(most([ADMIN, Need::Nothing]), ADMIN);
    }
}
