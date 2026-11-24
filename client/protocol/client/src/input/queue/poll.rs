//! Input queue polling, motion batching, pen move collapsing and packet emission.

use std::time::Instant;

use super::{Entry, InputPacket, InputQueue, Pending, TEXT_SETTLE};
use crate::control::wire::Delivery;
use crate::input::packets::{motion_type, touch_event, InputEvent};

impl InputQueue {
    /// Takes what is ready to send into `out`. `contact_lost` is the driver's view that the
    /// host has been silent through a network grace period. Returns when to call again for
    /// input waiting on a batching interval or the text settle time.
    pub fn poll(
        &mut self,
        now: Instant,
        contact_lost: bool,
        out: &mut Vec<InputPacket>,
    ) -> Option<Instant> {
        self.poll_inner(now, contact_lost, false, out)
    }

    /// Takes everything without waiting for batching intervals, as the C drains its queue
    /// when input stops.
    pub fn drain(&mut self, now: Instant, contact_lost: bool, out: &mut Vec<InputPacket>) {
        self.poll_inner(now, contact_lost, true, out);
    }

    fn poll_inner(
        &mut self,
        now: Instant,
        contact_lost: bool,
        force: bool,
        out: &mut Vec<InputPacket>,
    ) -> Option<Instant> {
        if self.holding && !contact_lost {
            self.release_held(out);
        }
        let motion_interval = self.motion_interval();
        let mut wake: Option<Instant> = None;
        let mut index = 0;
        while index < self.entries.len() {
            if !force {
                let entry = &self.entries[index];
                if matches!(entry, Entry::Text(_)) && !contact_lost {
                    let at = *self.text_ready_at.get_or_insert(now + TEXT_SETTLE);
                    if now < at {
                        wake = Some(wake.map_or(at, |w| w.min(at)));
                        index += 1;
                        continue;
                    }
                }
                // Keep text-before-keyboard ordering and never jump a later event past a waiting
                // key (e.g. a modifier before a click).
                if index > 0
                    && matches!(
                        entry,
                        Entry::Event(Pending {
                            event: InputEvent::Keyboard { .. },
                            ..
                        })
                    )
                {
                    break;
                }
                let wait_until = match entry {
                    Entry::RelativeMouse if self.entries.len() == index + 1 => {
                        self.last_mouse.map(|t| t + motion_interval.max(self.relative_floor))
                    }
                    Entry::AbsoluteMouse if self.entries.len() == index + 1 => {
                        self.last_mouse.map(|t| t + motion_interval)
                    }
                    Entry::Event(Pending {
                        event: InputEvent::Pen { event, buttons, .. },
                        ..
                    }) if touch_event::is_batchable(*event)
                        && !self.has_non_collapsible_pen_after(index, *event, *buttons) =>
                    {
                        self.last_pen.map(|t| t + motion_interval)
                    }
                    _ => None,
                };
                if let Some(at) = wait_until
                    && now < at
                {
                    wake = Some(wake.map_or(at, |w| w.min(at)));
                    break;
                }
            }
            let entry = self.entries.remove(index).expect("entry at index exists");
            match entry {
                Entry::Event(pending) => {
                    if let InputEvent::Pen { event, buttons, .. } = pending.event
                        && touch_event::is_batchable(event)
                    {
                        let latest = self.collapse_pen(index, pending, event, buttons);
                        self.last_pen = Some(now);
                        self.emit(latest, contact_lost, out);
                        continue;
                    }
                    self.emit(pending, contact_lost, out);
                }
                Entry::Controller { id, state } => {
                    if let InputEvent::Controller { number, .. } = state.event {
                        let slot = &mut self.controller_batch[usize::from(number)];
                        if *slot == Some(id) {
                            *slot = None;
                        }
                    }
                    self.emit(state, contact_lost, out);
                }
                Entry::RelativeMouse => {
                    let split = |remaining: &mut i32| {
                        let part = (*remaining).clamp(i32::from(i16::MIN), i32::from(i16::MAX));
                        *remaining -= part;
                        part as i16
                    };
                    while self.relative != (0, 0) {
                        let dx = split(&mut self.relative.0);
                        let dy = split(&mut self.relative.1);
                        self.emit(
                            Pending::new(InputEvent::MouseMove { dx, dy }),
                            contact_lost,
                            out,
                        );
                    }
                    self.relative_dirty = false;
                    self.last_mouse = Some(now);
                }
                Entry::AbsoluteMouse => {
                    let (x, y, width, height) = self.absolute;
                    // GFE's scaling cannot reach the far edge without the reference size less one.
                    let event = InputEvent::MousePosition {
                        x,
                        y,
                        width: width.wrapping_sub(1),
                        height: height.wrapping_sub(1),
                    };
                    self.absolute_dirty = false;
                    self.last_mouse = Some(now);
                    self.emit(Pending::new(event), contact_lost, out);
                }
                Entry::Motion { number, kind } => {
                    let sensor = &mut self.sensors[usize::from(number)][usize::from(kind - 1)];
                    sensor.dirty = false;
                    let [x, y, z] = sensor.xyz;
                    // A zero gyro reading lets a client park the gyro; it must not be lost.
                    let delivery = if kind == motion_type::GYRO && x == 0.0 && y == 0.0 && z == 0.0
                    {
                        Delivery::Reliable
                    } else {
                        Delivery::Unreliable
                    };
                    let event = InputEvent::ControllerMotion {
                        number,
                        motion_type: kind,
                        x,
                        y,
                        z,
                    };
                    self.emit(
                        Pending {
                            channel: event.channel(),
                            delivery,
                            event,
                        },
                        contact_lost,
                        out,
                    );
                }
                Entry::Text(text) => {
                    self.text_ready_at = None;
                    if contact_lost {
                        // Text typed while the host is silent would arrive late.
                        self.note_held();
                        continue;
                    }
                    for code_point in code_points(&text) {
                        self.emit(
                            Pending::new(InputEvent::Text(code_point.to_vec())),
                            contact_lost,
                            out,
                        );
                    }
                }
            }
        }
        wake
    }

    fn has_non_collapsible_pen_after(&self, index: usize, event: u8, buttons: u8) -> bool {
        self.entries.iter().skip(index + 1).any(|next| {
            !matches!(
                next,
                Entry::Event(Pending {
                    event: InputEvent::Pen { event: next_event, buttons: next_buttons, .. },
                    ..
                }) if *next_event == event && *next_buttons == buttons
            )
        })
    }

    /// Replaces a pen move or hover with the newest one queued right behind it with the same
    /// event and buttons.
    fn collapse_pen(
        &mut self,
        index: usize,
        mut pending: Pending,
        event: u8,
        buttons: u8,
    ) -> Pending {
        while let Some(Entry::Event(Pending {
            event:
                InputEvent::Pen {
                    event: next_event,
                    buttons: next_buttons,
                    ..
                },
            ..
        })) = self.entries.get(index)
        {
            if *next_event != event || *next_buttons != buttons {
                break;
            }
            let Some(Entry::Event(next)) = self.entries.remove(index) else {
                unreachable!("entry at index was a pen event");
            };
            // A button change travels reliably; the newer move that carries the same
            // buttons takes that over with its place.
            let reliable = matches!(pending.delivery, Delivery::Reliable);
            pending = next;
            if reliable {
                pending.delivery = Delivery::Reliable;
            }
        }
        pending
    }

    fn emit(&mut self, pending: Pending, contact_lost: bool, out: &mut Vec<InputPacket>) {
        if contact_lost {
            self.hold(pending);
            return;
        }
        note_sent(self.frame_warp.as_deref(), &pending.event);
        out.push(InputPacket {
            data: pending.event.encode(self.sunshine),
            channel: pending.channel,
            delivery: pending.delivery,
        });
    }
}

/// Counts a packet that goes on the wire for Frame Warp: relative mouse motion and controller
/// state, exactly as sent.
pub(super) fn note_sent(frame_warp: Option<&crate::frame_warp::FrameWarp>, event: &InputEvent) {
    let Some(frame_warp) = frame_warp else { return };
    match *event {
        InputEvent::MouseMove { dx, dy } => frame_warp.note_mouse(dx, dy),
        InputEvent::Controller {
            number,
            right_stick,
            ..
        } => frame_warp.note_controller(number, right_stick),
        _ => {}
    }
}

/// Splits UTF-8 into code points by their lead bytes, stopping at an invalid one as the C
/// does.
pub(super) fn code_points(text: &[u8]) -> Vec<&[u8]> {
    let mut points = Vec::new();
    let mut i = 0;
    while i < text.len() {
        let lead = text[i];
        let len = match lead {
            b if b & 0x80 == 0 => 1,
            b if b & 0xE0 == 0xC0 => 2,
            b if b & 0xF0 == 0xE0 => 3,
            b if b & 0xF8 == 0xF0 => 4,
            _ => break,
        };
        points.push(&text[i..(i + len).min(text.len())]);
        i += len;
    }
    points
}
