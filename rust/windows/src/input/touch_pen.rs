//! Synthetic touch contacts and pen frames with pointer refresh.

use super::*;

/// How often held touch contacts and an active pen are injected again:
/// Windows cancels pointer input that is not repeated.
const POINTER_REFRESH: std::time::Duration = std::time::Duration::from_millis(250);
/// Whether a pointer last injected at `last` is due again at `now`; if so,
/// its period restarts. Touch and pen keep their own periods: a pen that
/// keeps hovering must not hold back the repeat of a held touch, which
/// Windows would then cancel, nor the other way round.
pub(super) fn refresh_due(last: &mut std::time::Instant, now: std::time::Instant) -> bool {
    let due = now.saturating_duration_since(*last) >= POINTER_REFRESH;
    if due {
        *last = now;
    }
    due
}
fn pointer_rect(rect: RECT, origin: POINT) -> RECT {
    RECT {
        left: rect.left - origin.x,
        top: rect.top - origin.y,
        right: rect.right - origin.x,
        bottom: rect.bottom - origin.y,
    }
}
/// The contact area around `center`, kept inside `rect`. The client's
/// `major` and `minor` are fractions of the stream; any other value, such
/// as 1e30 or NaN, is clamped so the rectangle math cannot overflow.
fn contact_rect(
    center: POINT,
    rect: RECT,
    (major, minor): (f32, f32),
    angle: f32,
    (scale_x, scale_y): (f32, f32),
) -> RECT {
    let fraction = |value: f32| {
        if value.is_nan() {
            0.
        } else {
            value.clamp(0., 1.)
        }
    };
    let (major, minor) = (fraction(major), fraction(minor));
    let width = ((angle.cos().abs() * major + angle.sin().abs() * minor)
        * scale_x
        * (rect.right - rect.left) as f32)
        .max(1.);
    let height = ((angle.sin().abs() * major + angle.cos().abs() * minor)
        * scale_y
        * (rect.bottom - rect.top) as f32)
        .max(1.);
    let (half_width, half_height) = ((width / 2.).ceil() as i32, (height / 2.).ceil() as i32);
    RECT {
        left: center.x.saturating_sub(half_width).max(rect.left),
        right: center.x.saturating_add(half_width).min(rect.right),
        top: center.y.saturating_sub(half_height).max(rect.top),
        bottom: center.y.saturating_add(half_height).min(rect.bottom),
    }
}
fn touch_info(touch: POINTER_TOUCH_INFO) -> POINTER_TYPE_INFO {
    POINTER_TYPE_INFO {
        r#type: PT_TOUCH,
        Anonymous: POINTER_TYPE_INFO_0 { touchInfo: touch },
    }
}
/// A move cannot introduce a contact or turn a hover into a contact: Windows
/// requires a successful down first. Ignore stale moves after cancellation.
fn touch_start(
    touches: &BTreeMap<u32, POINTER_TOUCH_INFO>,
    id: u32,
    event: u8,
) -> Result<Option<POINTER_TOUCH_INFO>> {
    if let Some(existing) = touches.get(&id) {
        if event > 6
            || (event == 3
                && existing.pointerInfo.pointerFlags & POINTER_FLAG_INCONTACT == POINTER_FLAG_NONE)
        {
            return Ok(None);
        }
        return Ok(Some(*existing));
    }
    if !matches!(event, 0 | 1) {
        return Ok(None);
    }
    let pointer_id = (1..=32)
        .find(|candidate| {
            !touches
                .values()
                .any(|touch| touch.pointerInfo.pointerId == *candidate)
        })
        .ok_or_else(|| anyhow::anyhow!("touch pointer limit reached"))?;
    Ok(Some(POINTER_TOUCH_INFO {
        pointerInfo: POINTER_INFO {
            pointerType: PT_TOUCH,
            pointerId: pointer_id,
            ..Default::default()
        },
        ..Default::default()
    }))
}
/// Keep only frames Windows accepted. In particular a failed move must not
/// change the position used by the next up, and a refused hover owns no slot.
fn submit_touch(
    touches: &mut BTreeMap<u32, POINTER_TOUCH_INFO>,
    id: u32,
    mut frame: POINTER_TOUCH_INFO,
    event: u8,
    send: impl FnOnce(&[POINTER_TYPE_INFO]) -> Result<()>,
) -> Result<()> {
    let mut contacts: Vec<_> = touches
        .iter()
        .filter(|(other, _)| **other != id)
        .map(|(_, touch)| touch_info(*touch))
        .collect();
    contacts.push(touch_info(frame));
    send(&contacts)?;
    if matches!(event, 2 | 4 | 6) {
        touches.remove(&id);
    } else {
        frame.pointerInfo.pointerFlags &=
            !(POINTER_FLAG_DOWN | POINTER_FLAG_UP | POINTER_FLAG_CANCELED);
        frame.pointerInfo.pointerFlags |= POINTER_FLAG_UPDATE;
        touches.insert(id, frame);
    }
    Ok(())
}
/// If a stale contact invalidates the cancellation frame, still try every
/// other contact individually. The caller retires the device on failure.
fn cancel_touch_contacts(
    touches: &mut BTreeMap<u32, POINTER_TOUCH_INFO>,
    mut send: impl FnMut(&[POINTER_TYPE_INFO]) -> Result<()>,
) -> Result<()> {
    let contacts: Vec<_> = std::mem::take(touches)
        .into_values()
        .map(|mut touch| {
            pointer_event(&mut touch.pointerInfo, 4, POINT::default());
            touch_info(touch)
        })
        .collect();
    if contacts.is_empty() || send(&contacts).is_ok() {
        return Ok(());
    }
    let mut failures = Vec::new();
    for contact in &contacts {
        if let Err(error) = send(std::slice::from_ref(contact)) {
            failures.push(format!("{error:#}"));
        }
    }
    if !failures.is_empty() {
        bail!("touch cancellation failed: {}", failures.join("; "));
    }
    Ok(())
}
impl Injector {
    fn pointer_display(&mut self) -> Option<RECT> {
        let rect = self.display()?;
        // Synthetic pointers and their contact areas are relative to the
        // virtual screen's top-left, while display rectangles use the primary.
        // SAFETY: These metrics take no pointers and return the virtual screen origin.
        let origin = unsafe {
            POINT {
                x: GetSystemMetrics(SM_XVIRTUALSCREEN),
                y: GetSystemMetrics(SM_YVIRTUALSCREEN),
            }
        };
        Some(pointer_rect(rect, origin))
    }
    fn location(&self, rect: RECT, x: f32, y: f32) -> POINT {
        let (x, y) = self.on_display(x, y);
        POINT {
            x: rect.left + ((rect.right - rect.left - 1).max(0) as f32 * x.clamp(0., 1.)) as i32,
            y: rect.top + ((rect.bottom - rect.top - 1).max(0) as f32 * y.clamp(0., 1.)) as i32,
        }
    }
    pub(super) fn inject_touches(&self) -> Result<()> {
        if let Some(device) = self.touch_device {
            let contacts: Vec<_> = self
                .touches
                .values()
                .map(|touch| POINTER_TYPE_INFO {
                    r#type: PT_TOUCH,
                    Anonymous: POINTER_TYPE_INFO_0 { touchInfo: *touch },
                })
                .collect();
            if !contacts.is_empty() {
                // SAFETY: The live touch device owns these contacts, whose union fields and pointer types are initialized as touch input.
                unsafe {
                    inject_pointer(device, &contacts)?;
                }
            }
        }
        Ok(())
    }
    /// Cancel every touch contact, as a client's cancel-all does.
    pub(super) fn cancel_touches(&mut self) -> Result<()> {
        let Some(device) = self.touch_device else {
            self.touches.clear();
            return Ok(());
        };
        let result = cancel_touch_contacts(&mut self.touches, |contacts| {
            // SAFETY: The injector owns the device through every synchronous
            // attempt, and each frame is initialized as touch input.
            unsafe { inject_pointer(device, contacts) }
        });
        if result.is_err() {
            self.touch_device = None;
            // SAFETY: This injector owns the device, which has just been
            // detached so no refresh, later event or Drop can reuse it.
            unsafe { DestroySyntheticPointerDevice(device) };
        }
        result
    }
    /// Lift the pen if it is active.
    pub(super) fn cancel_pen(&mut self) -> Result<()> {
        let (Some(device), Some(frame)) = (self.pen_device, pen_cancel(self.pen)) else {
            return Ok(());
        };
        // SAFETY: The pen device is still owned and frame is initialized as pen input before borrowing it for the call.
        let result = unsafe {
            inject_pointer(
                device,
                &[POINTER_TYPE_INFO {
                    r#type: PT_PEN,
                    Anonymous: POINTER_TYPE_INFO_0 { penInfo: frame },
                }],
            )
        };
        self.pen = pen_after(frame, 7, result.is_err());
        if result.is_err() {
            self.pen_device = None;
            // SAFETY: The injector owns the detached device exactly once.
            unsafe { DestroySyntheticPointerDevice(device) };
        }
        result
    }
    #[allow(clippy::too_many_arguments)]
    pub(super) fn touch(
        &mut self,
        event: u8,
        id: u32,
        x: f32,
        y: f32,
        pressure: f32,
        major: f32,
        minor: f32,
        rotation: u16,
    ) -> Result<()> {
        if event == 7 {
            return self.cancel_touches();
        }
        let Some(mut p) = touch_start(&self.touches, id, event)? else {
            return Ok(());
        };
        // No contact exists before the display does: the rectangle is never
        // forgotten once known.
        let Some(rect) = self.pointer_display() else {
            return Ok(());
        };
        // SAFETY: The synthetic device is retained by this injector, and the initialized touch frames remain live for injection.
        unsafe {
            if self.touch_device.is_none() {
                self.touch_device = Some(
                    CreateSyntheticPointerDevice(PT_TOUCH, 32, POINTER_FEEDBACK_NONE)
                        .context("native touch input unavailable")?,
                );
            }
            pointer_event(&mut p.pointerInfo, event, self.location(rect, x, y));
            if event != 5 {
                p.touchMask = TOUCH_MASK_NONE;
                if p.pointerInfo.pointerFlags & POINTER_FLAG_INCONTACT != POINTER_FLAG_NONE {
                    p.touchMask |= TOUCH_MASK_PRESSURE;
                    p.pressure = if pressure > 0. {
                        (pressure.clamp(0., 1.) * 1024.) as u32
                    } else {
                        512
                    };
                    if major > 0. || minor > 0. {
                        let angle = if rotation == u16::MAX {
                            45.0f32.to_radians()
                        } else {
                            f32::from(rotation).to_radians()
                        };
                        // The contact's size is given on the stream too.
                        let (scale_x, scale_y) = self.picture().map_or((1., 1.), |(stream, p)| {
                            (
                                stream.0 as f32 / p.2.max(1) as f32,
                                stream.1 as f32 / p.3.max(1) as f32,
                            )
                        });
                        p.rcContact = contact_rect(
                            p.pointerInfo.ptPixelLocation,
                            rect,
                            (major, minor),
                            angle,
                            (scale_x, scale_y),
                        );
                        p.touchMask |= TOUCH_MASK_CONTACTAREA;
                    }
                }
                if rotation != u16::MAX {
                    p.orientation = u32::from(rotation % 360);
                    p.touchMask |= TOUCH_MASK_ORIENTATION;
                }
            }
            let device = self.touch_device.expect("touch device created above");
            let result = submit_touch(&mut self.touches, id, p, event, |contacts| {
                inject_pointer(device, contacts)
            });
            if result.is_err()
                && matches!(event, 2 | 4 | 6)
                && let Err(error) = self.cancel_touches()
            {
                tracing::warn!(%error, "touch device retired after a failed release");
            }
            self.touch_refreshed = std::time::Instant::now();
            result?;
        }
        Ok(())
    }
    #[allow(clippy::too_many_arguments)]
    pub(super) fn pen(
        &mut self,
        event: u8,
        tool: u8,
        buttons: u8,
        x: f32,
        y: f32,
        pressure: f32,
        rotation: u16,
        tilt: u8,
        roll: u16,
    ) -> Result<()> {
        // The pen cannot be active before the display exists.
        let Some(rect) = self.pointer_display() else {
            return Ok(());
        };
        let location = self.location(rect, x, y);
        let Some(frame) = pen_frame(
            self.pen, event, tool, buttons, location, pressure, rotation, tilt, roll,
        ) else {
            return Ok(());
        };
        let device = match self.pen_device {
            Some(device) => device,
            // SAFETY: The requested pen type and contact count are valid; the returned device is retained until injector teardown.
            None => *self.pen_device.insert(unsafe {
                CreateSyntheticPointerDevice(PT_PEN, 1, POINTER_FEEDBACK_NONE)
                    .context("native pen input unavailable")?
            }),
        };
        // SAFETY: The live pen device and the initialized pen frame remain valid throughout injection.
        let result = unsafe {
            inject_pointer(
                device,
                &[POINTER_TYPE_INFO {
                    r#type: PT_PEN,
                    Anonymous: POINTER_TYPE_INFO_0 { penInfo: frame },
                }],
            )
        };
        self.pen = pen_after(frame, event, result.is_err());
        self.pen_refreshed = std::time::Instant::now();
        result
    }
}
/// Inject touch or pen input, retrying once on the input desktop.
pub(super) unsafe fn inject_pointer(
    device: HSYNTHETICPOINTERDEVICE,
    info: &[POINTER_TYPE_INFO],
) -> Result<()> {
    // SAFETY: The caller keeps the synthetic device alive and supplies initialized frames of its pointer type for both synchronous attempts.
    unsafe {
        match InjectSyntheticPointerInput(device, info) {
            Ok(()) => Ok(()),
            Err(_) if follow_input_desktop() => Ok(InjectSyntheticPointerInput(device, info)?),
            Err(error) => Err(error.into()),
        }
    }
}
fn pointer_event(p: &mut POINTER_INFO, event: u8, location: POINT) {
    let contact = p.pointerFlags & POINTER_FLAG_INCONTACT != POINTER_FLAG_NONE;
    p.pointerFlags = match event {
        0 => POINTER_FLAG_UPDATE | POINTER_FLAG_INRANGE,
        1 => {
            (if contact {
                POINTER_FLAG_UPDATE
            } else {
                POINTER_FLAG_DOWN
            }) | POINTER_FLAG_INRANGE
                | POINTER_FLAG_INCONTACT
        }
        2 => POINTER_FLAG_UP,
        3 => POINTER_FLAG_UPDATE | POINTER_FLAG_INRANGE | POINTER_FLAG_INCONTACT,
        4 | 7 => {
            (if contact {
                POINTER_FLAG_UP
            } else {
                POINTER_FLAG_UPDATE
            }) | POINTER_FLAG_CANCELED
        }
        5 => p.pointerFlags | POINTER_FLAG_UPDATE,
        6 => POINTER_FLAG_UPDATE,
        _ => p.pointerFlags,
    };
    if matches!(event, 0 | 1 | 3) {
        p.ptPixelLocation = location;
    }
}
/// The pen's next frame from its last state, or None when there is nothing
/// to inject: Windows can pass a button only with an active pen.
#[allow(clippy::too_many_arguments)]
fn pen_frame(
    mut pen: POINTER_PEN_INFO,
    event: u8,
    tool: u8,
    buttons: u8,
    location: POINT,
    pressure: f32,
    rotation: u16,
    tilt: u8,
    roll: u16,
) -> Option<POINTER_PEN_INFO> {
    if event == 5 && pen.pointerInfo.pointerFlags == POINTER_FLAG_NONE {
        return None;
    }
    pen.pointerInfo.pointerType = PT_PEN;
    pen.pointerInfo.pointerId = 1;
    pointer_event(
        &mut pen.pointerInfo,
        if event == 7 { 4 } else { event },
        location,
    );
    pen.penFlags = if buttons != 0 {
        PEN_FLAG_BARREL
    } else {
        PEN_FLAG_NONE
    };
    if tool == 2 {
        pen.penFlags |= PEN_FLAG_ERASER | PEN_FLAG_INVERTED;
    }
    if event != 5 {
        // Windows has no hover distance, and a pressure of 0 is passed as
        // none, as in the C++ host.
        if pen.pointerInfo.pointerFlags & POINTER_FLAG_INCONTACT != POINTER_FLAG_NONE
            && pressure > 0.
        {
            pen.penMask = PEN_MASK_PRESSURE;
            pen.pressure = (pressure.clamp(0., 1.) * 1024.) as u32;
        } else {
            pen.penMask = PEN_MASK_NONE;
            pen.pressure = 0;
        }
        // Windows' rotation is the pen's twist. A known barrel roll (0x5500000B) is that;
        // otherwise the tilt's direction goes there, as in the C++ host.
        let twist = if roll != u16::MAX { roll } else { rotation };
        if twist != u16::MAX {
            pen.penMask |= PEN_MASK_ROTATION;
            pen.rotation = u32::from(twist % 360);
        }
        if tilt != u8::MAX && rotation != u16::MAX {
            let angle = f32::from(rotation).to_radians();
            let tilt = f32::from(tilt.min(90)).to_radians();
            pen.penMask |= PEN_MASK_TILT_X | PEN_MASK_TILT_Y;
            pen.tiltX = ((-angle).sin() * tilt.sin()).atan2(tilt.cos()).to_degrees() as i32;
            pen.tiltY = ((-angle).cos() * tilt.sin()).atan2(tilt.cos()).to_degrees() as i32;
        }
    }
    Some(pen)
}
/// The frame that lifts an active pen, as a client's cancel-all does.
fn pen_cancel(pen: POINTER_PEN_INFO) -> Option<POINTER_PEN_INFO> {
    if pen.pointerInfo.pointerFlags == POINTER_FLAG_NONE {
        return None;
    }
    pen_frame(
        pen,
        7,
        0,
        0,
        POINT::default(),
        0.,
        u16::MAX,
        u8::MAX,
        u16::MAX,
    )
}
/// The pen after a frame: edge flags last one frame. A pen that ended, or
/// that Windows refused to put down and so never saw, is inactive whether or
/// not the frame went through; otherwise the refresh would inject the stale
/// frame again every 250 ms.
fn pen_after(mut pen: POINTER_PEN_INFO, event: u8, failed: bool) -> POINTER_PEN_INFO {
    let flags = pen.pointerInfo.pointerFlags;
    pen.pointerInfo.pointerFlags = if matches!(event, 2 | 4 | 6 | 7)
        || (failed && flags & POINTER_FLAG_DOWN != POINTER_FLAG_NONE)
    {
        POINTER_FLAG_NONE
    } else {
        (flags & !(POINTER_FLAG_DOWN | POINTER_FLAG_UP | POINTER_FLAG_CANCELED))
            | POINTER_FLAG_UPDATE
    };
    pen
}

#[cfg(test)]
mod tests {
    use super::*;
    fn touch_frame(
        touches: &BTreeMap<u32, POINTER_TOUCH_INFO>,
        id: u32,
        event: u8,
        location: POINT,
    ) -> Option<POINTER_TOUCH_INFO> {
        let mut frame = touch_start(touches, id, event).unwrap()?;
        pointer_event(&mut frame.pointerInfo, event, location);
        Some(frame)
    }
    #[test]
    fn a_touch_move_needs_a_successful_down_and_refused_contacts_own_no_slot() {
        let mut touches = BTreeMap::new();
        let location = POINT { x: 10, y: 20 };
        // A lost down, or a move arriving after cancel-all, must not poison
        // every later frame with an UPDATE for an unknown Windows contact.
        assert!(touch_frame(&touches, 8, 3, location).is_none());
        for event in [0, 1] {
            let frame = touch_frame(&touches, 8, event, location).unwrap();
            assert!(
                submit_touch(&mut touches, 8, frame, event, |_| {
                    bail!("injection refused")
                })
                .is_err()
            );
            assert!(touches.is_empty());
            assert_eq!(
                touch_start(&touches, 9, 1)
                    .unwrap()
                    .unwrap()
                    .pointerInfo
                    .pointerId,
                1
            );
        }
        let hover = touch_frame(&touches, 8, 0, location).unwrap();
        submit_touch(&mut touches, 8, hover, 0, |_| Ok(())).unwrap();
        assert!(touch_frame(&touches, 8, 3, location).is_none());
        let down = touch_frame(&touches, 8, 1, location).unwrap();
        assert_ne!(
            down.pointerInfo.pointerFlags & POINTER_FLAG_DOWN,
            POINTER_FLAG_NONE
        );
        submit_touch(&mut touches, 8, down, 1, |_| Ok(())).unwrap();
        assert!(touch_frame(&touches, 8, 3, location).is_some());
    }
    #[test]
    fn a_refused_move_keeps_the_last_accepted_position_for_release() {
        let mut touches = BTreeMap::new();
        let original = POINT { x: 10, y: 20 };
        let down = touch_frame(&touches, 8, 1, original).unwrap();
        submit_touch(&mut touches, 8, down, 1, |_| Ok(())).unwrap();
        let moved = touch_frame(&touches, 8, 3, POINT { x: 30, y: 40 }).unwrap();
        assert!(submit_touch(&mut touches, 8, moved, 3, |_| bail!("busy")).is_err());
        assert_eq!(touches[&8].pointerInfo.ptPixelLocation, original);
        let up = touch_frame(&touches, 8, 2, POINT::default()).unwrap();
        assert_eq!(up.pointerInfo.ptPixelLocation, original);
        // A failed up retains the accepted contact for cancellation fallback.
        assert!(submit_touch(&mut touches, 8, up, 2, |_| bail!("busy")).is_err());
        assert_eq!(touches.len(), 1);
        submit_touch(&mut touches, 8, up, 2, |_| Ok(())).unwrap();
        assert!(touches.is_empty());
    }
    #[test]
    fn failed_batch_cancellation_attempts_every_contact_and_clears_state() {
        let mut touches = BTreeMap::new();
        for id in 1..=3 {
            let frame = touch_frame(
                &touches,
                id,
                1,
                POINT {
                    x: id as i32,
                    y: 20,
                },
            )
            .unwrap();
            submit_touch(&mut touches, id, frame, 1, |_| Ok(())).unwrap();
        }
        let mut attempted = Vec::new();
        let result = cancel_touch_contacts(&mut touches, |contacts| {
            let ids: Vec<_> = contacts
                .iter()
                .map(|contact| {
                    // SAFETY: cancel_touch_contacts creates initialized touch frames.
                    let touch = unsafe { contact.Anonymous.touchInfo };
                    assert_eq!(
                        touch.pointerInfo.ptPixelLocation.x,
                        touch.pointerInfo.pointerId as i32
                    );
                    assert_eq!(
                        touch.pointerInfo.pointerFlags,
                        POINTER_FLAG_UP | POINTER_FLAG_CANCELED
                    );
                    touch.pointerInfo.pointerId
                })
                .collect();
            let rejected = ids.contains(&2);
            attempted.push(ids);
            if rejected {
                bail!("stale contact");
            }
            Ok(())
        });
        assert!(result.is_err());
        assert_eq!(attempted, [vec![1, 2, 3], vec![1], vec![2], vec![3]]);
        assert!(touches.is_empty());
        assert!(touch_frame(&touches, 1, 3, POINT::default()).is_none());
        cancel_touch_contacts(&mut touches, |_| panic!("empty state must not inject")).unwrap();
    }
    #[test]
    fn cancellation_uses_one_batch_when_accepted_and_can_recover_a_failed_batch() {
        for fail_batch in [false, true] {
            let mut touches = BTreeMap::new();
            for id in 1..=2 {
                let hover = touch_frame(&touches, id, 0, POINT::default()).unwrap();
                submit_touch(&mut touches, id, hover, 0, |_| Ok(())).unwrap();
            }
            let mut calls = 0;
            cancel_touch_contacts(&mut touches, |contacts| {
                calls += 1;
                if fail_batch && calls == 1 {
                    bail!("batch refused");
                }
                for contact in contacts {
                    // SAFETY: The helper constructs only initialized touch frames.
                    let flags = unsafe { contact.Anonymous.touchInfo.pointerInfo.pointerFlags };
                    assert_eq!(flags, POINTER_FLAG_UPDATE | POINTER_FLAG_CANCELED);
                }
                Ok(())
            })
            .unwrap();
            assert_eq!(calls, if fail_batch { 3 } else { 1 });
            assert!(touches.is_empty());
        }
    }
    #[test]
    fn native_pointer_rectangles_use_the_virtual_screen_origin() {
        let primary = RECT {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        };
        assert_eq!(pointer_rect(primary, POINT::default()), primary);
        let origin = POINT { x: -1920, y: -1080 };
        assert_eq!(
            pointer_rect(primary, origin),
            RECT {
                left: 1920,
                top: 1080,
                right: 3840,
                bottom: 2160
            }
        );
        assert_eq!(
            pointer_rect(
                RECT {
                    left: -1920,
                    top: 0,
                    right: 0,
                    bottom: 1080
                },
                origin
            ),
            RECT {
                left: 0,
                top: 1080,
                right: 1920,
                bottom: 2160
            }
        );
        assert_eq!(
            pointer_rect(
                RECT {
                    left: 0,
                    top: -1080,
                    right: 1920,
                    bottom: 0
                },
                origin
            ),
            RECT {
                left: 1920,
                top: 0,
                right: 3840,
                bottom: 1080
            }
        );
    }
    #[test]
    fn native_pointer_locations_keep_letterboxing_and_display_edges() {
        let mut injector = Injector::new(r"\\.\DISPLAY99", "vhf").unwrap();
        let display = RECT {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        };
        injector.rect = Some(display);
        injector.set_stream_size(1920, 1440);
        for origin in [POINT::default(), POINT { x: -1920, y: -1080 }] {
            let rect = pointer_rect(display, origin);
            assert_eq!(
                injector.location(rect, 0., 0.125),
                POINT {
                    x: rect.left,
                    y: rect.top
                }
            );
            assert_eq!(
                injector.location(rect, 1., 0.875),
                POINT {
                    x: rect.right - 1,
                    y: rect.bottom - 1
                }
            );
            assert_eq!(
                injector.location(rect, 0.5, 0.5),
                POINT {
                    x: 959 - origin.x,
                    y: 539 - origin.y
                }
            );
            assert_eq!(
                injector.location(rect, 0., 0.),
                POINT {
                    x: rect.left,
                    y: rect.top
                }
            );
            assert_eq!(
                injector.location(rect, 1., 1.),
                POINT {
                    x: rect.right - 1,
                    y: rect.bottom - 1
                }
            );
        }
    }
    #[test]
    fn an_out_of_range_contact_size_stays_inside_the_display() {
        let rect = RECT {
            left: -1920,
            top: 0,
            right: 0,
            bottom: 1080,
        };
        let center = POINT { x: -960, y: 540 };
        let angle = 45.0f32.to_radians();
        for size in [1e30, f32::INFINITY, f32::MAX] {
            assert_eq!(
                contact_rect(center, rect, (size, size), angle, (1., 1.)),
                rect
            );
        }
        let point = contact_rect(center, rect, (f32::NAN, f32::NAN), angle, (1., 1.));
        assert_eq!((point.left, point.right), (-961, -959));
        assert_eq!((point.top, point.bottom), (539, 541));
        let edge = contact_rect(POINT { x: 0, y: 1080 }, rect, (1e30, 0.), 0., (1., 1.));
        assert_eq!((edge.right, edge.bottom), (0, 1080));
        let normal = contact_rect(center, rect, (0.25, 0.25), 0., (1., 1.));
        assert_eq!((normal.left, normal.right), (-1200, -720));
        assert_eq!((normal.top, normal.bottom), (405, 675));
    }
    #[test]
    fn a_held_touch_is_repeated_while_pen_events_keep_coming() {
        use std::time::{Duration, Instant};
        let start = Instant::now();
        // A touch is held still while the pen hovers, sending an event every
        // 8 ms (each injection restarts the pen's period), for one second.
        let mut touch = start;
        let mut repeats = vec![];
        for ms in (8..=1000).step_by(8) {
            let now = start + Duration::from_millis(ms);
            let mut pen = now;
            assert!(!refresh_due(&mut pen, now));
            if refresh_due(&mut touch, now) {
                repeats.push(ms);
            }
        }
        assert_eq!(repeats, [256, 512, 768]);
        // And a hovering pen is repeated while touch events keep coming.
        let mut pen = start;
        let mut repeats = 0;
        for ms in (8..=1000).step_by(8) {
            let now = start + Duration::from_millis(ms);
            let mut touch = now;
            assert!(!refresh_due(&mut touch, now));
            repeats += usize::from(refresh_due(&mut pen, now));
        }
        assert_eq!(repeats, 3);
    }
    fn pen_event(pen: POINTER_PEN_INFO, event: u8, pressure: f32) -> Option<POINTER_PEN_INFO> {
        let location = POINT { x: 10, y: 20 };
        pen_frame(
            pen,
            event,
            1,
            0,
            location,
            pressure,
            u16::MAX,
            u8::MAX,
            u16::MAX,
        )
    }
    #[test]
    fn pen_pressure_and_button_frames_match_the_cpp_host() {
        let idle = POINTER_PEN_INFO::default();
        let in_contact = POINTER_FLAG_UPDATE | POINTER_FLAG_INRANGE | POINTER_FLAG_INCONTACT;
        // A button change with no active pen injects nothing.
        assert!(
            pen_frame(
                idle,
                5,
                1,
                1,
                POINT::default(),
                0.,
                u16::MAX,
                u8::MAX,
                u16::MAX
            )
            .is_none()
        );
        // A pressure of 0 in contact is passed as none.
        let down = pen_event(idle, 1, 0.).unwrap();
        assert_eq!(
            down.pointerInfo.pointerFlags,
            POINTER_FLAG_DOWN | POINTER_FLAG_INRANGE | POINTER_FLAG_INCONTACT
        );
        assert_eq!((down.penMask, down.pressure), (PEN_MASK_NONE, 0));
        let held = pen_after(down, 1, false);
        assert_eq!(held.pointerInfo.pointerFlags, in_contact);
        let moved = pen_event(held, 3, 0.).unwrap();
        assert_eq!((moved.penMask, moved.pressure), (PEN_MASK_NONE, 0));
        let pressed = pen_event(held, 3, 0.5).unwrap();
        assert_eq!(
            (pressed.penMask, pressed.pressure),
            (PEN_MASK_PRESSURE, 512)
        );
        // A hover's distance is never passed as pressure.
        let hover = pen_event(idle, 0, 0.5).unwrap();
        assert_eq!((hover.penMask, hover.pressure), (PEN_MASK_NONE, 0));
        // With an active pen, a button change is passed on.
        let hovering = pen_after(hover, 0, false);
        let barrel = pen_frame(
            hovering,
            5,
            1,
            1,
            POINT::default(),
            0.,
            u16::MAX,
            u8::MAX,
            u16::MAX,
        )
        .unwrap();
        assert_eq!(barrel.penFlags, PEN_FLAG_BARREL);
        assert_eq!(
            barrel.pointerInfo.pointerFlags,
            POINTER_FLAG_UPDATE | POINTER_FLAG_INRANGE
        );
        assert_eq!(barrel.pointerInfo.ptPixelLocation, POINT { x: 10, y: 20 });
    }
    /// A known barrel roll is injected as the pen's twist, and the tilt still follows the
    /// tilt's direction; an unknown roll keeps the stock mapping.
    #[test]
    fn a_barrel_roll_is_injected_as_the_twist() {
        let frame = |rotation, tilt, roll| {
            let location = POINT { x: 10, y: 20 };
            pen_frame(
                POINTER_PEN_INFO::default(),
                1,
                1,
                0,
                location,
                0.5,
                rotation,
                tilt,
                roll,
            )
            .unwrap()
        };
        let tilt = PEN_MASK_TILT_X | PEN_MASK_TILT_Y;
        let stock = frame(90, 30, u16::MAX);
        assert_eq!(stock.penMask, PEN_MASK_PRESSURE | PEN_MASK_ROTATION | tilt);
        assert_eq!(stock.rotation, 90);
        for roll in [0, 45, 359] {
            let rolled = frame(90, 30, roll);
            assert_eq!(rolled.penMask, stock.penMask, "{roll}");
            assert_eq!(rolled.rotation, u32::from(roll));
            assert_eq!((rolled.tiltX, rolled.tiltY), (stock.tiltX, stock.tiltY));
            // With no tilt's direction the roll is still passed, without a tilt.
            let upright = frame(u16::MAX, u8::MAX, roll);
            assert_eq!(upright.penMask, PEN_MASK_PRESSURE | PEN_MASK_ROTATION);
            assert_eq!(upright.rotation, u32::from(roll));
        }
        assert_eq!(
            frame(u16::MAX, u8::MAX, u16::MAX).penMask,
            PEN_MASK_PRESSURE
        );
    }
    #[test]
    fn a_pen_frame_windows_refuses_is_not_repeated_as_a_stale_frame() {
        let in_contact = POINTER_FLAG_UPDATE | POINTER_FLAG_INRANGE | POINTER_FLAG_INCONTACT;
        let down = pen_event(POINTER_PEN_INFO::default(), 1, 0.5).unwrap();
        // Windows never saw the pen go down: there is nothing to repeat.
        assert_eq!(
            pen_after(down, 1, true).pointerInfo.pointerFlags,
            POINTER_FLAG_NONE
        );
        let held = pen_after(down, 1, false);
        // An ended pen is inactive whether or not its last frame went through.
        for event in [2, 4, 6, 7] {
            for failed in [false, true] {
                let frame = pen_event(held, event, 0.).unwrap();
                assert_eq!(
                    pen_after(frame, event, failed).pointerInfo.pointerFlags,
                    POINTER_FLAG_NONE
                );
            }
        }
        // A refused move keeps the pen's state, without edge flags.
        let moved = pen_event(held, 3, 0.5).unwrap();
        assert_eq!(
            pen_after(moved, 3, true).pointerInfo.pointerFlags,
            in_contact
        );
    }
    #[test]
    fn an_active_pen_is_lifted_before_its_device_goes_away() {
        let idle = POINTER_PEN_INFO::default();
        assert!(pen_cancel(idle).is_none());
        let held = pen_after(pen_event(idle, 1, 0.5).unwrap(), 1, false);
        let lifted = pen_cancel(held).unwrap();
        assert_eq!(
            lifted.pointerInfo.pointerFlags,
            POINTER_FLAG_UP | POINTER_FLAG_CANCELED
        );
        assert_eq!((lifted.penMask, lifted.pressure), (PEN_MASK_NONE, 0));
        assert_eq!(
            pen_after(lifted, 7, true).pointerInfo.pointerFlags,
            POINTER_FLAG_NONE
        );
        let hovering = pen_after(pen_event(idle, 0, 0.).unwrap(), 0, false);
        assert_eq!(
            pen_cancel(hovering).unwrap().pointerInfo.pointerFlags,
            POINTER_FLAG_UPDATE | POINTER_FLAG_CANCELED
        );
    }
}
