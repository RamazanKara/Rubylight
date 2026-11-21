// Copyright (c) 2026 Chase Payne
// SPDX-License-Identifier: MIT
//
// Ported from Nonary/libvirtualgamepad driver/src/steam_haptic_queue.h.
// Adapted for Rubylight by RamazanKara (https://github.com/RamazanKara/Rubylight).

//! Bounded per-slot rings for Steam Controller haptic reports and settings writes.

use crate::protocol::{FeedbackEvent, FeedbackType, SteamHapticFeedback};

pub const HAPTIC_QUEUE_CAPACITY: usize = 8;
pub const SETTINGS_QUEUE_CAPACITY: usize = 8;
pub const SETTINGS_EVENTS_PER_WRITE: usize = 3;

/// Returns a non-zero stop key identifying report family + actuator mask if `report` stops haptics.
pub fn steam_haptic_stop_key(report: &[u8]) -> u32 {
    let len = report.len();
    if len >= 10
        && report[0] == 0x80
        && report[4] == 0
        && report[5] == 0
        && report[7] == 0
        && report[8] == 0
    {
        return (0x80u32 << 2) | 3;
    }
    if len < 2 || report[1] > 2 {
        return 0;
    }
    if (len >= 8
        && report[0] == 0x81
        && ((report[2] == 0 && report[3] == 0) || (report[6] == 0 && report[7] == 0)))
        || (len >= 4 && report[0] == 0x82 && report[2] == 0)
    {
        let side_mask = if report[1] == 2 { 3 } else { 1u32 << report[1] };
        return (u32::from(report[0]) << 2) | side_mask;
    }
    0
}

pub fn haptic_stop_key(event: &FeedbackEvent) -> u32 {
    if event.feedback_type != FeedbackType::SteamHaptic as u16
        || usize::from(event.payload_size) != SteamHapticFeedback::WIRE_SIZE
    {
        return 0;
    }
    let Some(haptic) = SteamHapticFeedback::from_bytes(&event.payload) else {
        return 0;
    };
    let len = usize::from(haptic.length).min(haptic.report.len());
    steam_haptic_stop_key(&haptic.report[..len])
}

/// 8-slot Steam haptic ring that preserves all 7 stop families under overflow.
#[derive(Clone, Debug)]
pub struct HapticRing {
    queue: [FeedbackEvent; HAPTIC_QUEUE_CAPACITY],
    head: u8,
    count: u8,
    evicted: u32,
}

impl Default for HapticRing {
    fn default() -> Self {
        Self::new()
    }
}

impl HapticRing {
    pub fn new() -> Self {
        Self {
            queue: [FeedbackEvent::default(); HAPTIC_QUEUE_CAPACITY],
            head: 0,
            count: 0,
            evicted: 0,
        }
    }

    pub fn clear(&mut self) {
        self.head = 0;
        self.count = 0;
    }

    pub fn reset(&mut self) {
        self.clear();
        self.evicted = 0;
    }

    pub fn enqueue(&mut self, event: FeedbackEvent) -> bool {
        let cap = HAPTIC_QUEUE_CAPACITY;
        let stop = haptic_stop_key(&event);
        let mut count = usize::from(self.count);
        let head = usize::from(self.head);
        let mut victim = count;

        if stop != 0 {
            for i in 0..count {
                if haptic_stop_key(&self.queue[(head + i) % cap]) == stop {
                    victim = i;
                    break;
                }
            }
        }
        if victim == count && count == cap {
            for i in 0..count {
                if haptic_stop_key(&self.queue[(head + i) % cap]) == 0 {
                    victim = i;
                    break;
                }
            }
            if victim == count {
                return false;
            }
            self.evicted += 1;
        }
        if victim < count {
            for i in victim..(count - 1) {
                self.queue[(head + i) % cap] = self.queue[(head + i + 1) % cap];
            }
            count -= 1;
        }
        self.queue[(head + count) % cap] = event;
        self.count = (count + 1) as u8;
        true
    }

    pub fn pop(&mut self) -> Option<FeedbackEvent> {
        if self.count == 0 {
            return None;
        }
        let head = usize::from(self.head);
        let ev = self.queue[head];
        self.head = ((head + 1) % HAPTIC_QUEUE_CAPACITY) as u8;
        self.count -= 1;
        Some(ev)
    }

    pub const fn evicted(&self) -> u32 {
        self.evicted
    }

    pub const fn len(&self) -> usize {
        self.count as usize
    }

    pub const fn is_empty(&self) -> bool {
        self.count == 0
    }
}

/// 8-slot Steam settings FIFO; oldest chunk is evicted and counted on overflow.
#[derive(Clone, Debug)]
pub struct SettingsRing {
    queue: [FeedbackEvent; SETTINGS_QUEUE_CAPACITY],
    head: u8,
    count: u8,
    evicted: u32,
}

impl Default for SettingsRing {
    fn default() -> Self {
        Self::new()
    }
}

impl SettingsRing {
    pub fn new() -> Self {
        Self {
            queue: [FeedbackEvent::default(); SETTINGS_QUEUE_CAPACITY],
            head: 0,
            count: 0,
            evicted: 0,
        }
    }

    pub fn clear(&mut self) {
        self.head = 0;
        self.count = 0;
    }

    pub fn reset(&mut self) {
        self.clear();
        self.evicted = 0;
    }

    pub fn enqueue(&mut self, event: FeedbackEvent) {
        let cap = SETTINGS_QUEUE_CAPACITY;
        let mut head = usize::from(self.head);
        let mut count = usize::from(self.count);
        if count == cap {
            head = (head + 1) % cap;
            count -= 1;
            self.evicted += 1;
        }
        self.queue[(head + count) % cap] = event;
        self.head = head as u8;
        self.count = (count + 1) as u8;
    }

    pub fn pop(&mut self) -> Option<FeedbackEvent> {
        if self.count == 0 {
            return None;
        }
        let head = usize::from(self.head);
        let ev = self.queue[head];
        self.head = ((head + 1) % SETTINGS_QUEUE_CAPACITY) as u8;
        self.count -= 1;
        Some(ev)
    }

    pub const fn evicted(&self) -> u32 {
        self.evicted
    }

    pub const fn is_empty(&self) -> bool {
        self.count == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::saturate_counter;

    fn make_event(kind: u8, side: u8, stop: bool) -> FeedbackEvent {
        let mut haptic = SteamHapticFeedback {
            length: 12,
            ..Default::default()
        };
        haptic.report[0] = kind;
        haptic.report[1] = side;
        if !stop {
            haptic.report[2] = 3;
            haptic.report[4] = 1;
            haptic.report[6] = 1;
            haptic.report[7] = 1;
        }
        FeedbackEvent::new(0, FeedbackType::SteamHaptic, &haptic.to_bytes())
    }

    #[test]
    fn stops_survive_ordinary_overflow_and_accounting_matches_upstream() {
        let mut ring = HapticRing::new();
        assert!(ring.enqueue(make_event(0x80, 0, true)));
        for kind in [0x81, 0x82] {
            for side in 0..3 {
                assert!(ring.enqueue(make_event(kind, side, true)));
            }
        }
        for n in 0..1000 {
            assert!(ring.enqueue(make_event(0x81, 0, false)));
            assert!(ring.len() <= HAPTIC_QUEUE_CAPACITY);
            if n % 10 == 0 {
                assert!(ring.enqueue(make_event(0x82, 1, true)));
                assert!(ring.len() <= HAPTIC_QUEUE_CAPACITY);
            }
        }
        let mut stop_keys = [0u32; 8];
        let mut unique = 0usize;
        while let Some(ev) = ring.pop() {
            let key = haptic_stop_key(&ev);
            if key != 0 && !stop_keys[..unique].contains(&key) {
                stop_keys[unique] = key;
                unique += 1;
            }
        }
        assert_eq!(unique, 7);

        ring.reset();
        ring.enqueue(make_event(0x82, 1, true));
        ring.enqueue(make_event(0x82, 1, false));
        ring.enqueue(make_event(0x82, 1, true));
        assert_eq!(ring.len(), 2);
        assert_eq!(haptic_stop_key(&ring.pop().unwrap()), 0);
        assert_ne!(haptic_stop_key(&ring.pop().unwrap()), 0);

        // Eviction accounting.
        ring.reset();
        for _ in 0..8 {
            assert!(ring.enqueue(make_event(0x81, 0, false)));
        }
        assert_eq!(ring.evicted(), 0);
        for _ in 0..5 {
            assert!(ring.enqueue(make_event(0x81, 0, false)));
        }
        assert_eq!(ring.evicted(), 5);
        assert!(ring.enqueue(make_event(0x82, 1, true)));
        assert!(ring.enqueue(make_event(0x82, 1, true))); // coalesces
        assert!(ring.enqueue(make_event(0x80, 0, true)));
        assert_eq!(ring.evicted(), 7);

        // Settings FIFO overflow.
        let mut settings = SettingsRing::new();
        for n in 0..9u8 {
            let mut ev = FeedbackEvent::new(0, FeedbackType::SteamSettings, &[n]);
            ev.payload[0] = n;
            settings.enqueue(ev);
        }
        assert_eq!(settings.evicted(), 1);
        assert_eq!(settings.pop().unwrap().payload[0], 1);
        assert_eq!(saturate_counter(0xFFFF_FFFF), 255);
    }
}
