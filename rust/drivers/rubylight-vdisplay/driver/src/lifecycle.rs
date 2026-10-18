//! Adapter lifetime follows PnP start/stop, not D0/Dx power transitions.
//!
//! Kept free of WDK calls so ordinary CI exercises the actual lifecycle policy.

use std::vec::Vec;

#[derive(Default)]
pub struct Lifecycle {
    pub device: usize,
    pub powered: bool,
    adapter: Option<usize>,
    sequence: u64,
    pending: Option<Pending>,
}

struct Pending {
    sequence: u64,
    handle: Option<usize>,
    early: Vec<(usize, bool)>,
}

impl Lifecycle {
    pub const fn new() -> Self {
        Self {
            device: 0,
            powered: false,
            adapter: None,
            sequence: 0,
            pending: None,
        }
    }

    pub fn enter(&mut self, device: usize) {
        self.device = device;
        self.powered = true;
    }

    pub fn suspend(&mut self) {
        self.powered = false;
    }

    pub fn begin_init(&mut self) -> Option<u64> {
        if !self.powered || self.device == 0 || self.adapter.is_some() || self.pending.is_some() {
            return None;
        }
        self.sequence = self.sequence.wrapping_add(1);
        self.pending = Some(Pending {
            sequence: self.sequence,
            handle: None,
            early: Vec::new(),
        });
        Some(self.sequence)
    }

    /// Match the returned object to its eventual callback. A callback may race the
    /// initiating DDI's return, so retain early completions until that handle is known.
    pub fn init_returned(&mut self, sequence: u64, handle: Option<usize>) {
        let Some(pending) = self.pending.as_mut().filter(|p| p.sequence == sequence) else {
            return;
        };
        let Some(handle) = handle else {
            self.pending = None;
            return;
        };
        pending.handle = Some(handle);
        if let Some((_, succeeded)) = pending.early.iter().find(|(a, _)| *a == handle).copied() {
            self.finish_init(handle, succeeded);
        } else {
            pending.early.clear();
        }
    }

    /// A completion can land during suspend. Retain its adapter, but do not expose it
    /// to control requests until D0. Ignore callbacks after the UMDF device stopped.
    pub fn finish_init(&mut self, adapter: usize, succeeded: bool) {
        if let Some(pending) = self.pending.as_mut() {
            match pending.handle {
                None => pending.early.push((adapter, succeeded)),
                Some(handle) if handle == adapter => {
                    self.pending = None;
                    self.adapter = succeeded.then_some(adapter);
                }
                Some(_) => {} // Completion of an adapter from an earlier PnP start.
            }
        }
    }

    pub fn adapter(&self) -> Option<usize> {
        self.powered.then_some(self.adapter).flatten()
    }

    pub fn stop(&mut self) {
        *self = Self {
            sequence: self.sequence,
            ..Self::new()
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sleep_preserves_the_adapter_but_stop_requires_a_new_one() {
        let mut state = Lifecycle::new();
        state.enter(1);
        let init = state.begin_init().unwrap();
        state.init_returned(init, Some(42));
        state.finish_init(42, true);
        for _ in 0..3 {
            state.suspend();
            assert_eq!(state.adapter(), None);
            assert!(state.begin_init().is_none());
            state.enter(1);
            assert_eq!(state.adapter(), Some(42));
            assert!(state.begin_init().is_none());
        }
        state.stop();
        assert_eq!(state.device, 0);
        assert_eq!(state.adapter(), None);
        state.enter(1);
        let init = state.begin_init().unwrap();
        state.init_returned(init, Some(43));
        state.finish_init(43, true);
        assert_eq!(state.adapter(), Some(43));
    }

    #[test]
    fn concurrent_retry_and_suspend_do_not_duplicate_pending_initialization() {
        let mut state = Lifecycle::new();
        state.enter(1);
        let init = state.begin_init().unwrap();
        assert!(state.begin_init().is_none());
        state.suspend();
        state.finish_init(42, true);
        state.init_returned(init, Some(42));
        assert_eq!(state.adapter(), None);
        state.enter(1);
        assert!(state.begin_init().is_none());
        assert_eq!(state.adapter(), Some(42));
    }

    #[test]
    fn failed_initialization_retries_only_while_powered() {
        let mut state = Lifecycle::new();
        state.enter(1);
        let init = state.begin_init().unwrap();
        state.init_returned(init, Some(42));
        state.finish_init(42, false);
        state.suspend();
        assert!(state.begin_init().is_none());
        state.enter(1);
        assert!(state.begin_init().is_some());
        state.stop();
        state.finish_init(42, true);
        assert_eq!(state.adapter(), None);
    }

    #[test]
    fn a_late_completion_cannot_replace_the_new_starts_adapter() {
        let mut state = Lifecycle::new();
        state.enter(1);
        let old = state.begin_init().unwrap();
        state.stop();
        state.enter(1);
        let new = state.begin_init().unwrap();
        state.init_returned(old, Some(42));
        state.finish_init(42, true);
        state.init_returned(new, Some(43));
        assert_eq!(state.adapter(), None);
        state.finish_init(42, true);
        assert_eq!(state.adapter(), None);
        state.finish_init(43, true);
        assert_eq!(state.adapter(), Some(43));
    }
}
