//! "Controllers": whether controllers reach the PC at all, and as one player or a player each.
//! One choice on both clients, kept in two switches older versions read: send controllers
//! (desktop `controllers`, Android `checkbox_send_controllers`) and a player each (desktop
//! `multi_controller`, Android `checkbox_multi_controller`). Off keeps the second as it was, so
//! turning controllers on again brings back the last way they were sent.

/// The choice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Controllers {
    /// No controller input goes to the PC.
    Off,
    /// Every controller is player 1.
    OnePlayer,
    /// Each controller is its own player.
    PlayerEach,
}

impl Controllers {
    /// The choices in the order both clients list them.
    pub const ALL: [Self; 3] = [Self::Off, Self::OnePlayer, Self::PlayerEach];

    /// The choice the two saved switches make.
    pub fn from_saved(send: bool, player_each: bool) -> Self {
        match (send, player_each) {
            (false, _) => Self::Off,
            (true, false) => Self::OnePlayer,
            (true, true) => Self::PlayerEach,
        }
    }

    /// The two switches to save for this choice: send, and a player each (Off keeps `player_each`).
    pub fn to_saved(self, player_each: bool) -> (bool, bool) {
        match self {
            Self::Off => (false, player_each),
            Self::OnePlayer => (true, false),
            Self::PlayerEach => (true, true),
        }
    }

    /// The gamepad number controller `index` (0-based, in connection order) is sent as, or None
    /// when it is not sent.
    pub fn sent_index(self, index: u8) -> Option<u8> {
        match self {
            Self::Off => None,
            Self::OnePlayer => Some(0),
            Self::PlayerEach => Some(index),
        }
    }

    /// The choice's name on the JSON wire and in logs.
    pub fn key(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::OnePlayer => "one_player",
            Self::PlayerEach => "player_each",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|choice| choice.key() == key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn existing_switches_read_as_before() {
        // Desktop's `controllers` off was Off; Android's multi-controller off was one player.
        assert_eq!(Controllers::from_saved(false, true), Controllers::Off);
        assert_eq!(Controllers::from_saved(false, false), Controllers::Off);
        assert_eq!(Controllers::from_saved(true, false), Controllers::OnePlayer);
        assert_eq!(Controllers::from_saved(true, true), Controllers::PlayerEach, "both defaults");
    }

    #[test]
    fn each_choice_saves_what_reads_back_as_it() {
        for choice in Controllers::ALL {
            for player_each in [true, false] {
                let (send, each) = choice.to_saved(player_each);
                assert_eq!(Controllers::from_saved(send, each), choice);
                assert_eq!(Controllers::from_key(choice.key()), Some(choice));
            }
        }
        assert_eq!(Controllers::Off.to_saved(false), (false, false), "Off keeps the player switch");
        assert_eq!(Controllers::Off.to_saved(true), (false, true));
        assert_eq!(Controllers::from_key("everyone"), None);
    }

    #[test]
    fn one_player_sends_every_controller_as_the_first() {
        assert_eq!(Controllers::OnePlayer.sent_index(3), Some(0));
        assert_eq!(Controllers::PlayerEach.sent_index(3), Some(3));
        assert_eq!(Controllers::Off.sent_index(0), None);
    }
}
