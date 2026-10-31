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
mod tests;
