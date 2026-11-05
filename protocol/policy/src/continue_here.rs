//! Continue here: when selecting a game asks, and what a stream that moved away may do. Android's
//! `ContinueHerePolicy` follows the same traces.
use crate::trace::{Args, Subject};
use serde_json::Value;

/// Whether selecting a game asks before starting it: another device plays exactly this game, the
/// PC offers the move and this is a plain stream start. A host action, an Input-only or co-op
/// start, and any other game go on as they always did.
pub fn asks_on_select(offered: bool, plain_start: bool, same_game: bool) -> bool {
    offered && plain_start && same_game
}

/// Whether a stream that ended asks the PC if it moved: any end this device did not ask for, on a
/// PC that offers the move. One that goes on to reconnect does not wait for the answer on a link
/// that may be dead: its resume asks once the PC answered, where the game is no longer its own.
pub fn reads_move_after_end(offered: bool, left_on_purpose: bool, recovery_follows: bool) -> bool {
    offered && !left_on_purpose && !recovery_follows
}

/// Whether a stream may reconnect on its own. One that moved never does: two devices must not
/// fight over the stream. A PC that could not be asked (`moved` false) is asked again before the
/// reconnect launches, by the same check on the resume path.
pub fn recovers(transient: bool, moved: bool) -> bool {
    transient && !moved
}

/// Whether a prompt saying the stream moved turns into "the game came back": the PC no longer says
/// it moved and this device owns the game again, so its press failed. A note that ran out with the
/// game on the other device, or a game that ended, is no reason to offer anything.
pub fn came_back(moved: bool, owns: bool) -> bool {
    !moved && owns
}

/// Whether a resume joins the game the PC runs, once the PC answered who plays: `/resume` joins it
/// whoever owns it, so only an answer that says it is another device's may. `too`: the user already
/// chose to stream beside it ("did not move", Stream here too). Otherwise another device's same game
/// asks first, and only a press that failed with nobody to ask about joins. A PC that gave no answer
/// is no reason to join: the caller stays with the resume that fails if the game is not its own.
pub fn joins_after_read(too: bool, same_game: bool, press_failed: bool) -> bool {
    if too {
        same_game || press_failed
    } else {
        !same_game && press_failed
    }
}

pub(crate) fn traced(args: &Value) -> Result<Box<dyn Subject>, String> {
    if !args.is_null() {
        return Err("static policy: new must be null".into());
    }
    Ok(Box::new(Traced))
}

struct Traced;

impl Subject for Traced {
    fn call(&mut self, method: &str, args: &Value) -> Result<Value, String> {
        match method {
            "asks_on_select" => {
                let a = Args::new(args, 3)?;
                Ok(asks_on_select(a.flag(0)?, a.flag(1)?, a.flag(2)?).into())
            }
            "reads_move_after_end" => {
                let a = Args::new(args, 3)?;
                Ok(reads_move_after_end(a.flag(0)?, a.flag(1)?, a.flag(2)?).into())
            }
            "recovers" => {
                let a = Args::new(args, 2)?;
                Ok(recovers(a.flag(0)?, a.flag(1)?).into())
            }
            "came_back" => {
                let a = Args::new(args, 2)?;
                Ok(came_back(a.flag(0)?, a.flag(1)?).into())
            }
            "joins_after_read" => {
                let a = Args::new(args, 3)?;
                Ok(joins_after_read(a.flag(0)?, a.flag(1)?, a.flag(2)?).into())
            }
            other => Err(format!("unknown method {other}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stream_that_moved_never_reconnects() {
        assert!(!recovers(true, true));
        assert!(!recovers(false, true));
        assert!(recovers(true, false));
        assert!(!recovers(false, false));
    }

    #[test]
    fn only_a_game_this_device_owns_again_came_back() {
        assert!(came_back(false, true));
        assert!(!came_back(true, true));
        assert!(!came_back(false, false));
        assert!(!came_back(true, false));
    }

    #[test]
    fn only_an_answer_that_the_game_is_another_devices_joins() {
        assert!(joins_after_read(true, true, false));
        assert!(joins_after_read(true, false, true));
        assert!(!joins_after_read(true, false, false));
        assert!(joins_after_read(false, false, true));
        assert!(!joins_after_read(false, true, true), "it asks first");
        assert!(!joins_after_read(false, false, false));
    }

    #[test]
    fn only_another_devices_same_game_on_an_offering_pc_asks() {
        assert!(asks_on_select(true, true, true));
        for (offered, plain, same) in [
            (false, true, true),
            (true, false, true),
            (true, true, false),
        ] {
            assert!(!asks_on_select(offered, plain, same));
        }
    }

    #[test]
    fn leaving_on_purpose_an_old_pc_or_a_coming_recovery_reads_nothing() {
        assert!(reads_move_after_end(true, false, false));
        assert!(!reads_move_after_end(true, true, false));
        assert!(!reads_move_after_end(false, false, false));
        assert!(!reads_move_after_end(true, false, true));
    }
}
