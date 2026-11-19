//! The order Home and All games list a library in, the same on desktop and Android
//! (`com.limelight.console.LibraryOrder` through a JSON call). A PC sends each game's title, when it
//! was last played and how long it was played; this says which comes first.

use std::cmp::Ordering;

use serde::{Deserialize, Serialize};

/// What the library is sorted by.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SortKey {
    #[default]
    Title,
    LastPlayed,
    Playtime,
}

/// Which way: ascending is A to Z, oldest first and least played first.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    #[default]
    Ascending,
    Descending,
}

/// What the order reads of one game.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct Facts {
    pub title: String,
    /// Seconds since the epoch, `0` for never played, `None` when no PC says.
    #[serde(default)]
    pub last_played: Option<u64>,
    /// Minutes, `0` for never played, `None` when no PC says.
    #[serde(default)]
    pub playtime_minutes: Option<u64>,
}

/// The places of `games` in the order `key` and `direction` give. A game no PC says anything about
/// counts as never played, so it sits with them; equal games go by title, A to Z, then by place.
pub fn order(games: &[Facts], key: SortKey, direction: Direction) -> Vec<usize> {
    let titles: Vec<String> = games.iter().map(|game| game.title.to_lowercase()).collect();
    let mut places: Vec<usize> = (0..games.len()).collect();
    places.sort_by(|&a, &b| {
        let by_key = match key {
            SortKey::Title => titles[a].cmp(&titles[b]),
            SortKey::LastPlayed => games[a].last_played.unwrap_or(0).cmp(&games[b].last_played.unwrap_or(0)),
            SortKey::Playtime => games[a].playtime_minutes.unwrap_or(0).cmp(&games[b].playtime_minutes.unwrap_or(0)),
        };
        let by_key = match direction {
            Direction::Ascending => by_key,
            Direction::Descending => by_key.reverse(),
        };
        by_key.then_with(|| if key == SortKey::Title { Ordering::Equal } else { titles[a].cmp(&titles[b]) })
    });
    places
}

/// The one JSON call Android's app reaches this through.
pub mod json {
    use super::*;
    use serde_json::{json, Value};

    #[derive(Deserialize)]
    struct Request {
        #[serde(default)]
        key: SortKey,
        #[serde(default)]
        direction: Direction,
        /// Each game, in the order of the list.
        games: Vec<Facts>,
    }

    /// `{"key": "title" | "last_played" | "playtime", "direction": "ascending" | "descending", "games":
    /// [{"title": t, "last_played": s | null, "playtime_minutes": m | null}, ...]}` answers `{"ok":
    /// {"order": [place, ...]}}`, or `{"error": "invalid", "detail": why}`.
    pub fn call(request: &str) -> String {
        let answer: Value = match serde_json::from_str::<Request>(request) {
            Ok(request) => json!({"ok": {"order": order(&request.games, request.key, request.direction)}}),
            Err(error) => json!({"error": "invalid", "detail": error.to_string()}),
        };
        answer.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn game(title: &str, last_played: Option<u64>, playtime_minutes: Option<u64>) -> Facts {
        Facts { title: title.to_owned(), last_played, playtime_minutes }
    }

    fn games() -> Vec<Facts> {
        vec![
            game("hades", Some(300), Some(40)),
            game("Celeste", Some(0), Some(0)),
            game("Balatro", Some(900), Some(600)),
            game("Desktop", None, None),
            game("Apex", Some(300), Some(40)),
        ]
    }

    #[test]
    fn title_ignores_capitals_both_ways() {
        assert_eq!(order(&games(), SortKey::Title, Direction::Ascending), [4, 2, 1, 3, 0]);
        assert_eq!(order(&games(), SortKey::Title, Direction::Descending), [0, 3, 1, 2, 4]);
    }

    #[test]
    fn last_played_puts_never_and_unknown_together_and_ties_go_by_title() {
        assert_eq!(order(&games(), SortKey::LastPlayed, Direction::Descending), [2, 4, 0, 1, 3]);
        assert_eq!(order(&games(), SortKey::LastPlayed, Direction::Ascending), [1, 3, 4, 0, 2]);
    }

    #[test]
    fn playtime_sorts_like_last_played() {
        assert_eq!(order(&games(), SortKey::Playtime, Direction::Descending), [2, 4, 0, 1, 3]);
        assert_eq!(order(&games(), SortKey::Playtime, Direction::Ascending), [1, 3, 4, 0, 2]);
    }

    #[test]
    fn equal_titles_keep_their_places() {
        let same = [game("A", None, None), game("a", None, None)];
        assert_eq!(order(&same, SortKey::Title, Direction::Descending), [0, 1]);
        assert!(order(&[], SortKey::Playtime, Direction::Ascending).is_empty());
    }

    #[test]
    fn the_json_call_answers_defaults_and_refuses() {
        let answer: serde_json::Value = serde_json::from_str(&json::call(
            r#"{"key":"last_played","direction":"descending","games":[{"title":"B","last_played":5},{"title":"A","last_played":null}]}"#,
        ))
        .unwrap();
        assert_eq!(answer, serde_json::json!({"ok":{"order":[0,1]}}));
        let defaults: serde_json::Value =
            serde_json::from_str(&json::call(r#"{"games":[{"title":"b"},{"title":"A"}]}"#)).unwrap();
        assert_eq!(defaults, serde_json::json!({"ok":{"order":[1,0]}}), "title, ascending");
        let refused: serde_json::Value = serde_json::from_str(&json::call(r#"{"key":"size","games":[]}"#)).unwrap();
        assert_eq!(refused["error"], "invalid");
    }
}
