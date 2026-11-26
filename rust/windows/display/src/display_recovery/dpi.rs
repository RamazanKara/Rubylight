//! The Windows scale of every physical display from before a virtual display
//! joined the desktop, kept in the recovery journal and put back once the last
//! virtual display is gone, or by recovery after a crash (issue #129). Windows
//! can give a physical display another scale while a virtual one comes or
//! goes; [`super::dpi_settle`] checks that the restored scale stays.

use super::{Journal, PATH, dpi_settle, lock, read_journal};
use anyhow::{Context, Result};
use std::{collections::BTreeMap, sync::Mutex};

/// The journal entry that holds the baseline.
pub(super) const ID: &str = "physical-dpi-baseline";

/// How many virtual displays hold the baseline now.
static USERS: Mutex<usize> = Mutex::new(0);

/// Held by every virtual display for its lifetime. The first one records each
/// physical display's scale before the virtual display is added; when the last
/// one is dropped, after its display was removed, the scales are put back and
/// checked. Nothing changes for a display whose scale stayed.
pub struct DpiBaseline(());

impl DpiBaseline {
    pub fn acquire() -> Self {
        let mut users = crate::lock_unpoisoned(&USERS);
        if *users == 0
            && let Err(error) = record()
        {
            tracing::warn!(error = %format!("{error:#}"), "could not record the physical displays' Windows scale before adding a virtual display");
        }
        *users += 1;
        Self(())
    }
}

impl Drop for DpiBaseline {
    fn drop(&mut self) {
        let mut users = crate::lock_unpoisoned(&USERS);
        *users = users.saturating_sub(1);
        if *users == 0
            && let Err(error) = restore()
        {
            tracing::warn!(error = %format!("{error:#}"), "could not put the physical displays' Windows scale back");
        }
    }
}

/// Record the scales unless display recovery is not set up, as in a probe.
fn record() -> Result<()> {
    if PATH.get().is_none() {
        return Ok(());
    }
    let scales = physical_scales()?;
    super::change_journal(|journal| merge(journal, scales))
}

/// Add the scales read now to the baseline. A display already in it keeps its
/// recorded scale: that one is from before an earlier virtual display, which
/// has not been put back yet. A display whose scale a stream leases is that
/// stream's to restore and stays out.
pub(super) fn merge(journal: &mut Journal, scales: BTreeMap<String, u32>) {
    let leased: Vec<String> = journal
        .entries
        .iter()
        .filter(|(_, entry)| entry.scale.is_some())
        .map(|(id, _)| id.clone())
        .collect();
    let entry = journal.entries.entry(ID.into()).or_default();
    entry.stale = false;
    let baseline = entry.dpi_baseline.get_or_insert_with(BTreeMap::new);
    for (id, percent) in scales {
        if !leased.contains(&id) {
            baseline.entry(id).or_insert(percent);
        }
    }
    if baseline.is_empty() {
        entry.dpi_baseline = None;
    }
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

/// What stays in the baseline after a settle: the displays still at another
/// scale. A display that is not connected is dropped: Windows keeps its own
/// scale for it, and a later restore could undo one the user chose meanwhile.
pub(super) fn keep_drifted(
    baseline: &mut Option<BTreeMap<String, u32>>,
    outcome: &dpi_settle::Settled,
) {
    if let Some(scales) = baseline {
        scales.retain(|id, _| outcome.drifted.contains(id));
        if scales.is_empty() {
            *baseline = None;
        }
    }
}

/// The last virtual display is gone: put the recorded scales back.
fn restore() -> Result<()> {
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
    let outcome = settle(&baseline);
    super::change_journal(|journal| {
        if let Some(entry) = journal.entries.get_mut(ID) {
            entry.stale = false;
            keep_drifted(&mut entry.dpi_baseline, &outcome);
        }
    })?;
    if outcome.drifted.is_empty() {
        Ok(())
    } else {
        anyhow::bail!(
            "the Windows scale did not stay on {}",
            outcome.drifted.join(", ")
        )
    }
}

/// [`dpi_settle::settle`] on the connected physical displays, logged.
pub(super) fn settle(expected: &BTreeMap<String, u32>) -> dpi_settle::Settled {
    let outcome = dpi_settle::settle(
        expected,
        physical_scales,
        |id, percent| {
            let monitors = crate::display::monitors()?;
            let monitor = monitors
                .iter()
                .find(|m| m.device_id == id)
                .context("the display is gone")?;
            tracing::info!(output = %monitor.display_name, scale = percent, "putting a display's Windows scale back");
            crate::display::set_dpi_scale(monitor, percent)
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
/// and put it back again within the bound when Windows moved it.
pub fn settle_scale(id: &str, percent: u32) -> Result<()> {
    let outcome = settle(&BTreeMap::from([(id.to_owned(), percent)]));
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
    fn only_displays_still_at_another_scale_stay_after_a_settle() {
        let mut baseline = Some(scales(&[("primary", 150), ("side", 100), ("tv", 200)]));
        let outcome = dpi_settle::Settled {
            settled: false,
            absent: vec!["tv".into()],
            drifted: vec!["side".into()],
            ..Default::default()
        };
        keep_drifted(&mut baseline, &outcome);
        assert_eq!(baseline, Some(scales(&[("side", 100)])));
        keep_drifted(&mut baseline, &dpi_settle::Settled::default());
        assert_eq!(baseline, None);
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
