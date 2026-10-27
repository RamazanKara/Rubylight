//! The device's keyboard layout on the control loop (`butterpollo_core::keyboard_layout`,
//! 0x553E): reports and saves of `keyboard_layout` pick the layout here, off the input
//! path, and the injector takes the read layout between passes. Each layout id is read at
//! most once a peer, however often a device reports it.
use std::time::{Duration, Instant};

use anyhow::Result;
use butterpollo_core::config::{Config, KeyboardLayoutSetting};
use butterpollo_core::session::Warnings;
use butterpollo_windows::keylayout::{Layout, Layouts};

/// The session warning code while the layout keys should follow cannot be read.
pub(super) const WARNING: &str = "keyboard_layout";
/// Reports closer than this are coalesced: the newest is applied once it passes.
const REPORT_INTERVAL: Duration = Duration::from_millis(125);

/// One control peer's keyboard layout and the configuration version it follows.
#[derive(Default)]
pub(super) struct PeerLayout {
    version: Option<u64>,
    /// `keyboard_layout`, once read.
    setting: Option<KeyboardLayoutSetting>,
    /// The layout id the device reported, resolved on arrival.
    reported: Option<u32>,
    /// The layout id keys follow (None: the host's), once picked.
    klid: Option<Option<u32>>,
    layouts: Layouts,
    layout: Option<Layout>,
    /// The layout changed since the injector took it.
    fresh: bool,
    /// When the next report may be applied, and whether one waits for it.
    next_report: Option<Instant>,
    deferred: bool,
}

impl PeerLayout {
    #[cfg(test)]
    fn with_layouts(layouts: Layouts) -> Self {
        Self {
            layouts,
            ..Default::default()
        }
    }

    /// Each pass: follows saves of `keyboard_layout`, and the device's report from before
    /// this peer (`reported`, read with the configuration), and applies a coalesced report.
    pub(super) fn follow(
        &mut self,
        version: u64,
        warnings: &Warnings,
        now: Instant,
        reported: impl FnOnce() -> Option<u32>,
        config: impl FnOnce() -> Result<Config>,
    ) {
        let mut changed = false;
        if self.deferred && self.next_report.is_none_or(|next| now >= next) {
            self.deferred = false;
            self.next_report = Some(now + REPORT_INTERVAL);
            changed = true;
        }
        if self.version != Some(version) {
            self.version = Some(version);
            match config() {
                Ok(config) => self.setting = Some(config.keyboard_layout()),
                // The last setting stays; input reports the configuration itself.
                Err(error) => {
                    tracing::debug!(error = %format!("{error:#}"), "keyboard layout setting unavailable")
                }
            }
            if self.klid.is_none() {
                self.reported = reported();
            }
            changed = true;
        }
        if changed {
            self.apply(warnings);
        }
    }

    /// The device reported the layout id `klid` (None: one the host cannot name). Applied
    /// at once unless one came within [`REPORT_INTERVAL`]; then the newest is applied when
    /// it passes, so a flood of reports reads nothing more.
    pub(super) fn report(&mut self, klid: Option<u32>, warnings: &Warnings, now: Instant) {
        self.reported = klid;
        if self.next_report.is_some_and(|next| now < next) {
            self.deferred = true;
            return;
        }
        self.deferred = false;
        self.next_report = Some(now + REPORT_INTERVAL);
        self.apply(warnings);
    }

    /// The layout keys follow when it changed since the last call: `Some(None)` for the
    /// host's own.
    pub(super) fn take(&mut self) -> Option<Option<Layout>> {
        std::mem::take(&mut self.fresh).then(|| self.layout.clone())
    }

    /// A new injector takes the layout at its first pass.
    pub(super) fn refresh(&mut self) {
        self.fresh = true;
    }

    fn apply(&mut self, warnings: &Warnings) {
        let Some(setting) = self.setting else {
            return;
        };
        let klid = butterpollo_core::keyboard_layout::effective(setting, self.reported);
        if self.klid == Some(klid) {
            return;
        }
        self.klid = Some(klid);
        let read = klid.map(|klid| self.layouts.get(klid));
        self.layout = match read {
            None => {
                warnings.clear(WARNING);
                None
            }
            Some((Ok(layout), _)) => {
                warnings.clear(WARNING);
                Some(layout)
            }
            Some((Err(error), first)) => {
                // Once a layout id; the warning stays while keys would follow it.
                if first {
                    tracing::warn!(%error, "keyboard layout unavailable");
                }
                warnings.set(
                    WARNING,
                    format!(
                        "The device's keyboard layout could not be read ({error}); keys follow this PC's layout. Set Keyboard layout under Input to a layout id this PC has, or to us."
                    ),
                );
                None
            }
        };
        self.fresh = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const BAD: u32 = 0x0bad_0407;

    fn config(text: &'static str) -> impl FnOnce() -> Result<Config> {
        move || Config::parse(text)
    }

    fn klid(layout: Option<Option<Layout>>) -> Option<Option<u32>> {
        layout.map(|layout| layout.map(|layout| layout.klid()))
    }

    fn warned(warnings: &Warnings) -> bool {
        warnings.snapshot().iter().any(|w| w.code == WARNING)
    }

    /// Each layout id is read once a peer, a failed one too, and a failure warns until keys
    /// follow a layout again.
    #[test]
    fn each_layout_is_read_once_and_a_failure_warns() {
        static LOADS: AtomicUsize = AtomicUsize::new(0);
        fn load(klid: u32) -> Result<Layout> {
            LOADS.fetch_add(1, Ordering::SeqCst);
            Layout::load(klid)
        }
        let warnings = Warnings::default();
        let mut peer = PeerLayout::with_layouts(Layouts::with_loader(load));
        let mut now = Instant::now();
        peer.follow(1, &warnings, now, || None, config(""));
        // Client is the default; nothing reported yet keeps the host's layout.
        assert_eq!(klid(peer.take()), Some(None));
        for (reported, expected) in [
            (0x0407, Some(0x0407)),
            (0x040c, Some(0x040c)),
            (0x0407, Some(0x0407)),
            (BAD, None),
            (0x040c, Some(0x040c)),
            (BAD, None),
        ] {
            now += REPORT_INTERVAL;
            peer.report(Some(reported), &warnings, now);
            assert_eq!(klid(peer.take()), Some(expected), "{reported:08X}");
            assert_eq!(warned(&warnings), expected.is_none(), "{reported:08X}");
        }
        assert_eq!(LOADS.load(Ordering::SeqCst), 3);
        // The same report again changes nothing.
        now += REPORT_INTERVAL;
        peer.report(Some(BAD), &warnings, now);
        assert_eq!(klid(peer.take()), None);
    }

    /// A flood of reports applies the first at once and then the newest every interval.
    #[test]
    fn reports_closer_than_the_interval_are_coalesced() {
        let warnings = Warnings::default();
        let mut peer = PeerLayout::default();
        let start = Instant::now();
        peer.follow(1, &warnings, start, || None, config(""));
        peer.take();
        peer.report(Some(0x0407), &warnings, start);
        assert_eq!(klid(peer.take()), Some(Some(0x0407)));
        for step in 1..10 {
            let klid = if step % 2 == 0 { 0x0407 } else { 0x0809 };
            peer.report(Some(klid), &warnings, start + Duration::from_millis(step));
        }
        peer.report(Some(0x040c), &warnings, start + Duration::from_millis(10));
        peer.follow(
            1,
            &warnings,
            start + Duration::from_millis(50),
            || None,
            config(""),
        );
        assert_eq!(klid(peer.take()), None);
        peer.follow(1, &warnings, start + REPORT_INTERVAL, || None, config(""));
        assert_eq!(klid(peer.take()), Some(Some(0x040c)));
    }

    /// A save of `keyboard_layout` mid-stream takes over at the next pass; the configuration
    /// is read only when its version changes.
    #[test]
    fn the_setting_follows_saves() {
        let warnings = Warnings::default();
        let mut peer = PeerLayout::default();
        let now = Instant::now();
        // The device's report from before this peer counts.
        peer.follow(1, &warnings, now, || Some(0x0407), config(""));
        assert_eq!(klid(peer.take()), Some(Some(0x0407)));
        peer.follow(
            2,
            &warnings,
            now,
            || panic!("read again"),
            config("keyboard_layout = us\n"),
        );
        assert_eq!(klid(peer.take()), Some(Some(0x0409)));
        peer.follow(
            2,
            &warnings,
            now,
            || panic!("read again"),
            || panic!("read again"),
        );
        assert_eq!(klid(peer.take()), None);
        peer.follow(
            3,
            &warnings,
            now,
            || panic!("read again"),
            config("keyboard_layout = 00000809\n"),
        );
        assert_eq!(klid(peer.take()), Some(Some(0x0809)));
        // Back to the device's layout, which it reported before.
        peer.follow(4, &warnings, now, || panic!("read again"), config(""));
        assert_eq!(klid(peer.take()), Some(Some(0x0407)));
        // A new injector takes the layout again.
        peer.refresh();
        assert_eq!(klid(peer.take()), Some(Some(0x0407)));
    }
}
