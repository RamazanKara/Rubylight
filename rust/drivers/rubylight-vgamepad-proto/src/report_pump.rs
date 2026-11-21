// Copyright (c) 2026 Chase Payne
// SPDX-License-Identifier: MIT
//
// Ported from Nonary/libvirtualgamepad driver/src/report_pump.h and report_pump.cpp.
// Adapted for Rubylight by RamazanKara (https://github.com/RamazanKara/Rubylight).

//! Input report pacing queue for VHF custom buffering.

pub const MAX_REPORT_BYTES: usize = 64;
pub const TRANSITION_CAPACITY: usize = 16;
pub const PRIORITY_CAPACITY: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReportKind {
    Continuous,
    Transition,
    Priority,
    /// Ordered with transitions without discarding a waiting continuous snapshot.
    Aside,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReportBuffer {
    pub data: [u8; MAX_REPORT_BYTES],
    pub length: u32,
    pub report_id: u8,
}

impl Default for ReportBuffer {
    fn default() -> Self {
        Self {
            data: [0; MAX_REPORT_BYTES],
            length: 0,
            report_id: 0,
        }
    }
}

impl ReportBuffer {
    pub fn store(&mut self, bytes: &[u8], report_id: u8) {
        let copied = if bytes.len() > MAX_REPORT_BYTES {
            MAX_REPORT_BYTES
        } else {
            bytes.len()
        };
        self.data[..copied].copy_from_slice(&bytes[..copied]);
        self.length = copied as u32;
        self.report_id = report_id;
    }
}

/// Three-tier input pacing pump (`priority > transition/aside > latest continuous`).
#[derive(Clone, Debug)]
pub struct ReportPump {
    ready: bool,
    /// Retained by VHF until the next `EvtVhfReadyForNextReadReport` callback.
    pub in_flight: ReportBuffer,
    priority: [ReportBuffer; PRIORITY_CAPACITY],
    priority_count: u8,
    transitions: [ReportBuffer; TRANSITION_CAPACITY],
    transition_aside: [bool; TRANSITION_CAPACITY],
    transition_head: u8,
    transition_count: u8,
    latest: ReportBuffer,
    have_latest: bool,
    dropped: u32,
    have_signature: bool,
    last_buttons: u32,
    last_left_trigger: bool,
    last_right_trigger: bool,
}

impl Default for ReportPump {
    fn default() -> Self {
        Self::new()
    }
}

impl ReportPump {
    pub const fn new() -> Self {
        const EMPTY_BUF: ReportBuffer = ReportBuffer {
            data: [0; MAX_REPORT_BYTES],
            length: 0,
            report_id: 0,
        };
        Self {
            ready: false,
            in_flight: EMPTY_BUF,
            priority: [EMPTY_BUF; PRIORITY_CAPACITY],
            priority_count: 0,
            transitions: [EMPTY_BUF; TRANSITION_CAPACITY],
            transition_aside: [false; TRANSITION_CAPACITY],
            transition_head: 0,
            transition_count: 0,
            latest: EMPTY_BUF,
            have_latest: false,
            dropped: 0,
            have_signature: false,
            last_buttons: 0,
            last_left_trigger: false,
            last_right_trigger: false,
        }
    }

    pub fn reset(&mut self) {
        self.ready = false;
        self.priority_count = 0;
        self.transition_head = 0;
        self.transition_count = 0;
        self.transition_aside = [false; TRANSITION_CAPACITY];
        self.have_latest = false;
        self.dropped = 0;
        self.have_signature = false;
        self.last_buttons = 0;
        self.last_left_trigger = false;
        self.last_right_trigger = false;
    }

    pub fn classify(&mut self, buttons: u32, left_trigger: u8, right_trigger: u8) -> ReportKind {
        let left_pressed = left_trigger > 0;
        let right_pressed = right_trigger > 0;
        let discrete_change = !self.have_signature
            || buttons != self.last_buttons
            || left_pressed != self.last_left_trigger
            || right_pressed != self.last_right_trigger;
        self.have_signature = true;
        self.last_buttons = buttons;
        self.last_left_trigger = left_pressed;
        self.last_right_trigger = right_pressed;
        if discrete_change {
            ReportKind::Transition
        } else {
            ReportKind::Continuous
        }
    }

    /// Enqueues a report. Returns `false` if an entry (either oldest transition or new aside)
    /// had to be dropped because the ordered queue was full.
    pub fn enqueue(&mut self, data: &[u8], report_id: u8, kind: ReportKind) -> bool {
        if data.is_empty() {
            return true;
        }
        match kind {
            ReportKind::Priority => {
                if usize::from(self.priority_count) >= PRIORITY_CAPACITY {
                    return false;
                }
                self.priority[usize::from(self.priority_count)].store(data, report_id);
                self.priority_count += 1;
                true
            }
            ReportKind::Transition | ReportKind::Aside => {
                let is_aside = kind == ReportKind::Aside;
                if !is_aside {
                    self.have_latest = false;
                }
                let mut dropped_now = false;
                if usize::from(self.transition_count) == TRANSITION_CAPACITY {
                    if is_aside {
                        return false;
                    }
                    let victim_aside = self.transition_aside[usize::from(self.transition_head)];
                    self.transition_head =
                        ((usize::from(self.transition_head) + 1) % TRANSITION_CAPACITY) as u8;
                    self.transition_count -= 1;
                    if !victim_aside {
                        self.dropped += 1;
                    }
                    dropped_now = true;
                }
                let tail = (usize::from(self.transition_head) + usize::from(self.transition_count))
                    % TRANSITION_CAPACITY;
                self.transitions[tail].store(data, report_id);
                self.transition_aside[tail] = is_aside;
                self.transition_count += 1;
                !dropped_now
            }
            ReportKind::Continuous => {
                self.latest.store(data, report_id);
                self.have_latest = true;
                true
            }
        }
    }

    pub fn set_ready(&mut self) {
        self.ready = true;
    }

    /// Populates `self.in_flight` and consumes readiness if a queued report is available.
    pub fn take(&mut self) -> Option<&mut ReportBuffer> {
        if !self.ready {
            return None;
        }
        if self.priority_count > 0 {
            self.in_flight = self.priority[0];
            for i in 1..usize::from(self.priority_count) {
                self.priority[i - 1] = self.priority[i];
            }
            self.priority_count -= 1;
            self.ready = false;
            return Some(&mut self.in_flight);
        }
        if self.transition_count > 0 {
            let head = usize::from(self.transition_head);
            self.in_flight = self.transitions[head];
            self.transition_head = ((head + 1) % TRANSITION_CAPACITY) as u8;
            self.transition_count -= 1;
            self.ready = false;
            return Some(&mut self.in_flight);
        }
        if self.have_latest {
            self.in_flight = self.latest;
            self.have_latest = false;
            self.ready = false;
            return Some(&mut self.in_flight);
        }
        None
    }

    pub const fn ready(&self) -> bool {
        self.ready
    }

    pub const fn empty(&self) -> bool {
        self.priority_count == 0 && self.transition_count == 0 && !self.have_latest
    }

    pub const fn transitions_waiting(&self) -> u8 {
        self.transition_count
    }

    pub const fn dropped_transitions(&self) -> u32 {
        self.dropped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pump_priority_transition_aside_and_drop_accounting() {
        let mut pump = ReportPump::new();
        assert!(pump.take().is_none());

        assert!(pump.enqueue(&[1, 1], 1, ReportKind::Continuous));
        assert!(pump.enqueue(&[2, 2], 1, ReportKind::Transition));
        // Transition discards stale continuous snapshot.
        assert!(pump.enqueue(&[3, 3], 1, ReportKind::Continuous));
        // Aside preserves waiting continuous snapshot.
        assert!(pump.enqueue(&[4, 4], 0x43, ReportKind::Aside));
        assert!(pump.enqueue(&[5, 5], 0x21, ReportKind::Priority));

        pump.set_ready();
        let first = *pump.take().unwrap();
        assert_eq!(first.report_id, 0x21);
        assert_eq!(first.data[..2], [5, 5]);

        pump.set_ready();
        assert_eq!(pump.take().unwrap().data[..2], [2, 2]);

        pump.set_ready();
        let aside = *pump.take().unwrap();
        assert_eq!(aside.report_id, 0x43);

        pump.set_ready();
        assert_eq!(pump.take().unwrap().data[..2], [3, 3]);

        // Overflow transition ring with 1 aside at head: evicting aside doesn't count.
        assert!(pump.enqueue(&[9], 0x44, ReportKind::Aside));
        for i in 0..TRANSITION_CAPACITY {
            let expected = i < TRANSITION_CAPACITY - 1;
            assert_eq!(
                pump.enqueue(&[i as u8], 1, ReportKind::Transition),
                expected
            );
        }
        assert_eq!(pump.dropped_transitions(), 0);
        // Next transition evicts a real transition.
        assert!(!pump.enqueue(&[99], 1, ReportKind::Transition));
        assert_eq!(pump.dropped_transitions(), 1);
        // Full ring drops a new aside silently without incrementing dropped_transitions.
        assert!(!pump.enqueue(&[77], 0x43, ReportKind::Aside));
        assert_eq!(pump.dropped_transitions(), 1);
    }

    #[test]
    fn classify_detects_discrete_edges_only() {
        let mut pump = ReportPump::new();
        assert_eq!(pump.classify(0, 0, 0), ReportKind::Transition);
        assert_eq!(pump.classify(0, 0, 0), ReportKind::Continuous);
        assert_eq!(pump.classify(1, 0, 0), ReportKind::Transition);
        assert_eq!(pump.classify(1, 0, 0), ReportKind::Continuous);
        assert_eq!(pump.classify(1, 1, 0), ReportKind::Transition);
        assert_eq!(pump.classify(1, 200, 0), ReportKind::Continuous);
        assert_eq!(pump.classify(1, 0, 0), ReportKind::Transition);
    }
}
