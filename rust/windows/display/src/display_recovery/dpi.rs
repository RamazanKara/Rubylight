//! The Windows scale of every physical display from before a virtual display
//! joined the desktop, kept in the recovery journal and put back once the last
//! virtual display is gone, or by recovery after a crash (issue #129). Windows
//! can give a physical display another scale while a virtual one comes or
//! goes; [`super::dpi_settle`] checks that the restored scale stays.

use super::{Journal, PATH, dpi_settle, lock, read_journal};
use anyhow::{Context, Result};
use std::{
    collections::BTreeMap,
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

/// The journal entry that holds the baseline.
pub(super) const ID: &str = "physical-dpi-baseline";
/// How long a restore waits for the virtual displays to leave Windows after
/// they were removed (a clean end) or their host died (the driver drops them
/// when the host's handles close, or after ten silent seconds).
pub(super) const LEAVE_AFTER_REMOVAL: Duration = Duration::from_secs(5);
pub(super) const LEAVE_AFTER_CRASH: Duration = Duration::from_secs(12);

/// How many virtual displays hold the baseline now. Never held while the
/// journal lock is taken, so a restore may read it under that lock.
static USERS: Mutex<usize> = Mutex::new(0);
/// One restore at a time; a new virtual display never waits for one.
static RESTORE: Mutex<()> = Mutex::new(());
/// Counts virtual displays acquired, so a restore can tell that one came and
/// went while it ran.
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// Held by every virtual display for its lifetime. The first one records each
/// physical display's scale before the virtual display is added; when the last
/// one is dropped, after its display was removed, every scale Windows left
/// elsewhere is put back and checked. Nothing changes for a display whose
/// scale stayed.
pub struct DpiBaseline {
    /// The displays this token's record put in the baseline.
    recorded: Vec<String>,
    /// [`GENERATION`] once this token was counted.
    generation: u64,
    stage: Stage,
}

/// How far creating a token's virtual display got.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    /// Windows does not have the display: no scale moved for it.
    Recorded,
    /// The driver added the display, and creating it failed after that.
    Added,
    /// The display was created.
    Created,
}

/// Where the last token's drop puts the scales back.
#[derive(Debug, PartialEq, Eq)]
enum Restore {
    /// Here: the display ended after it was created.
    InPlace,
    /// On a thread of its own: the virtual display leases' lock is held, by a
    /// failed creation or a lease that dropped the last stream's display,
    /// which a restore would keep for seconds.
    Aside,
    No,
}

thread_local! {
    /// Whether this thread holds the virtual display leases' lock.
    static LEASING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Run `f`, which holds the virtual display leases' lock: a token dropped
/// meanwhile puts the scales back on a thread of its own.
pub fn while_leasing<R>(f: impl FnOnce() -> R) -> R {
    struct Reset(bool);
    impl Drop for Reset {
        fn drop(&mut self) {
            LEASING.set(self.0);
        }
    }
    let _reset = Reset(LEASING.replace(true));
    f()
}

impl DpiBaseline {
    pub fn acquire() -> Self {
        let (first, generation) = {
            let mut users = crate::lock_unpoisoned(&USERS);
            *users += 1;
            (*users == 1, GENERATION.fetch_add(1, Ordering::AcqRel) + 1)
        };
        let recorded = if first {
            record().unwrap_or_else(|error| {
                tracing::warn!(error = %format!("{error:#}"), "could not record the physical displays' Windows scale before adding a virtual display");
                Vec::new()
            })
        } else {
            Vec::new()
        };
        Self {
            recorded,
            generation,
            stage: Stage::Recorded,
        }
    }

    /// The driver added the display: Windows may move the scales from now on.
    pub fn added(&mut self) {
        self.stage = Stage::Added;
    }

    /// The display was created; the end of its last stream drops the token.
    pub fn created(&mut self) {
        self.stage = Stage::Created;
    }
}

impl Drop for DpiBaseline {
    fn drop(&mut self) {
        let last = {
            let mut users = crate::lock_unpoisoned(&USERS);
            *users = users.saturating_sub(1);
            *users == 0
        };
        if !last {
            return;
        }
        let owed = || {
            abandon(&self.recorded, self.generation).unwrap_or_else(|error| {
                tracing::warn!(error = %format!("{error:#}"), "could not take a failed virtual display's scales out of the baseline");
                false
            })
        };
        match last_drop(self.stage, LEASING.get(), owed) {
            Restore::InPlace => restore_logged(),
            Restore::Aside => {
                if let Err(error) = std::thread::Builder::new()
                    .name("dpi-restore".into())
                    .spawn(restore_logged)
                {
                    tracing::warn!(error = %error, "could not start putting the physical displays' Windows scale back");
                }
            }
            Restore::No => {}
        }
    }
}

/// What the last token's drop does. A creation that failed before Windows had
/// the display moved no scale: only a restore an earlier virtual display left
/// owed (`owed`, which takes this one's record out) still runs. `leasing`
/// when this thread holds the virtual display leases' lock.
fn last_drop(stage: Stage, leasing: bool, owed: impl FnOnce() -> bool) -> Restore {
    match stage {
        Stage::Created if !leasing => Restore::InPlace,
        Stage::Created | Stage::Added => Restore::Aside,
        Stage::Recorded if owed() => Restore::Aside,
        Stage::Recorded => Restore::No,
    }
}

fn restore_logged() {
    if let Err(error) = restore() {
        tracing::warn!(error = %format!("{error:#}"), "could not put the physical displays' Windows scale back");
    }
}

/// Take out what a failed creation's token recorded, unless a virtual display
/// counted since holds the baseline. True when it still holds scales to put
/// back.
fn abandon(recorded: &[String], generation: u64) -> Result<bool> {
    if PATH.get().is_none() {
        return Ok(false);
    }
    let mut owed = false;
    super::change_journal(|journal| {
        if !in_use() {
            let alone = GENERATION.load(Ordering::Acquire) == generation;
            owed = abandoned(journal, recorded, alone);
        }
    })?;
    Ok(owed)
}

/// [`abandon`] on the journal: `alone` when no virtual display was counted
/// after the failed one, which could rely on its record.
pub(super) fn abandoned(journal: &mut Journal, recorded: &[String], alone: bool) -> bool {
    if alone {
        for id in recorded {
            forget(journal, id);
        }
    }
    journal
        .entries
        .get(ID)
        .and_then(|entry| entry.dpi_baseline.as_ref())
        .is_some_and(|baseline| !baseline.is_empty())
}

fn in_use() -> bool {
    *crate::lock_unpoisoned(&USERS) > 0
}

/// Record the scales unless display recovery is not set up, as in a probe;
/// the displays this put in the baseline.
fn record() -> Result<Vec<String>> {
    if PATH.get().is_none() {
        return Ok(Vec::new());
    }
    let scales = physical_scales()?;
    let mut recorded = Vec::new();
    super::change_journal(|journal| recorded = merge(journal, scales))?;
    Ok(recorded)
}

/// Add the scales read now to the baseline. A display already in it keeps its
/// recorded scale: that one is from before an earlier virtual display, which
/// has not been put back yet. A display whose scale a stream leases is that
/// stream's to restore and stays out. Returns the displays it put in.
pub(super) fn merge(journal: &mut Journal, scales: BTreeMap<String, u32>) -> Vec<String> {
    let leased: Vec<String> = journal
        .entries
        .iter()
        .filter(|(_, entry)| entry.scale.is_some())
        .map(|(id, _)| id.clone())
        .collect();
    let entry = journal.entries.entry(ID.into()).or_default();
    entry.stale = false;
    let baseline = entry.dpi_baseline.get_or_insert_with(BTreeMap::new);
    let mut added = Vec::new();
    for (id, percent) in scales {
        if !leased.contains(&id) && !baseline.contains_key(&id) {
            baseline.insert(id.clone(), percent);
            added.push(id);
        }
    }
    if baseline.is_empty() {
        entry.dpi_baseline = None;
    }
    added
}

/// Take a display out of the baseline.
pub(super) fn forget(journal: &mut Journal, id: &str) {
    if let Some(entry) = journal.entries.get_mut(ID)
        && let Some(baseline) = &mut entry.dpi_baseline
    {
        baseline.remove(id);
        if baseline.is_empty() {
            entry.dpi_baseline = None;
        }
    }
}

/// The last virtual display is gone: put the recorded scales back. A new
/// virtual display that arrives meanwhile takes the baseline over, and a
/// display a stream leases meanwhile is that stream's. A scale changed by hand
/// during the stream is put back too: the host cannot tell it from one
/// Windows moved, which is what this restore is for.
fn restore() -> Result<()> {
    let _serial = crate::lock_unpoisoned(&RESTORE);
    let Some(path) = PATH.get().filter(|p| p.exists()) else {
        return Ok(());
    };
    let baseline = {
        let _guard = lock(path)?;
        read_journal(path)?
            .entries
            .get(ID)
            .and_then(|entry| entry.dpi_baseline.clone())
            .unwrap_or_default()
    };
    if baseline.is_empty() {
        return Ok(());
    }
    let generation = GENERATION.load(Ordering::Acquire);
    let outcome = crate::desktop::on_input_desktop(|| {
        wait_for_virtual_displays(LEAVE_AFTER_REMOVAL, in_use);
        settle(&baseline, |id, set| {
            while_journal(|journal| !in_use() && in_baseline(journal, id), set)
        })
    });
    let mut drifted = Vec::new();
    super::change_journal(|journal| {
        // A virtual display that came (and maybe went) meanwhile holds the
        // baseline now; its own end restores and checks again.
        if in_use() || GENERATION.load(Ordering::Acquire) != generation {
            return;
        }
        // A display a stream leased meanwhile left the baseline: not a drift.
        drifted = outcome
            .drifted
            .iter()
            .filter(|id| in_baseline(journal, id))
            .cloned()
            .collect();
        if let Some(entry) = journal.entries.get_mut(ID) {
            entry.stale = false;
            entry.dpi_baseline = None;
        }
    })?;
    if drifted.is_empty() {
        Ok(())
    } else {
        anyhow::bail!("the Windows scale did not stay on {}", drifted.join(", "))
    }
}

fn in_baseline(journal: &Journal, id: &str) -> bool {
    journal
        .entries
        .get(ID)
        .and_then(|entry| entry.dpi_baseline.as_ref())
        .is_some_and(|baseline| baseline.contains_key(id))
}

/// Run `set` under the journal lock when `wanted` still holds for the journal
/// then: a stream that leases the display's scale, or a new virtual display,
/// cannot slip in between the check and the set.
fn while_journal(
    wanted: impl FnOnce(&Journal) -> bool,
    set: &mut dyn FnMut() -> Result<()>,
) -> Result<()> {
    let Some(path) = PATH.get() else {
        return set();
    };
    let _guard = lock(path)?;
    let journal = if path.exists() {
        read_journal(path)?
    } else {
        Journal::default()
    };
    if wanted(&journal) { set() } else { Ok(()) }
}

/// Wait, up to `bound`, until Windows lists no Rubylight virtual display, or
/// `abort` says to stop. Windows can move a scale again as one leaves.
pub(super) fn wait_for_virtual_displays(bound: Duration, abort: impl Fn() -> bool) {
    let started = Instant::now();
    while started.elapsed() < bound && !abort() {
        let listed = crate::display::monitors()
            .map(|all| all.iter().any(|m| m.is_rubylight()))
            .unwrap_or(false);
        if !listed {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    if !abort() {
        tracing::info!(
            waited_ms = started.elapsed().as_millis() as u64,
            "a virtual display is still listed; checking the scales anyway"
        );
    }
}

/// [`dpi_settle::settle`] on the connected physical displays, logged. `check`
/// runs each set it lets through.
pub(super) fn settle(
    expected: &BTreeMap<String, u32>,
    check: impl Fn(&str, &mut dyn FnMut() -> Result<()>) -> Result<()>,
) -> dpi_settle::Settled {
    let outcome = dpi_settle::settle(
        expected,
        physical_scales,
        |id, percent| {
            check(id, &mut || {
                let monitors = crate::display::monitors()?;
                let monitor = monitors
                    .iter()
                    .find(|m| m.device_id == id)
                    .context("the display is gone")?;
                tracing::info!(output = %monitor.display_name, scale = percent, "putting a display's Windows scale back");
                crate::display::set_dpi_scale(monitor, percent)
            })
        },
        std::thread::sleep,
    );
    if outcome.reapplied > 0 || !outcome.settled {
        tracing::info!(
            dpi_settled_after_ms = outcome.after_ms,
            settled = outcome.settled,
            reapplied = outcome.reapplied,
            drifted = ?outcome.drifted,
            absent = ?outcome.absent,
            "physical displays' Windows scale checked after a restore"
        );
    } else {
        tracing::debug!(
            dpi_settled_after_ms = outcome.after_ms,
            "physical displays kept their Windows scale"
        );
    }
    outcome
}

/// Check that a display a stream scaled keeps the scale put back at its end,
/// and put it back again within the bound when Windows moved it; a stream
/// that leases the display's scale meanwhile has it.
pub fn settle_scale(id: &str, percent: u32) -> Result<()> {
    let outcome = settle(&BTreeMap::from([(id.to_owned(), percent)]), |id, set| {
        while_journal(
            |journal| journal.entries.get(id).is_none_or(|e| e.scale.is_none()),
            set,
        )
    });
    if outcome.drifted.is_empty() {
        Ok(())
    } else {
        anyhow::bail!("the Windows scale did not stay at {percent}%")
    }
}

/// The scale of each connected physical display, by device id.
fn physical_scales() -> Result<BTreeMap<String, u32>> {
    Ok(crate::display::monitors()?
        .iter()
        .filter(|m| m.is_known_physical())
        .filter_map(|m| {
            crate::display::dpi_scale(m)
                .ok()
                .map(|percent| (m.device_id.clone(), percent))
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::super::Entry;
    use super::*;

    fn scales(list: &[(&str, u32)]) -> BTreeMap<String, u32> {
        list.iter().map(|(id, p)| ((*id).into(), *p)).collect()
    }

    #[test]
    fn the_baseline_records_each_physical_display_once_and_keeps_the_oldest() {
        let mut journal = Journal::default();
        merge(&mut journal, scales(&[("primary", 150), ("side", 100)]));
        // A second virtual display before the first was put back: Windows has
        // moved the primary meanwhile, and the scale from before stays.
        merge(&mut journal, scales(&[("primary", 100), ("tv", 200)]));
        assert_eq!(
            journal.entries[ID].dpi_baseline,
            Some(scales(&[("primary", 150), ("side", 100), ("tv", 200)]))
        );
        assert!(journal.entries[ID].pending());
    }

    #[test]
    fn a_display_whose_scale_a_stream_leases_stays_out_of_the_baseline() {
        let mut journal = Journal::default();
        journal.entries.insert(
            "side".into(),
            Entry {
                scale: Some((100, 125)),
                ..Default::default()
            },
        );
        merge(&mut journal, scales(&[("primary", 150), ("side", 125)]));
        assert_eq!(
            journal.entries[ID].dpi_baseline,
            Some(scales(&[("primary", 150)]))
        );
        // A lease taken later takes the display out as well.
        forget(&mut journal, "primary");
        assert_eq!(journal.entries[ID].dpi_baseline, None);
        assert!(!journal.entries[ID].pending());
    }

    #[test]
    fn a_failed_creation_never_restores_under_the_display_leases_lock() {
        // The display ended after it was created, outside that lock.
        let last = |stage, leasing| last_drop(stage, leasing, || unreachable!());
        assert_eq!(last(Stage::Created, false), Restore::InPlace);
        // A lease that dropped the last stream's display under the lock.
        assert_eq!(last(Stage::Created, true), Restore::Aside);
        // Windows had the display, which may have moved a scale.
        assert_eq!(last(Stage::Added, true), Restore::Aside);
        // Windows never had it: nothing to put back unless an earlier
        // virtual display left a restore owed.
        assert_eq!(last_drop(Stage::Recorded, true, || false), Restore::No);
        assert_eq!(last_drop(Stage::Recorded, true, || true), Restore::Aside);
    }

    #[test]
    fn leasing_holds_only_while_the_leases_lock_is_held() {
        assert!(!LEASING.get());
        let inside = while_leasing(|| while_leasing(|| LEASING.get()) && LEASING.get());
        assert!(inside);
        assert!(!LEASING.get());
        let unwound = std::panic::catch_unwind(|| while_leasing::<()>(|| panic!("lease")));
        assert!(unwound.is_err());
        assert!(!LEASING.get());
    }

    #[test]
    fn a_failed_creation_takes_out_only_what_it_recorded() {
        let mut journal = Journal::default();
        assert_eq!(
            merge(&mut journal, scales(&[("primary", 150)])),
            vec!["primary".to_owned()]
        );
        assert!(!abandoned(&mut journal, &["primary".into()], true));
        assert_eq!(journal.entries[ID].dpi_baseline, None);

        // An earlier virtual display's scale is still owed: it stays, and a
        // restore runs for it.
        let mut journal = Journal::default();
        merge(&mut journal, scales(&[("primary", 150)]));
        let recorded = merge(&mut journal, scales(&[("primary", 100), ("tv", 200)]));
        assert_eq!(recorded, vec!["tv".to_owned()]);
        assert!(abandoned(&mut journal, &recorded, true));
        assert_eq!(
            journal.entries[ID].dpi_baseline,
            Some(scales(&[("primary", 150)]))
        );

        // A virtual display counted after the failed one may rely on its
        // record: everything stays.
        let mut journal = Journal::default();
        let recorded = merge(&mut journal, scales(&[("primary", 150)]));
        assert!(abandoned(&mut journal, &recorded, false));
        assert_eq!(
            journal.entries[ID].dpi_baseline,
            Some(scales(&[("primary", 150)]))
        );
    }

    #[test]
    fn a_journal_from_an_older_host_reads_without_the_new_fields() {
        let old = r#"{"pid":4,"started":9,"entries":{"primary":{"output":"DISPLAY1","mode":null,"hdr":null,"scale":[150,100]}}}"#;
        let journal: Journal = serde_json::from_str(old).unwrap();
        let entry = journal.entries.values().next().unwrap();
        assert_eq!(entry.scale, Some((150, 100)));
        assert!(!entry.scale_requested);
        assert_eq!(entry.dpi_baseline, None);
        // And the new fields survive a round trip; the display a stream
        // leases stays out of the baseline.
        let mut journal = journal;
        merge(&mut journal, scales(&[("primary", 100), ("side", 125)]));
        let text = serde_json::to_string(&journal).unwrap();
        let back: Journal = serde_json::from_str(&text).unwrap();
        assert_eq!(
            back.entries[ID].dpi_baseline,
            Some(scales(&[("side", 125)]))
        );
    }
}
