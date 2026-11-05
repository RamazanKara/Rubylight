//! The reads that watch an open "Now playing on ...": one at a time, a limited number of them, and
//! each answer is for the prompt that asked.

use crate::hosts::MOVE_WATCH_READS;

#[derive(Default)]
pub struct MoveWatch {
    token: u64,
    reads: u32,
    reading: bool,
}

impl MoveWatch {
    /// A prompt opened: it gets its own reads, whatever the one before it still has in flight.
    pub fn open(&mut self) {
        self.token += 1;
        self.reads = MOVE_WATCH_READS;
        self.reading = false;
    }

    /// The token for the next read, if one is due.
    pub fn next(&mut self) -> Option<u64> {
        if self.reading || self.reads == 0 {
            return None;
        }
        self.reads -= 1;
        self.reading = true;
        Some(self.token)
    }

    /// The read could not start, and no answer comes.
    pub fn not_started(&mut self) {
        self.reading = false;
    }

    /// An answer came. False when it is for a prompt that is gone: it then changes nothing here.
    pub fn answered(&mut self, token: u64) -> bool {
        if token != self.token {
            return false;
        }
        self.reading = false;
        true
    }

    /// The PC said how it ended: no more reads for this prompt.
    pub fn done(&mut self) {
        self.reads = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_answer_for_a_prompt_that_is_gone_leaves_the_next_prompt_watched() {
        let mut watch = MoveWatch::default();
        watch.open();
        let first = watch.next().expect("the first prompt reads");

        // Its prompt closed with that read in flight, and another PC's opened.
        watch.open();
        let second = watch.next().expect("it reads at once, not after the old answer");
        assert_ne!(first, second);

        assert!(!watch.answered(first), "the old answer is not for this prompt");
        assert_eq!(watch.next(), None, "its own read is still out");
        assert!(watch.answered(second));
        assert_eq!(watch.next(), Some(second), "and the watch goes on");
    }

    #[test]
    fn one_read_at_a_time_up_to_the_limit_and_none_after_the_end() {
        let mut watch = MoveWatch::default();
        assert_eq!(watch.next(), None, "no prompt");
        watch.open();
        for _ in 0..MOVE_WATCH_READS {
            let token = watch.next().expect("a read is due");
            assert_eq!(watch.next(), None);
            assert!(watch.answered(token));
        }
        assert_eq!(watch.next(), None, "a dead PC is not asked forever");

        watch.open();
        let token = watch.next().unwrap();
        assert!(watch.answered(token));
        watch.done();
        assert_eq!(watch.next(), None);

        watch.open();
        let token = watch.next().unwrap();
        watch.not_started();
        assert_eq!(watch.next(), Some(token), "nothing is on its way");
    }
}
