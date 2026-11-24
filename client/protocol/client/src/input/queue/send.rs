//! Producer `send_*` methods on [`InputQueue`]: mouse, keyboard, text, controllers,
//! scrolling, touch, pen and laptop touchpad input.

use rubylight_protocol::pen;

use super::{
    Entry, InputError, InputQueue, Pending, GFE_CONTROLLERS, MAX_MOTION_TYPES, WHEEL_DELTA,
};
use crate::control::negotiation::feature;
use crate::control::wire::Delivery;
use crate::input::packets::{
    button, capability, touch_event, touchpad, InputEvent, TouchpadContact, MAX_CONTROLLERS,
};

impl InputQueue {
    pub fn send_mouse_move(&mut self, dx: i16, dy: i16) -> Result<(), InputError> {
        self.check()?;
        if dx == 0 && dy == 0 {
            return Ok(());
        }
        self.relative.0 += i32::from(dx);
        self.relative.1 += i32::from(dy);
        if !self.relative_dirty {
            self.offer(Entry::RelativeMouse)?;
            self.relative_dirty = true;
        }
        Ok(())
    }

    /// An absolute position within a `width` x `height` reference area.
    pub fn send_mouse_position(
        &mut self,
        x: i16,
        y: i16,
        width: i16,
        height: i16,
    ) -> Result<(), InputError> {
        self.check()?;
        self.absolute = (x, y, width, height);
        let result = if self.absolute_dirty {
            Ok(())
        } else {
            self.offer(Entry::AbsoluteMouse)
                .inspect(|_| self.absolute_dirty = true)
        };
        let fraction = |v: i16, size: i16| {
            let span = f32::from(size) - 1.0;
            if span <= 0.0 {
                0.0
            } else {
                f32::from(v).clamp(0.0, span) / span
            }
        };
        self.position = (fraction(x, width), fraction(y, height));
        result
    }

    /// Relative motion sent as an absolute position, from the last position sent.
    pub fn send_mouse_move_as_position(
        &mut self,
        dx: i16,
        dy: i16,
        width: i16,
        height: i16,
    ) -> Result<(), InputError> {
        let old_x = (self.position.0 * (f32::from(width) - 1.0) + 0.5) as i16;
        let old_y = (self.position.1 * (f32::from(height) - 1.0) + 0.5) as i16;
        let clamp = |v: i32, max: i16| v.clamp(0, i32::from(max.max(0))) as i16;
        self.send_mouse_position(
            clamp(i32::from(old_x) + i32::from(dx), width),
            clamp(i32::from(old_y) + i32::from(dy), height),
            width,
            height,
        )
    }

    pub fn send_mouse_button(&mut self, button: u8, down: bool) -> Result<(), InputError> {
        self.check()?;
        self.offer_event(InputEvent::MouseButton { button, down })
    }

    /// A key with a Windows virtual key code. GFE gets the modifier fix-ups the C applies;
    /// `flags` reaches Sunshine hosts only.
    pub fn send_keyboard(
        &mut self,
        key_code: u16,
        down: bool,
        mut modifiers: u8,
        flags: u8,
    ) -> Result<(), InputError> {
        use crate::input::packets::modifier::*;
        self.check()?;
        if !self.sunshine {
            match key_code & 0xFF {
                // GFE drops events carrying META, but the Windows key itself works without it.
                0x5B | 0x5C => modifiers &= !META,
                0xA0 => modifiers |= SHIFT,
                0xA1 => modifiers &= !SHIFT,
                0xA2 => modifiers |= CTRL,
                0xA3 => modifiers &= !CTRL,
                0xA4 => modifiers |= ALT,
                0xA5 => modifiers &= !ALT,
                _ => {}
            }
            modifiers &= !EXTENDED;
        }
        let flags = if self.sunshine { flags } else { 0 };
        self.offer_event(InputEvent::Keyboard {
            key_code,
            down,
            modifiers,
            flags,
        })
    }

    pub fn send_text(&mut self, text: &str) -> Result<(), InputError> {
        self.check()?;
        if text.is_empty() {
            return Ok(());
        }
        self.offer(Entry::Text(text.as_bytes().to_vec()))
    }

    /// A controller's full state. Calls with unchanged buttons update a state still queued.
    #[allow(clippy::too_many_arguments)]
    pub fn send_controller(
        &mut self,
        number: u16,
        mut active_mask: u16,
        mut buttons: u32,
        left_trigger: u8,
        right_trigger: u8,
        left_stick: (i16, i16),
        right_stick: (i16, i16),
    ) -> Result<(), InputError> {
        self.check()?;
        // Callers that passed a sign-extended 16-bit value set every upper flag with Y.
        if buttons & 0x8000_0000 != 0 {
            buttons &= 0xFFFF;
        }
        // The C button's flag (0x40_0000) is also a Steam Controller's left
        // grip touch (`rubylight_protocol::steam_controller::LEFT_GRIP_TOUCH_FLAG`),
        // so it may not be cleared here for every pad: a host without
        // `ControllerIdentityVersion` would never see the left grip
        // (nightjar#125). The desktop sends its own C press only to a host
        // that takes identities (`send_state` in `gamepad.rs`).
        let number = if self.sunshine {
            number % u16::from(MAX_CONTROLLERS)
        } else {
            // GFE takes four controllers and no extended buttons; MISC stands in for Guide.
            active_mask &= (1 << GFE_CONTROLLERS) - 1;
            if buttons & button::MISC != 0 {
                buttons |= button::SPECIAL;
            }
            number % u16::from(GFE_CONTROLLERS)
        };
        let event = InputEvent::Controller {
            number,
            active_mask,
            buttons,
            left_trigger,
            right_trigger,
            left_stick,
            right_stick,
        };
        let sunshine = self.sunshine;
        let same_buttons = |queued: &InputEvent| match queued {
            InputEvent::Controller { buttons: old, .. } => {
                *old as u16 == buttons as u16
                    && (!sunshine || (*old >> 16) as u16 == (buttons >> 16) as u16)
            }
            _ => false,
        };
        if let Some(id) = self.controller_batch[usize::from(number)]
            && let Some(Entry::Controller { state, .. }) = self
                .entries
                .iter_mut()
                .rev()
                .find(|e| matches!(e, Entry::Controller { id: queued, .. } if *queued == id))
            && same_buttons(&state.event)
        {
            state.event = event;
            return Ok(());
        }
        let id = self.next_id;
        self.next_id += 1;
        self.offer(Entry::Controller {
            id,
            state: Pending::new(event),
        })?;
        self.controller_batch[usize::from(number)] = Some(id);
        Ok(())
    }

    /// `LiSendControllerEvent`: controller 0 alone.
    pub fn send_single_controller(
        &mut self,
        buttons: u32,
        left_trigger: u8,
        right_trigger: u8,
        left_stick: (i16, i16),
        right_stick: (i16, i16),
    ) -> Result<(), InputError> {
        self.send_controller(
            0,
            1,
            buttons,
            left_trigger,
            right_trigger,
            left_stick,
            right_stick,
        )
    }

    /// Tells a host with `ControllerIdentityVersion` which controller `number` is, by its USB
    /// vendor and product. Send it before the arrival: the host chooses the virtual pad then.
    /// Nothing is sent to any other host.
    pub fn send_controller_identity(
        &mut self,
        number: u8,
        vendor: u16,
        product: u16,
    ) -> Result<(), InputError> {
        self.check()?;
        if !self.controller_identity {
            return Ok(());
        }
        self.offer_event(InputEvent::ControllerIdentity {
            number: number % MAX_CONTROLLERS,
            vendor,
            product,
        })
    }

    /// Announces a controller to Sunshine hosts, then sends a neutral state for hosts that do
    /// not take arrivals.
    pub fn send_controller_arrival(
        &mut self,
        number: u8,
        active_mask: u16,
        kind: u8,
        supported_buttons: u32,
        mut capabilities: u16,
    ) -> Result<(), InputError> {
        self.check()?;
        let number = number % MAX_CONTROLLERS;
        if capabilities & capability::DUAL_TOUCHPAD != 0 {
            capabilities |= capability::TOUCHPAD;
        }
        if self.sunshine {
            self.offer_event(InputEvent::ControllerArrival {
                number,
                kind,
                capabilities,
                supported_buttons,
            })?;
        }
        self.send_controller(u16::from(number), active_mask, 0, 0, 0, (0, 0), (0, 0))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn send_controller_touch(
        &mut self,
        number: u8,
        event: u8,
        touchpad: u8,
        pointer_id: u32,
        x: f32,
        y: f32,
        pressure: f32,
    ) -> Result<(), InputError> {
        self.check()?;
        if self.feature_flags & feature::CONTROLLER_TOUCH_EVENTS == 0 {
            return Err(InputError::Unsupported);
        }
        let number = number % MAX_CONTROLLERS;
        self.offer_event(InputEvent::ControllerTouch {
            number,
            event,
            touchpad,
            pointer_id,
            x,
            y,
            pressure,
        })
    }

    /// Accelerometer (m/s², gravity included) or gyroscope (deg/s); only the latest value per
    /// controller and sensor is sent.
    pub fn send_controller_motion(
        &mut self,
        number: u8,
        kind: u8,
        x: f32,
        y: f32,
        z: f32,
    ) -> Result<(), InputError> {
        self.check()?;
        if !(1..=MAX_MOTION_TYPES as u8).contains(&kind) {
            return Err(InputError::Invalid);
        }
        if self.feature_flags & feature::CONTROLLER_TOUCH_EVENTS == 0 {
            return Err(InputError::Unsupported);
        }
        let number = number % MAX_CONTROLLERS;
        let sensor = &mut self.sensors[usize::from(number)][usize::from(kind - 1)];
        sensor.xyz = [x, y, z];
        if !sensor.dirty {
            self.offer(Entry::Motion { number, kind })?;
            self.sensors[usize::from(number)][usize::from(kind - 1)].dirty = true;
        }
        Ok(())
    }

    pub fn send_controller_battery(
        &mut self,
        number: u8,
        state: u8,
        percentage: u8,
    ) -> Result<(), InputError> {
        self.check()?;
        if !self.sunshine {
            return Err(InputError::Unsupported);
        }
        let number = number % MAX_CONTROLLERS;
        self.offer_event(InputEvent::ControllerBattery {
            number,
            state,
            percentage,
        })
    }

    /// Whole wheel clicks.
    pub fn send_scroll(&mut self, clicks: i8) -> Result<(), InputError> {
        self.send_high_res_scroll(i16::from(clicks) * WHEEL_DELTA as i16)
    }

    /// Scroll in 1/120 of a click. GFE's virtual HID devices take only whole clicks, so for
    /// them amounts are gathered into clicks.
    pub fn send_high_res_scroll(&mut self, amount: i16) -> Result<(), InputError> {
        self.check()?;
        if amount == 0 {
            return Ok(());
        }
        if !self.batched_scroll {
            return self.offer_event(InputEvent::Scroll { amount });
        }
        if (self.scroll_accumulator < 0 && amount > 0)
            || (self.scroll_accumulator > 0 && amount < 0)
        {
            self.scroll_accumulator = 0;
        }
        self.scroll_accumulator += i32::from(amount);
        while self.scroll_accumulator.abs() >= WHEEL_DELTA {
            let click = if self.scroll_accumulator > 0 {
                WHEEL_DELTA
            } else {
                -WHEEL_DELTA
            };
            self.offer_event(InputEvent::Scroll {
                amount: click as i16,
            })?;
            self.scroll_accumulator -= click;
        }
        Ok(())
    }

    /// A wheel turn in the events' units: one is a whole notch, and a trackpad sends a fraction of
    /// one per event. The fraction that is not a whole 1/120 of a click waits for the next turn,
    /// so a slow scroll still moves the page instead of vanishing in the conversion. The vertical
    /// axis goes first: a host that refuses horizontal scroll (GFE) must not cost the turn's
    /// scroll, and a full queue on one axis must not drop the other.
    pub fn send_wheel_turn(&mut self, x: f64, y: f64) -> Result<(), InputError> {
        let vertical = self.send_wheel_axis(y, true);
        let horizontal = self.send_wheel_axis(x, false);
        vertical.and(horizontal)
    }

    fn send_wheel_axis(&mut self, units: f64, vertical: bool) -> Result<(), InputError> {
        self.check()?;
        if !units.is_finite() || units == 0.0 {
            return Ok(());
        }
        let held = if vertical { self.wheel_units.1 } else { self.wheel_units.0 };
        let total = held + units * f64::from(WHEEL_DELTA);
        let whole = total.trunc().clamp(-32_000.0, 32_000.0) as i16;
        if whole == 0 {
            if vertical { self.wheel_units.1 = total } else { self.wheel_units.0 = total };
            return Ok(());
        }
        let result = if vertical {
            self.send_high_res_scroll(whole)
        } else {
            self.send_high_res_hscroll(whole)
        };
        let kept = match &result {
            Ok(()) => total - f64::from(whole),
            // The host never takes this axis: nothing would ever carry the fraction.
            Err(InputError::Unsupported) => 0.0,
            // The accumulator already owns the whole clicks of this axis: keeping them here too
            // would send them again with the next turn.
            Err(_) if vertical && self.batched_scroll => total - f64::from(whole),
            Err(_) => total,
        };
        if vertical { self.wheel_units.1 = kept } else { self.wheel_units.0 = kept };
        result
    }

    pub fn send_hscroll(&mut self, clicks: i8) -> Result<(), InputError> {
        self.send_high_res_hscroll(i16::from(clicks) * WHEEL_DELTA as i16)
    }

    pub fn send_high_res_hscroll(&mut self, amount: i16) -> Result<(), InputError> {
        self.check()?;
        if !self.sunshine {
            return Err(InputError::Unsupported);
        }
        if amount == 0 {
            return Ok(());
        }
        self.offer_event(InputEvent::HScroll { amount })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn send_touch(
        &mut self,
        event: u8,
        pointer_id: u32,
        x: f32,
        y: f32,
        pressure_or_distance: f32,
        contact_major: f32,
        contact_minor: f32,
        rotation: u16,
    ) -> Result<(), InputError> {
        self.check()?;
        if self.feature_flags & feature::PEN_TOUCH_EVENTS == 0 {
            return Err(InputError::Unsupported);
        }
        self.offer_event(InputEvent::Touch {
            event,
            pointer_id,
            x,
            y,
            pressure_or_distance,
            contact_major,
            contact_minor,
            rotation,
        })
    }

    /// `rotation` is the tilt's direction (stock), `roll` the barrel roll in degrees 0-359
    /// clockwise or [`pen::ROLL_UNKNOWN`]; the roll goes only to a host that takes it.
    #[allow(clippy::too_many_arguments)]
    pub fn send_pen(
        &mut self,
        event: u8,
        tool: u8,
        buttons: u8,
        x: f32,
        y: f32,
        pressure_or_distance: f32,
        contact_major: f32,
        contact_minor: f32,
        rotation: u16,
        tilt: u8,
        roll: u16,
    ) -> Result<(), InputError> {
        self.check()?;
        if self.feature_flags & feature::PEN_TOUCH_EVENTS == 0 {
            return Err(InputError::Unsupported);
        }
        // Moves and hovers without a button change may be dropped for a newer one.
        let delivery = if touch_event::is_batchable(event) && buttons == self.pen_buttons {
            Delivery::Unreliable
        } else {
            Delivery::Reliable
        };
        let event = InputEvent::Pen {
            event,
            tool,
            buttons,
            x,
            y,
            pressure_or_distance,
            contact_major,
            contact_minor,
            rotation,
            tilt,
            roll: if self.pen_barrel_roll && roll < 360 {
                roll
            } else {
                pen::ROLL_UNKNOWN
            },
        };
        self.offer(Entry::Event(Pending {
            channel: event.channel(),
            delivery,
            event,
        }))?;
        // Only once it is queued: a refused event leaves the buttons the host last got,
        // so the click tried again is still a change.
        self.pen_buttons = buttons;
        Ok(())
    }

    /// `LiSendTouchpadFrameEvent`: the contacts of one laptop touchpad frame, at most
    /// [`touchpad::MAX_FRAME_CONTACTS`]. A host without `TOUCHPAD_FRAME_EVENTS` refuses it.
    pub fn send_touchpad_frame(
        &mut self,
        contacts: &[TouchpadContact],
        buttons: u8,
        width_mm: u16,
        height_mm: u16,
    ) -> Result<(), InputError> {
        self.check()?;
        if self.feature_flags & feature::TOUCHPAD_FRAME_EVENTS == 0 {
            return Err(InputError::Unsupported);
        }
        if contacts.len() > touchpad::MAX_FRAME_CONTACTS {
            return Err(InputError::Invalid);
        }
        let mut frame = [TouchpadContact::default(); touchpad::MAX_FRAME_CONTACTS];
        frame[..contacts.len()].copy_from_slice(contacts);
        // Only a frame of moves with no button change may be dropped for a newer one.
        let batchable = contacts.iter().all(|c| touch_event::is_batchable(c.event));
        let delivery = self.touchpad_delivery(batchable, buttons);
        let event = InputEvent::TouchpadFrame {
            count: contacts.len() as u8,
            buttons,
            rotation: touchpad::ROTATION_UNKNOWN,
            width_mm,
            height_mm,
            contacts: frame,
        };
        self.offer(Entry::Event(Pending { channel: event.channel(), delivery, event }))
    }

    /// `LiSendTouchpadEvent`: one laptop touchpad contact. A host without `TOUCHPAD_EVENTS`
    /// refuses it.
    pub fn send_touchpad(
        &mut self,
        contact: TouchpadContact,
        buttons: u8,
        width_mm: u16,
        height_mm: u16,
    ) -> Result<(), InputError> {
        self.check()?;
        if self.feature_flags & feature::TOUCHPAD_EVENTS == 0 {
            return Err(InputError::Unsupported);
        }
        let delivery = self.touchpad_delivery(touch_event::is_batchable(contact.event), buttons);
        let event = InputEvent::Touchpad {
            event: contact.event,
            buttons,
            rotation: touchpad::ROTATION_UNKNOWN,
            pointer_id: contact.pointer_id,
            width_mm,
            height_mm,
            x: contact.x,
            y: contact.y,
            pressure: contact.pressure,
            contact_major: 0.0,
            contact_minor: 0.0,
        };
        self.offer(Entry::Event(Pending { channel: event.channel(), delivery, event }))
    }

    /// Moves without a button change may be dropped; a click or release never is.
    fn touchpad_delivery(&mut self, batchable: bool, buttons: u8) -> Delivery {
        let delivery = if batchable && buttons == self.touchpad_buttons {
            Delivery::Unreliable
        } else {
            Delivery::Reliable
        };
        self.touchpad_buttons = buttons;
        delivery
    }
}
