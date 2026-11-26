//! `dpi_settle`: after a display's Windows scale is put back, read it again for
//! a bounded time and put it back once more when Windows moved it (issue #129).
//! Windows can reassign a scale shortly after the topology changes, such as
//! when a virtual display goes away, so one successful set proves little.

use anyhow::Result;
use std::{collections::BTreeMap, time::Duration};

/// When the scales are read, in milliseconds after the restore. Two reads in a
/// row that find every display at its scale end the wait early; the last read
/// is the bound.
pub(crate) const CHECKS_MS: [u64; 5] = [0, 150, 500, 1000, 1500];

/// What [`settle`] found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Settled {
    /// Milliseconds after the restore of the last read.
    pub after_ms: u64,
    /// Whether two reads in a row found every connected display at its scale.
    pub settled: bool,
    /// Displays not connected at the last read; nothing was set on them.
    pub absent: Vec<String>,
    /// Displays still at another scale at the last read, or that could not be
    /// read then.
    pub drifted: Vec<String>,
    /// How many times a scale was put back.
    pub reapplied: u32,
}

/// Bring every display in `expected` (device id to percent) to its scale and
/// check that it stays there: read the scales at each of [`CHECKS_MS`], put
/// back any that differ, and stop after two matching reads in a row. `read`
/// lists the scales of the displays connected now; an error counts as a read
/// that matched nothing. A display that is not connected is left alone.
pub(crate) fn settle(
    expected: &BTreeMap<String, u32>,
    mut read: impl FnMut() -> Result<BTreeMap<String, u32>>,
    mut set: impl FnMut(&str, u32) -> Result<()>,
    mut sleep: impl FnMut(Duration),
) -> Settled {
    let mut outcome = Settled::default();
    if expected.is_empty() {
        outcome.settled = true;
        return outcome;
    }
    let mut matched_in_a_row = 0;
    let mut waited = 0;
    let last = CHECKS_MS[CHECKS_MS.len() - 1];
    for at in CHECKS_MS {
        if at > waited {
            sleep(Duration::from_millis(at - waited));
            waited = at;
        }
        outcome.after_ms = at;
        let current = match read() {
            Ok(current) => current,
            Err(error) => {
                tracing::debug!(error = %format!("{error:#}"), "the display scales could not be read");
                matched_in_a_row = 0;
                outcome.absent.clear();
                outcome.drifted = expected.keys().cloned().collect();
                continue;
            }
        };
        outcome.absent = expected
            .keys()
            .filter(|id| !current.contains_key(*id))
            .cloned()
            .collect();
        let wrong: Vec<(&String, u32)> = expected
            .iter()
            .filter(|(id, want)| current.get(*id).is_some_and(|now| now != *want))
            .map(|(id, want)| (id, *want))
            .collect();
        outcome.drifted = wrong.iter().map(|(id, _)| (*id).clone()).collect();
        if wrong.is_empty() {
            matched_in_a_row += 1;
            if matched_in_a_row >= 2 {
                outcome.settled = true;
                return outcome;
            }
            continue;
        }
        matched_in_a_row = 0;
        // A set after the last read could not be checked any more.
        if at == last {
            break;
        }
        for (id, want) in wrong {
            outcome.reapplied += 1;
            if let Err(error) = set(id, want) {
                tracing::warn!(display = %id, scale = want, error = %format!("{error:#}"), "could not put a display's Windows scale back");
            }
        }
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// A fake Windows: `script` is what each read returns before the host sets
    /// anything; a set changes the display's scale from then on.
    struct Fake {
        now: RefCell<BTreeMap<String, u32>>,
        /// Reads after which Windows moves a display by itself: (read index, id, percent).
        moves: Vec<(usize, &'static str, u32)>,
        reads: RefCell<usize>,
        slept: RefCell<Vec<u64>>,
        sets: RefCell<Vec<(String, u32)>>,
        refuse: bool,
    }
    impl Fake {
        fn new(now: &[(&str, u32)]) -> Self {
            Self {
                now: RefCell::new(now.iter().map(|(id, p)| ((*id).into(), *p)).collect()),
                moves: Vec::new(),
                reads: RefCell::new(0),
                slept: RefCell::new(Vec::new()),
                sets: RefCell::new(Vec::new()),
                refuse: false,
            }
        }
        fn run(&self, expected: &[(&str, u32)]) -> Settled {
            let expected = expected.iter().map(|(id, p)| ((*id).into(), *p)).collect();
            settle(
                &expected,
                || {
                    let index = *self.reads.borrow();
                    for (at, id, percent) in &self.moves {
                        if *at == index {
                            self.now.borrow_mut().insert((*id).into(), *percent);
                        }
                    }
                    *self.reads.borrow_mut() += 1;
                    Ok(self.now.borrow().clone())
                },
                |id, percent| {
                    self.sets.borrow_mut().push((id.into(), percent));
                    if self.refuse {
                        anyhow::bail!("refused");
                    }
                    self.now.borrow_mut().insert(id.into(), percent);
                    Ok(())
                },
                |d| self.slept.borrow_mut().push(d.as_millis() as u64),
            )
        }
    }

    #[test]
    fn a_display_at_its_scale_settles_after_two_reads_without_a_set() {
        let fake = Fake::new(&[("primary", 150), ("side", 100)]);
        let outcome = fake.run(&[("primary", 150), ("side", 100)]);
        assert!(outcome.settled);
        assert_eq!(outcome.after_ms, 150);
        assert_eq!(outcome.reapplied, 0);
        assert!(fake.sets.borrow().is_empty());
        assert_eq!(*fake.slept.borrow(), vec![150]);
    }

    #[test]
    fn a_scale_windows_moved_is_put_back_and_checked_again() {
        let fake = Fake::new(&[("primary", 100), ("side", 100)]);
        let outcome = fake.run(&[("primary", 150), ("side", 100)]);
        assert!(outcome.settled);
        assert_eq!(outcome.after_ms, 500);
        assert_eq!(outcome.reapplied, 1);
        assert_eq!(*fake.sets.borrow(), vec![("primary".to_owned(), 150)]);
        assert!(outcome.drifted.is_empty());
    }

    #[test]
    fn a_scale_windows_moves_again_after_the_restore_is_put_back_once_more() {
        let mut fake = Fake::new(&[("primary", 150)]);
        // Windows reassigns the scale between the first and second read, as
        // after the virtual display's removal finishes.
        fake.moves.push((1, "primary", 100));
        let outcome = fake.run(&[("primary", 150)]);
        assert!(outcome.settled);
        assert_eq!(outcome.reapplied, 1);
        assert_eq!(*fake.sets.borrow(), vec![("primary".to_owned(), 150)]);
        assert_eq!(outcome.after_ms, 1000);
        assert_eq!(*fake.slept.borrow(), vec![150, 350, 500]);
    }

    #[test]
    fn a_scale_that_will_not_stay_is_reported_within_the_bound() {
        let mut fake = Fake::new(&[("primary", 100)]);
        fake.refuse = true;
        let outcome = fake.run(&[("primary", 150)]);
        assert!(!outcome.settled);
        assert_eq!(outcome.after_ms, 1500);
        assert_eq!(outcome.drifted, vec!["primary".to_owned()]);
        // No set after the last read, which nothing would check.
        assert_eq!(outcome.reapplied, 4);
        assert_eq!(fake.slept.borrow().iter().sum::<u64>(), 1500);
    }

    #[test]
    fn a_display_that_is_not_connected_is_left_alone() {
        let fake = Fake::new(&[("primary", 150)]);
        let outcome = fake.run(&[("primary", 150), ("tv", 200)]);
        assert!(outcome.settled);
        assert_eq!(outcome.absent, vec!["tv".to_owned()]);
        assert!(fake.sets.borrow().is_empty());
    }

    #[test]
    fn nothing_to_settle_returns_at_once() {
        let fake = Fake::new(&[]);
        let outcome = fake.run(&[]);
        assert!(outcome.settled);
        assert_eq!(*fake.reads.borrow(), 0);
        assert!(fake.slept.borrow().is_empty());
    }

    #[test]
    fn an_unreadable_last_check_keeps_every_display_to_settle_later() {
        let expected = BTreeMap::from([("primary".to_owned(), 150)]);
        let outcome = settle(
            &expected,
            || anyhow::bail!("no displays"),
            |_, _| Ok(()),
            |_| {},
        );
        assert!(!outcome.settled);
        assert_eq!(outcome.drifted, vec!["primary".to_owned()]);
        assert!(outcome.absent.is_empty());
    }
}
