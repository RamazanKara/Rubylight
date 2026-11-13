//! PC commands a device runs from its stream menu: the host-wide `server_cmd`
//! list and, while an app runs, that app's own `menu-cmd` list.
//!
//! Per-app menu commands follow Foundation Sunshine's `menu-cmd` app key
//! (AlkaidLab/foundation-sunshine, GPL-3.0): `{id, name, cmd, elevated}`. Ours
//! adds an `undo` command that runs once when the app closes, for every command
//! that ran during the app's run, in reverse order, as prep-command undos do.
//!
//! A device sees one list and sends a command by its one-byte index (control
//! message `0x3000`), so the order is fixed: the host-wide commands first
//! (unless the app hides them with `exclude-global-menu-cmd`), then the app's.
//! An entry's position never shifts when another entry is malformed, so the
//! index a device read in `/serverinfo` stays the command the host runs.
use serde_json::Value;

/// The most commands an app may carry.
pub const MAX_APP_COMMANDS: usize = 64;
/// The most commands a device can be offered: the index is one byte.
pub const MAX_LISTED: usize = 256;
/// The longest command line Windows takes.
const MAX_COMMAND: usize = 32767;
/// The longest name a menu shows.
const MAX_NAME: usize = 256;

/// The app key with the app's own menu commands.
pub const APP_KEY: &str = "menu-cmd";
/// The app key that hides the host-wide PC commands while the app runs.
pub const EXCLUDE_GLOBAL_KEY: &str = "exclude-global-menu-cmd";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MenuCommand {
    pub name: String,
    pub cmd: String,
    /// Runs when the app closes if `cmd` ran during its run. Always empty for
    /// host-wide commands, which belong to no app run.
    pub undo: String,
    pub elevated: bool,
}

/// Where a listed command comes from, with its index in that list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Global(usize),
    App(usize),
}

/// `elevated` as Apollo, Foundation and the C++ host store it: a boolean, a
/// number or `"true"`/`"1"`/`"yes"`.
fn flag(value: Option<&Value>) -> bool {
    match value {
        Some(Value::Bool(flag)) => *flag,
        Some(Value::Number(number)) => number.as_i64().is_some_and(|n| n != 0),
        Some(Value::String(text)) => matches!(
            text.trim().to_ascii_lowercase().as_str(),
            "true" | "1" | "yes"
        ),
        _ => false,
    }
}

fn text(entry: &Value, key: &str) -> String {
    entry
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

/// Every entry of a command list, in place. A malformed entry keeps its
/// position with an empty command, which never runs. `with_undo` is false for
/// the host-wide list.
pub fn entries(value: &Value, with_undo: bool) -> Vec<MenuCommand> {
    value
        .as_array()
        .map(|list| {
            list.iter()
                .map(|entry| MenuCommand {
                    name: text(entry, "name"),
                    cmd: text(entry, "cmd"),
                    undo: if with_undo {
                        text(entry, "undo")
                    } else {
                        String::new()
                    },
                    elevated: flag(entry.get("elevated")),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The host-wide `server_cmd` setting's commands.
pub fn global(server_cmd: &str) -> Vec<MenuCommand> {
    serde_json::from_str::<Value>(server_cmd)
        .map(|value| entries(&value, false))
        .unwrap_or_default()
}

/// The commands a device sees, in index order: the host-wide ones unless the
/// app hides them, then the app's, at most [`MAX_LISTED`].
pub fn listed<'a>(
    global: &'a [MenuCommand],
    app: &'a [MenuCommand],
    exclude_global: bool,
) -> Vec<(Source, &'a MenuCommand)> {
    let global = global
        .iter()
        .enumerate()
        .filter(|_| !exclude_global)
        .map(|(index, command)| (Source::Global(index), command));
    let app = app
        .iter()
        .enumerate()
        .map(|(index, command)| (Source::App(index), command));
    global.chain(app).take(MAX_LISTED).collect()
}

/// Checks an app's `menu-cmd` before it is saved: a list of at most
/// [`MAX_APP_COMMANDS`] objects, each with a name and a command line, an
/// optional undo and an optional `elevated`. `null` or a missing key is fine.
pub fn check_app(value: Option<&Value>) -> Result<(), String> {
    let list = match value {
        None | Some(Value::Null) => return Ok(()),
        Some(Value::Array(list)) => list,
        Some(_) => return Err(format!("{APP_KEY} must be a list")),
    };
    if list.len() > MAX_APP_COMMANDS {
        return Err(format!(
            "{APP_KEY} has more than {MAX_APP_COMMANDS} commands"
        ));
    }
    for (at, entry) in list.iter().enumerate() {
        let position = at + 1;
        let Some(object) = entry.as_object() else {
            return Err(format!("{APP_KEY} command {position} is not an object"));
        };
        let field = |key: &str, required: bool, limit: usize| -> Result<(), String> {
            match object.get(key) {
                None | Some(Value::Null) if !required => Ok(()),
                Some(Value::String(text)) => {
                    if required && text.trim().is_empty() {
                        Err(format!("{APP_KEY} command {position} has no {key}"))
                    } else if text.len() > limit || text.contains('\0') {
                        Err(format!(
                            "{APP_KEY} command {position}: {key} is too long or has a NUL"
                        ))
                    } else {
                        Ok(())
                    }
                }
                _ => Err(format!("{APP_KEY} command {position} has no {key}")),
            }
        };
        field("name", true, MAX_NAME)?;
        field("cmd", true, MAX_COMMAND)?;
        field("undo", false, MAX_COMMAND)?;
        match object.get("elevated") {
            None | Some(Value::Null | Value::Bool(_)) => {}
            Some(_) => {
                return Err(format!(
                    "{APP_KEY} command {position}: elevated must be true or false"
                ));
            }
        }
    }
    Ok(())
}

/// The app commands that ran during one app run, for their undo when it
/// closes. Each command is undone once, last run first.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UndoLog {
    ran: Vec<usize>,
}

impl UndoLog {
    /// App command `index` ran. A command without an undo is not recorded,
    /// and one that ran before moves to the end, so it is undone first.
    pub fn record(&mut self, index: usize, command: &MenuCommand) {
        if command.undo.trim().is_empty() {
            return;
        }
        self.ran.retain(|&ran| ran != index);
        self.ran.push(index);
    }
    /// The indices to undo, last run first; the log is empty afterwards.
    pub fn take(&mut self) -> Vec<usize> {
        let mut ran = std::mem::take(&mut self.ran);
        ran.reverse();
        ran
    }
    pub fn is_empty(&self) -> bool {
        self.ran.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn command(name: &str, cmd: &str, undo: &str) -> MenuCommand {
        MenuCommand {
            name: name.into(),
            cmd: cmd.into(),
            undo: undo.into(),
            elevated: false,
        }
    }

    #[test]
    fn menu_commands_keep_positions_and_read_foundation_and_apollo_shapes() {
        let list = entries(
            &json!([
                {"id":"a1","name":"Pet","cmd":"pet.exe","undo":"taskkill /im pet.exe","elevated":"true"},
                "not an entry",
                {"name":"Keyboard","cmd":"osk","elevated":1},
                {"name":"No command"}
            ]),
            true,
        );
        assert_eq!(list.len(), 4);
        assert_eq!(list[0].name, "Pet");
        assert_eq!(list[0].undo, "taskkill /im pet.exe");
        assert!(list[0].elevated && list[2].elevated);
        assert_eq!(list[1], MenuCommand::default());
        assert!(list[3].cmd.is_empty());
        // The host-wide list never carries an undo.
        let global = global(r#"[{"name":"Lock","cmd":"lock","undo":"unlock"}]"#);
        assert_eq!(global, vec![command("Lock", "lock", "")]);
        assert!(super::global("not json").is_empty());
        assert!(entries(&json!({"name":"x"}), true).is_empty());
    }

    #[test]
    fn menu_commands_list_global_first_unless_the_app_hides_them() {
        let global = vec![command("Lock", "lock", ""), command("Sleep", "sleep", "")];
        let app = vec![command("Pet", "pet", "kill")];
        let names = |exclude| {
            listed(&global, &app, exclude)
                .into_iter()
                .map(|(source, c)| (source, c.name.clone()))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            names(false),
            vec![
                (Source::Global(0), "Lock".to_owned()),
                (Source::Global(1), "Sleep".to_owned()),
                (Source::App(0), "Pet".to_owned()),
            ]
        );
        assert_eq!(names(true), vec![(Source::App(0), "Pet".to_owned())]);
        assert_eq!(listed(&global, &[], false).len(), 2);
        let many = vec![command("x", "x", ""); 300];
        let listed = listed(&many, &app, false);
        assert_eq!(listed.len(), MAX_LISTED);
        assert!(
            listed
                .iter()
                .all(|(source, _)| matches!(source, Source::Global(_)))
        );
    }

    #[test]
    fn menu_commands_app_lists_are_checked_before_saving() {
        assert!(check_app(None).is_ok());
        assert!(check_app(Some(&Value::Null)).is_ok());
        assert!(check_app(Some(&json!([]))).is_ok());
        assert!(
            check_app(Some(&json!([
                {"name":"Pet","cmd":"pet.exe","undo":"taskkill /im pet.exe","elevated":true},
                {"id":"x","name":"Keyboard","cmd":"osk","undo":null}
            ])))
            .is_ok()
        );
        for (bad, says) in [
            (json!("pet.exe"), "must be a list"),
            (json!(["pet.exe"]), "not an object"),
            (json!([{"cmd":"pet.exe"}]), "no name"),
            (json!([{"name":" ","cmd":"pet.exe"}]), "no name"),
            (json!([{"name":"Pet"}]), "no cmd"),
            (json!([{"name":"Pet","cmd":""}]), "no cmd"),
            (json!([{"name":"Pet","cmd":"x","undo":5}]), "no undo"),
            (json!([{"name":"Pet","cmd":"x\u{0}"}]), "NUL"),
            (
                json!([{"name":"Pet","cmd":"x","elevated":"yes"}]),
                "elevated",
            ),
        ] {
            let error = check_app(Some(&bad)).unwrap_err();
            assert!(error.contains(says), "{bad}: {error}");
        }
        let too_many = Value::Array(vec![json!({"name":"x","cmd":"x"}); MAX_APP_COMMANDS + 1]);
        assert!(
            check_app(Some(&too_many))
                .unwrap_err()
                .contains("more than")
        );
    }

    #[test]
    fn menu_commands_undo_each_once_last_run_first() {
        let with_undo = command("Pet", "pet", "kill pet");
        let without = command("Note", "note", "");
        let mut log = UndoLog::default();
        log.record(0, &with_undo);
        log.record(1, &without);
        log.record(2, &with_undo);
        log.record(0, &with_undo);
        assert!(!log.is_empty());
        assert_eq!(log.take(), vec![0, 2]);
        assert!(log.is_empty());
        assert!(log.take().is_empty());
    }
}
