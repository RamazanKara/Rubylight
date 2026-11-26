//! Quick Suspend on the desktop: the stream menu's Suspend game and Resume game tiles and the line
//! the stream shows while the game is paused, over control message 0x554B. The host pauses and
//! resumes the game's processes and decides who may; Nightjar asks for the state once at the start,
//! takes the host's answers and pushed changes, and words a refusal in the user's language
//! (`nightjar_policy::quick_suspend`, the same words as Android's).

use super::*;
use nightjar_policy::quick_suspend::{self as policy, Words};
use rubylight_protocol::quick_suspend::{ClientRequest, HostState, Op, State};

/// What the host last said about Quick Suspend on this stream.
#[derive(Clone, Debug, Default)]
pub struct QuickSuspend {
    /// The host announced 0x554B for this stream.
    offered: bool,
    /// The host's last state other than a refusal; None until it answers.
    state: Option<State>,
    /// The last request's id, 0 before the first: an answer with it is this device's.
    last_id: u32,
}

impl QuickSuspend {
    pub fn new(offered: bool) -> Self {
        Self {
            offered,
            ..Self::default()
        }
    }

    /// The next request, under a fresh nonzero id.
    fn request(&mut self, op: Op) -> ClientRequest {
        self.last_id = self.last_id.wrapping_add(1).max(1);
        ClientRequest {
            flags: 0,
            request_id: self.last_id,
            op,
        }
    }

    /// The menu's tile: Some(false) offers Suspend game, Some(true) Resume game, None neither
    /// (Quick Suspend off for this game, another device's game, no answer yet).
    pub fn tile(&self) -> Option<bool> {
        let name = state_name(self.state?);
        policy::tile(name).map(|_| policy::paused(name))
    }

    /// The line the stream shows while the game is paused.
    pub fn banner(&self) -> Option<&'static str> {
        self.paused().then(|| say(policy::PAUSED))
    }

    fn paused(&self) -> bool {
        self.state
            .is_some_and(|state| policy::paused(state_name(state)))
    }

    /// Takes the host's report and gives the line to show for it, if any. A refusal leaves the
    /// game as it was, so the tile stays; only the device that asked hears why.
    fn answered(&mut self, report: HostState) -> Option<&'static str> {
        if report.state == State::Refused {
            let ours = report.request_id != 0 && report.request_id == self.last_id;
            return ours.then(|| say(policy::refusal(report.reason.as_str())));
        }
        let was_paused = self.paused();
        self.state = Some(report.state);
        (was_paused && report.state == State::Ready).then(|| say(policy::RESUMED))
    }
}

impl Streaming {
    /// Asks for the state the menu's tile needs, once at the start.
    pub(super) fn query_quick_suspend(&mut self) {
        if self.quick_suspend.offered {
            self.send_quick_suspend(Op::QueryState);
        }
    }

    /// Suspend game or Resume game, chosen in the stream menu.
    pub fn quick_suspend_tile(&mut self, id: &str) {
        self.send_quick_suspend(if id == policy::RESUME_TILE {
            Op::Resume
        } else {
            Op::Suspend
        });
    }

    fn send_quick_suspend(&mut self, op: Op) {
        let request = self.quick_suspend.request(op);
        if let Err(error) = self.session.send(ClientMessage::QuickSuspend(request)) {
            eprintln!("nightjar: quick suspend: {error}");
            self.show_toast(error.to_string());
        }
    }

    /// The host's answer to a request or a change it pushed.
    pub(super) fn quick_suspend_state(&mut self, report: HostState) {
        if let Some(line) = self.quick_suspend.answered(report) {
            self.show_toast(line);
        }
        self.needs_redraw = true;
    }
}

fn state_name(state: State) -> &'static str {
    match state {
        State::Unavailable => "unavailable",
        State::Ready => "ready",
        State::Suspended => "suspended",
        State::Resuming => "resuming",
        State::Refused => "refused",
    }
}

/// The shared words in the user's language. Each English text is written out so the translation
/// tests find it in every table.
fn say(words: Words) -> &'static str {
    match words.key {
        "paused" => tr("The game is paused on the PC. Open the stream menu to resume it."),
        "resumed" => tr("Game resumed."),
        "refused_anti_cheat" => tr("This game runs anti-cheat, so the PC won’t pause it."),
        "refused_protected" => {
            tr("A protected or system program runs with this game, so the PC won’t pause it.")
        }
        "refused_others" => tr("Others are playing or watching, so the PC won’t pause the game."),
        "refused_not_owner" => {
            tr("Only the device that started the game, or one with host control, can pause it.")
        }
        "refused_busy" => tr("The game is starting or stopping. Try again in a moment."),
        "refused_nothing" => tr("There is no game the PC can pause right now."),
        _ => tr("The PC couldn’t pause the game, so nothing stays paused."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::strings::{with_lang, Lang};
    use rubylight_protocol::quick_suspend::Reason;

    fn report(request_id: u32, state: State, reason: Reason) -> HostState {
        HostState {
            flags: 0,
            request_id,
            state,
            reason,
            suspended_count: 0,
            suspended_since_unix_ms: 0,
        }
    }

    #[test]
    fn the_desktop_says_what_android_says() {
        let all = [policy::PAUSED, policy::RESUMED]
            .into_iter()
            .chain(policy::REFUSALS);
        for words in all {
            assert_eq!(
                with_lang(Lang::En, || say(words)),
                words.en,
                "{}",
                words.key
            );
            assert_ne!(
                with_lang(Lang::De, || say(words)),
                words.en,
                "{} has no German",
                words.key
            );
        }
    }

    #[test]
    fn the_tile_follows_the_host_and_a_refusal_leaves_the_game_as_it_was() {
        let mut quick = QuickSuspend::new(true);
        assert_eq!(quick.tile(), None, "nothing before the host answers");
        let query = quick.request(Op::QueryState);
        assert_eq!((query.flags, query.request_id), (0, 1));
        assert_eq!(quick.answered(report(1, State::Ready, Reason::None)), None);
        assert_eq!(quick.tile(), Some(false));
        assert_eq!(quick.banner(), None);

        let suspend = quick.request(Op::Suspend);
        assert_eq!(suspend.request_id, 2);
        let refused = with_lang(Lang::En, || {
            quick.answered(report(2, State::Refused, Reason::AntiCheatDetected))
        });
        assert_eq!(refused, Some(policy::REFUSALS[0].en));
        assert_eq!(quick.tile(), Some(false), "a refusal keeps the tile");
        // Another device's refusal is not this one's to show.
        assert_eq!(
            quick.answered(report(9, State::Refused, Reason::NotOwner)),
            None
        );

        assert_eq!(
            quick.answered(report(0, State::Suspended, Reason::None)),
            None
        );
        assert_eq!(quick.tile(), Some(true));
        assert_eq!(
            with_lang(Lang::En, || quick.banner()),
            Some(policy::PAUSED.en)
        );
        assert_eq!(
            quick.answered(report(0, State::Resuming, Reason::None)),
            None
        );
        assert_eq!(quick.tile(), Some(true));
        let resumed = with_lang(Lang::En, || {
            quick.answered(report(3, State::Ready, Reason::None))
        });
        assert_eq!(resumed, Some(policy::RESUMED.en));
        assert_eq!(quick.banner(), None);

        assert_eq!(
            quick.answered(report(0, State::Unavailable, Reason::DisabledForApp)),
            None
        );
        assert_eq!(quick.tile(), None, "Quick Suspend off for this game");
    }

    #[test]
    fn request_ids_are_never_zero() {
        let mut quick = QuickSuspend {
            last_id: u32::MAX,
            ..QuickSuspend::new(true)
        };
        assert_eq!(quick.request(Op::Resume).request_id, 1);
    }
}
