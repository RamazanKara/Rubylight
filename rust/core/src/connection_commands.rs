//! Commands that run on the PC when one paired device connects and when its
//! stream ends, as in Apollo's Client Commands (ClassicOldSong/Apollo, GPL-3.0,
//! since v0.3.0): the device's `do` and `undo` lists, each entry a command line
//! and an `elevated` flag. An app turns them off with `allow-client-commands`.
//!
//! They differ from an app's preparation commands (`prep-cmd`,
//! `global_prep_cmd`), which run around the app whichever device starts it, and
//! from webhooks, which send events to a URL and run nothing on the PC.
use serde_json::Value;

/// The most commands one list may hold.
pub const MAX_COMMANDS: usize = 64;
/// The longest command line Windows takes.
const MAX_COMMAND: usize = 32767;
/// The device keys that hold the lists.
pub const KEYS: [&str; 2] = ["do", "undo"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectionCommand {
    pub cmd: String,
    pub elevated: bool,
}

/// A device's command list as Apollo and the C++ host store it too: an empty
/// list may be "", and `elevated` may be "true"/"false" or a number. An entry
/// without a command line, more than [`MAX_COMMANDS`] entries, an overlong
/// command or one with a NUL is an error: no command may run if its undo list
/// cannot be read.
pub fn parse(value: Option<&Value>, key: &str) -> Result<Vec<ConnectionCommand>, String> {
    let entries = match value {
        None | Some(Value::Null) => return Ok(vec![]),
        Some(Value::String(text)) if text.trim().is_empty() => return Ok(vec![]),
        Some(Value::Array(entries)) => entries,
        Some(_) => return Err(format!("the device's {key} commands are not a list")),
    };
    if entries.len() > MAX_COMMANDS {
        return Err(format!(
            "the device's {key} commands are more than {MAX_COMMANDS}"
        ));
    }
    entries
        .iter()
        .map(|entry| {
            let Some(cmd) = entry.get("cmd").and_then(Value::as_str) else {
                return Err(format!("a device {key} command has no command line"));
            };
            if cmd.len() > MAX_COMMAND || cmd.contains('\0') {
                return Err(format!("a device {key} command is too long or has a NUL"));
            }
            Ok(ConnectionCommand {
                cmd: cmd.to_owned(),
                elevated: match entry.get("elevated") {
                    Some(Value::Bool(flag)) => *flag,
                    Some(Value::Number(number)) => number.as_i64().is_some_and(|n| n != 0),
                    Some(Value::String(text)) => matches!(
                        text.trim().to_ascii_lowercase().as_str(),
                        "true" | "1" | "yes"
                    ),
                    _ => false,
                },
            })
        })
        .collect()
}

/// Checks the `do` and `undo` lists of a device update before anything is
/// saved, so a list the host could not run is refused instead of stored.
pub fn check_update(update: &Value) -> Result<(), String> {
    for key in KEYS {
        parse(update.get(key), key)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn connection_commands_in_the_formats_apollo_writes_are_accepted() {
        let parse = |value: Value| {
            parse(Some(&value), "do").map(|commands| {
                commands
                    .into_iter()
                    .map(|c| (c.cmd, c.elevated))
                    .collect::<Vec<_>>()
            })
        };
        assert!(parse(json!("")).unwrap().is_empty());
        assert!(super::parse(None, "do").unwrap().is_empty());
        assert!(super::parse(Some(&Value::Null), "do").unwrap().is_empty());
        assert_eq!(
            parse(json!([
                {"cmd": "a", "elevated": "false"},
                {"cmd": "b", "elevated": "true"},
                {"cmd": "c", "elevated": 1},
                {"cmd": "d"},
                {"cmd": "e", "elevated": true}
            ]))
            .unwrap(),
            [
                ("a".to_owned(), false),
                ("b".to_owned(), true),
                ("c".to_owned(), true),
                ("d".to_owned(), false),
                ("e".to_owned(), true)
            ]
        );
        assert!(parse(json!([{"elevated": true}])).is_err());
        assert!(parse(json!({"cmd": "x"})).is_err());
        assert!(parse(json!("not a list")).is_err());
        assert!(parse(json!([{"cmd": "x\u{0}"}])).is_err());
        assert!(parse(Value::Array(vec![json!({"cmd":"x"}); MAX_COMMANDS])).is_ok());
        assert!(parse(Value::Array(vec![json!({"cmd":"x"}); MAX_COMMANDS + 1])).is_err());
    }

    #[test]
    fn connection_commands_a_device_update_with_a_bad_list_is_refused() {
        assert!(check_update(&json!({"uuid": "a", "name": "Phone"})).is_ok());
        assert!(check_update(&json!({"do": [{"cmd": "a"}], "undo": null})).is_ok());
        assert!(check_update(&json!({"do": "", "undo": []})).is_ok());
        let error = check_update(&json!({"do": [], "undo": [{"cmd": 5}]})).unwrap_err();
        assert!(error.contains("undo"), "{error}");
        assert!(check_update(&json!({"do": {"cmd": "a"}})).is_err());
    }
}
