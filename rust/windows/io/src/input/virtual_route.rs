//! Virtual HID mouse and Precision Touchpad routing and fallback reporting for [`Injector`].

use super::*;

impl Injector {
    /// Plugs in the virtual mouse for `mouse_mode` auto or virtual_mouse, and takes a share of
    /// the virtual touchpad, which plugs in at the first touchpad packet. Tests use neither.
    pub(super) fn start_virtual_devices(&mut self) {
        if cfg!(test) || !self.policy.mouse {
            return;
        }
        if self.policy.mouse_mode.wants_virtual() {
            virtual_hid::MOUSE.acquire();
            self.virtual_mouse = true;
            let availability = virtual_hid::MOUSE
                .plug(|| butterpollo_input_policy::virtual_hid::HidMouse::new(virtual_hid::SERIAL));
            self.report_virtual_mouse(&availability);
        }
        if self.policy.native_touchpad {
            virtual_hid::TOUCHPAD.acquire();
            self.touchpad = true;
        }
    }
    /// Says why mouse input is injected when the virtual mouse cannot take it: a log line
    /// for `auto`, and a stream warning too when the setting asked for the virtual mouse.
    fn report_virtual_mouse(&self, availability: &virtual_hid::Availability) {
        use butterpollo_input_policy::input_policy::MouseMode;
        use virtual_hid::Availability;
        let asked = self.policy.mouse_mode == MouseMode::VirtualMouse;
        let why = match availability {
            Availability::Ready | Availability::Plugging => {
                tracing::info!(
                    mode = ?self.policy.mouse_mode,
                    "mouse input goes to the virtual mouse once it is plugged in"
                );
                return;
            }
            Availability::NoDriver => {
                "it needs usbip-win2, the USB audio and haptics driver".to_owned()
            }
            Availability::Failed(error) => format!("it could not be plugged in: {error}"),
        };
        if asked {
            tracing::warn!(
                why,
                "mouse input is injected instead of using the virtual mouse"
            );
            self.warnings.set(
                "mouse_virtual_unavailable",
                format!(
                    "Mouse input uses Windows injection instead of the virtual mouse: {why}. Install the USB audio and haptics driver with the setup, or set Mouse input method to Injection."
                ),
            );
        } else {
            tracing::info!(
                why,
                "mouse input is injected: the virtual mouse is unavailable"
            );
        }
    }
    /// Sends inputs as SendInput would, putting mouse input on the virtual mouse while this
    /// stream uses it and it is plugged in, while keeping each mouse button press and release
    /// on the exact route that pressed it. Returns how many of the first inputs went out.
    pub(super) fn deliver(&self, inputs: &[INPUT]) -> usize {
        let routed = self.virtual_mouse && virtual_hid::MOUSE.ready();
        if self.virtual_mouse
            && self
                .virtual_mouse_routed
                .swap(routed, std::sync::atomic::Ordering::Relaxed)
                != routed
        {
            if routed {
                tracing::info!("mouse input goes to the virtual mouse");
            } else {
                tracing::warn!("mouse input is injected: the virtual mouse is gone");
            }
        }
        let forced = self.forced_route.get();
        let from_pending = forced.is_none()
            && !self.pending.inputs.is_empty()
            && std::ptr::eq(inputs.as_ptr(), self.pending.inputs.as_ptr())
            && self.pending.routes.len() == inputs.len();
        let route_of = |i: usize| -> MouseRoute {
            if let Some(route) = forced {
                route
            } else if from_pending {
                self.pending.routes[i]
            } else {
                MouseRoute::Auto
            }
        };
        let any_virtual = match forced {
            Some(MouseRoute::Virtual) => true,
            Some(MouseRoute::Injection) => false,
            Some(MouseRoute::Auto) | None => {
                routed || (from_pending && self.pending.routes.contains(&MouseRoute::Virtual))
            }
        };
        if !any_virtual {
            let sent = (self.inject)(inputs);
            tracing::trace!(
                target: "input_timing",
                qpc = virtual_hid::qpc(),
                route = "injection",
                inputs = sent,
                "delivered"
            );
            return sent;
        }
        let single_display = virtual_hid::single_display();
        // One mouse input's steps, on the stack and reused across the pass.
        let mut ops = [virtual_hid::Op::Motion(0, 0); virtual_hid::MAX_OPS];
        let mut sent = 0;
        let mut start = 0;
        for (i, input) in inputs.iter().enumerate() {
            if input.r#type != INPUT_MOUSE {
                continue;
            }
            let route = route_of(i);
            if route == MouseRoute::Injection || (route == MouseRoute::Auto && !routed) {
                continue;
            }
            // SAFETY: a mouse INPUT holds its MOUSEINPUT in the union.
            let mi = unsafe { input.Anonymous.mi };
            let Some(len) = virtual_hid::hid_ops(&mi, single_display, &mut ops) else {
                continue;
            };
            // Keys and SendInput-routed mouse inputs before it go first, in order.
            if start < i {
                let n = (self.inject)(&inputs[start..i]);
                sent += n;
                if n < i - start {
                    return sent;
                }
            }
            let applied = if route == MouseRoute::Virtual {
                virtual_hid::apply(&ops[..len])
                    || ops[..len]
                        .iter()
                        .all(|op| matches!(op, virtual_hid::Op::Button(_, false)))
            } else {
                virtual_hid::apply(&ops[..len]) || (self.inject)(std::slice::from_ref(input)) == 1
            };
            if !applied {
                return sent;
            }
            sent += 1;
            start = i + 1;
        }
        if start < inputs.len() {
            sent += (self.inject)(&inputs[start..]);
        }
        tracing::trace!(
            target: "input_timing",
            qpc = virtual_hid::qpc(),
            route = "virtual_mouse",
            inputs = sent,
            "delivered"
        );
        sent
    }
    /// A tablet's physical touchpad: its fingers replay on the virtual Precision Touchpad,
    /// plugged in at the first packet with the client's size.
    pub(super) fn touchpad_packet(
        &mut self,
        buttons: u8,
        width_mm: u16,
        height_mm: u16,
        contacts: &[butterpollo_input_policy::virtual_hid::Contact],
    ) {
        use virtual_hid::Availability;
        if !self.touchpad {
            return;
        }
        let now = Instant::now();
        let applied = virtual_hid::TOUCHPAD.with(|pad| {
            pad.apply(contacts, buttons, now);
            (pad.active(), pad.early_contacts)
        });
        let (code, why) = match applied {
            Some((true, _)) => return,
            // Windows reads the configuration at once; this many contacts without it means it
            // took the device as something else.
            Some((false, early)) if early >= 500 => (
                "touchpad_not_precision",
                "Windows did not switch the virtual touchpad to precision touchpad input"
                    .to_owned(),
            ),
            Some(_) => return,
            None => match virtual_hid::TOUCHPAD.plug(move || {
                butterpollo_input_policy::virtual_hid::PrecisionTouchpad::new(
                    width_mm,
                    height_mm,
                    virtual_hid::SERIAL,
                    Instant::now(),
                )
            }) {
                Availability::Ready | Availability::Plugging => {
                    if !self.touchpad_reported {
                        self.touchpad_reported = true;
                        tracing::info!(
                            width_mm,
                            height_mm,
                            "plugging in the virtual touchpad for the device's touchpad"
                        );
                    }
                    return;
                }
                Availability::NoDriver => (
                    "touchpad_no_driver",
                    "the virtual touchpad needs usbip-win2, the USB audio and haptics driver"
                        .to_owned(),
                ),
                Availability::Failed(error) => (
                    "touchpad_failed",
                    format!("the virtual touchpad could not be plugged in: {error}"),
                ),
            },
        };
        self.touchpad_unavailable(code, &why);
    }
    /// Says on the stream card, once per `code`, why the device's touchpad gestures do not reach
    /// Windows.
    pub(super) fn touchpad_unavailable(&mut self, code: &'static str, why: &str) {
        if self.touchpad_warned == Some(code) {
            return;
        }
        self.touchpad_warned = Some(code);
        tracing::warn!(
            why,
            "touchpad gestures from the device do not reach Windows"
        );
        self.warnings.set(
            code,
            format!(
                "Touchpad gestures from this device do not reach Windows: {why}. Turn off touchpad gestures on the device to use its touchpad as a mouse."
            ),
        );
    }
}
