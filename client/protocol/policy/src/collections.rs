//! Home's groups by collection, the same on desktop and Android (`com.limelight.console.CollectionGroups`
//! through a JSON call). A PC sends each game's collections by name (Steam collections, Playnite
//! categories); this says which group shows which game.

use serde::{Deserialize, Serialize};

/// One group of Home, with the games in it as places in the list asked about, in that order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Group {
    /// The collection's name as the first game in it writes it; `None` for the games in none.
    pub name: Option<String>,
    pub games: Vec<usize>,
}

/// A group per collection by name, whatever its capitals and surrounding spaces, a game in several
/// in each; then the games in none. Nothing at all when no game is in a collection: Home has no
/// such groups to offer then.
pub fn groups<S: AsRef<str>>(games: &[Vec<S>]) -> Vec<Group> {
    let mut named: std::collections::BTreeMap<String, Group> = Default::default();
    let mut none = Vec::new();
    for (place, names) in games.iter().enumerate() {
        let mut any = false;
        for name in names.iter().map(|name| name.as_ref().trim()).filter(|name| !name.is_empty()) {
            any = true;
            let group = named
                .entry(name.to_lowercase())
                .or_insert_with(|| Group { name: Some(name.to_owned()), games: Vec::new() });
            // The same collection twice on one game, in other capitals, shows it once.
            if group.games.last() != Some(&place) {
                group.games.push(place);
            }
        }
        if !any {
            none.push(place);
        }
    }
    let mut groups: Vec<Group> = named.into_values().collect();
    if !groups.is_empty() && !none.is_empty() {
        groups.push(Group { name: None, games: none });
    }
    groups
}

/// The one JSON call Android's app reaches this through.
pub mod json {
    use super::*;
    use serde_json::{json, Value};

    #[derive(Deserialize)]
    struct Request {
        /// Each game's collections, in the order of the list.
        games: Vec<Vec<String>>,
    }

    /// `{"games": [[name, ...], ...]}` answers `{"ok": {"groups": [{"name": name | null, "games":
    /// [place, ...]}, ...]}}`, or `{"error": "invalid", "detail": why}`.
    pub fn call(request: &str) -> String {
        let answer: Value = match serde_json::from_str::<Request>(request) {
            Ok(request) => json!({"ok": {"groups": groups(&request.games)}}),
            Err(error) => json!({"error": "invalid", "detail": error.to_string()}),
        };
        answer.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(games: &[&[&str]]) -> Vec<Vec<String>> {
        games.iter().map(|names| names.iter().map(|name| (*name).to_owned()).collect()).collect()
    }

    fn group(name: Option<&str>, games: &[usize]) -> Group {
        Group { name: name.map(str::to_owned), games: games.to_vec() }
    }

    #[test]
    fn a_game_in_several_shows_in_each_and_those_in_none_come_last() {
        let games = names(&[&["VR", "Couch"], &[], &["couch"], &["Arcade"], &[]]);
        assert_eq!(
            groups(&games),
            [group(Some("Arcade"), &[3]), group(Some("Couch"), &[0, 2]), group(Some("VR"), &[0]), group(None, &[1, 4])]
        );
    }

    #[test]
    fn no_collections_means_no_groups_and_no_empty_last_group() {
        assert!(groups(&names(&[&[], &[]])).is_empty());
        assert!(groups(&names(&[&["  "]])).is_empty(), "a blank name is none");
        assert!(groups::<String>(&[]).is_empty());
        assert_eq!(groups(&names(&[&["A"]])), [group(Some("A"), &[0])]);
    }

    #[test]
    fn capitals_and_spaces_are_one_collection_and_a_game_shows_once_in_it() {
        assert_eq!(groups(&names(&[&["  VR ", "vr"], &["Vr"]])), [group(Some("VR"), &[0, 1])]);
    }

    #[test]
    fn the_json_call_answers_and_refuses() {
        let answer: serde_json::Value =
            serde_json::from_str(&json::call(r#"{"games":[["A"],[]]}"#)).unwrap();
        assert_eq!(answer, serde_json::json!({"ok":{"groups":[{"name":"A","games":[0]},{"name":null,"games":[1]}]}}));
        let refused: serde_json::Value = serde_json::from_str(&json::call("[")).unwrap();
        assert_eq!(refused["error"], "invalid");
    }
}
