//! Keyboard and mouse injection, batching, and held-key ownership.

use super::*;

impl Injector {
    /// Inject input now.
    fn send(&self, inputs: &[INPUT]) -> Result<()> {
        if (self.inject)(inputs) < inputs.len() {
            bail!(
                "Windows input injection failed: {}",
                std::io::Error::last_os_error()
            );
        }
        Ok(())
    }
    /// Send the pass's keyboard and mouse input, and record the presses
    /// Windows took. A refused press is not held.
    pub(super) fn flush(&mut self) {
        if self.pending.inputs.is_empty() {
            return;
        }
        let sent = (self.inject)(&self.pending.inputs);
        let refused =
            (sent < self.pending.inputs.len()).then(|| std::io::Error::last_os_error().to_string());
        self.pending.inputs.clear();
        let mut events = std::mem::take(&mut self.pending.events);
        for (end, press) in events.drain(..) {
            if end <= sent {
                if let Some(press) = press {
                    self.hold(press);
                }
                continue;
            }
            if let Some(press) = press
                && press.counted
            {
                let mut held = HELD.lock().unwrap();
                let identity = (press.keyboard, press.id);
                match held.get(&identity).copied().unwrap_or(0) {
                    0 | 1 => held.remove(&identity),
                    count => held.insert(identity, count - 1),
                };
            }
            self.pending.errors.push(anyhow::anyhow!(
                "Windows input injection failed: {}",
                refused.as_deref().unwrap_or_default()
            ));
        }
        self.pending.events = events;
    }
    /// Record a press Windows took.
    pub(super) fn hold(&mut self, press: Press) {
        if !press.keyboard {
            self.buttons.insert(press.id as u8);
            return;
        }
        self.keys.insert(press.id);
        self.key_sources.insert(press.source, press.id);
        self.key_flags.insert(press.id, press.flags);
        if let Some(modifiers) = press.repeat
            && let Some(delay) = self.policy.repeat_delay
        {
            self.repeat = Some((
                press.id,
                press.flags,
                modifiers,
                std::time::Instant::now() + delay,
            ));
        }
    }
    /// Whether a key (or button) press of the pass is still unsent: what
    /// recording it changes decides how a later key (or button) is sent.
    pub(super) fn press_pending(&self, keyboard: bool) -> bool {
        self.pending
            .events
            .iter()
            .any(|(_, press)| press.as_ref().is_some_and(|p| p.keyboard == keyboard))
    }
    pub(super) fn key(key: u16, down: bool, unicode: bool) -> INPUT {
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VIRTUAL_KEY(if unicode { 0 } else { key }),
                    wScan: if unicode { key } else { 0 },
                    dwFlags: (if down {
                        KEYBD_EVENT_FLAGS(0)
                    } else {
                        KEYEVENTF_KEYUP
                    }) | if unicode {
                        KEYEVENTF_UNICODE
                    } else if legacy_extended_key(key) {
                        KEYEVENTF_EXTENDEDKEY
                    } else {
                        KEYBD_EVENT_FLAGS(0)
                    },
                    ..Default::default()
                },
            },
        }
    }
    pub(super) fn key_scan(&self, key: u32, down: bool, flags: u8) -> INPUT {
        Self::keyboard_input(
            key,
            down,
            flags,
            self.policy.always_send_scancodes,
            self.layout.as_ref(),
        )
    }
    fn keyboard_input(
        key: u32,
        down: bool,
        flags: u8,
        always_send_scancodes: bool,
        layout: Option<&crate::keylayout::Layout>,
    ) -> INPUT {
        let extended = key & EXPLICIT_EXTENDED_KEY != 0;
        let key = key as u16;
        let mut input = Self::key(key, down, false);
        if extended {
            // SAFETY: Self::key constructed a keyboard INPUT, so its ki union field is initialized.
            unsafe { input.Anonymous.ki.dwFlags |= KEYEVENTF_EXTENDEDKEY };
        }
        // Normalized Moonlight VKs always use the fixed US table. Non-normalized
        // keys get scan codes only when the administrator asks for it: from the
        // device's layout when it is known, else from the host's.
        let scan = if key == 0x2c && (flags & 1 == 0 || always_send_scancodes) {
            // Print Screen is E0 37. The retained table's SysRq scan 54
            // stops working when a newer client marks it extended.
            0x37
        } else if flags & 1 == 0 {
            crate::keylayout::SCANCODES[(key & 255) as usize] as u16
        } else if always_send_scancodes && !matches!(key, 0x5b | 0x5c | 0x13) {
            match layout.and_then(|layout| layout.scancode(key)) {
                Some(scan) => u16::from(scan),
                // SAFETY: MapVirtualKeyW takes a virtual-key value and a valid mapping constant, with no borrowed pointers.
                None => unsafe { MapVirtualKeyW(u32::from(key), MAPVK_VK_TO_VSC) as u16 },
            }
        } else {
            0
        };
        if scan != 0 {
            // SAFETY: Self::key constructed a keyboard INPUT, so its ki union field is initialized.
            unsafe {
                input.Anonymous.ki.wVk = VIRTUAL_KEY(0);
                input.Anonymous.ki.wScan = scan;
                input.Anonymous.ki.dwFlags |= KEYEVENTF_SCANCODE;
            }
        }
        input
    }
    pub(super) fn mouse(dx: i32, dy: i32, data: u32, flags: MOUSE_EVENT_FLAGS) -> INPUT {
        INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx,
                    dy,
                    mouseData: data,
                    dwFlags: flags,
                    ..Default::default()
                },
            },
        }
    }
    pub(super) fn button(button: u8, down: bool) -> INPUT {
        let flag = match (button, down) {
            (1, true) => MOUSEEVENTF_LEFTDOWN,
            (1, false) => MOUSEEVENTF_LEFTUP,
            (2, true) => MOUSEEVENTF_MIDDLEDOWN,
            (2, false) => MOUSEEVENTF_MIDDLEUP,
            (3, true) => MOUSEEVENTF_RIGHTDOWN,
            (3, false) => MOUSEEVENTF_RIGHTUP,
            (_, true) => MOUSEEVENTF_XDOWN,
            (_, false) => MOUSEEVENTF_XUP,
        };
        Self::mouse(
            0,
            0,
            if button >= 4 {
                u32::from(button - 3)
            } else {
                0
            },
            flag,
        )
    }
    pub fn next_due(&self) -> Option<std::time::Instant> {
        self.left_release
            .into_iter()
            .chain(self.repeat.map(|(_, _, _, due)| due))
            .min()
    }
    /// Delayed releases and key repeat have their own deadlines; waiting for
    /// the 8 ms feedback tick would make a 10 ms release take up to 18 ms.
    pub fn due(&mut self) -> Result<()> {
        let now = std::time::Instant::now();
        let mut first = None;
        if self.left_release.is_some_and(|due| now >= due) {
            self.left_release = None;
            if let Err(error) = self.held(false, 1, false, true, Self::button(1, false)) {
                first.get_or_insert(error);
            }
        }
        if let Some((key, flags, modifiers, due)) = self.repeat
            && now >= due
        {
            self.repeat = Some((key, flags, modifiers, now + self.policy.repeat_period));
            let inputs = Self::repeat_inputs(
                key,
                flags,
                modifiers,
                &self.keys,
                &HELD.lock().unwrap(),
                self.policy.always_send_scancodes,
                self.layout.as_ref(),
            );
            if let Err(error) = self.send(&inputs) {
                first.get_or_insert(error);
            }
        }
        first.map_or(Ok(()), Err)
    }
    /// Modifiers in a key-down packet that neither this client nor another
    /// holds as keys (Moonlight reports Shift/Ctrl/Alt both ways).
    pub(super) fn synthetic_modifiers(&self, key: u32, reported: u8) -> u8 {
        unheld_modifiers(key, reported, &self.keys, &HELD.lock().unwrap())
    }
    /// The key's input surrounded by temporary presses of `modifiers`.
    pub(super) fn with_modifiers(
        key: INPUT,
        modifiers: u8,
        always_send_scancodes: bool,
    ) -> Vec<INPUT> {
        let generic = [
            (MODIFIER_SHIFT, 0x10),
            (MODIFIER_CTRL, 0x11),
            (MODIFIER_ALT, 0x12),
        ];
        let mut inputs = Vec::with_capacity(7);
        for (mask, vk) in generic {
            if modifiers & mask != 0 {
                inputs.push(Self::keyboard_input(
                    vk,
                    true,
                    0,
                    always_send_scancodes,
                    None,
                ));
            }
        }
        inputs.push(key);
        for (mask, vk) in generic.iter().rev() {
            if modifiers & mask != 0 {
                inputs.push(Self::keyboard_input(
                    *vk,
                    false,
                    0,
                    always_send_scancodes,
                    None,
                ));
            }
        }
        inputs
    }
    /// One repeat of a held key. The modifiers pressed around it at key-down
    /// are pressed again only while still no client holds them: the repeat
    /// must not release a Shift the user pressed since.
    fn repeat_inputs(
        key: u32,
        flags: u8,
        modifiers: u8,
        keys: &BTreeSet<u32>,
        held: &BTreeMap<(bool, u32), usize>,
        always_send_scancodes: bool,
        layout: Option<&crate::keylayout::Layout>,
    ) -> Vec<INPUT> {
        Self::with_modifiers(
            Self::keyboard_input(key, true, flags, always_send_scancodes, layout),
            unheld_modifiers(key, modifiers, keys, held),
            always_send_scancodes,
        )
    }
    pub(super) fn held(
        &self,
        keyboard: bool,
        id: u32,
        down: bool,
        owned: bool,
        input: INPUT,
    ) -> Result<()> {
        Self::update_held(
            &mut HELD.lock().unwrap(),
            (keyboard, id),
            down,
            owned,
            input,
            |inputs| self.send(inputs),
        )
    }
    /// Teardown gets one extra release attempt while ownership remains
    /// locked. A new client cannot press between the failed up and its retry.
    pub(super) fn release_held(
        held: &mut BTreeMap<(bool, u32), usize>,
        identity: (bool, u32),
        input: INPUT,
        mut send: impl FnMut(&[INPUT]) -> Result<()>,
    ) -> Result<()> {
        Self::update_held(held, identity, false, true, input, |inputs| {
            send(inputs).or_else(|_| send(inputs))
        })
    }
    pub(super) fn release_owned(&self, keyboard: bool, id: u32, input: INPUT) -> Result<()> {
        Self::release_held(&mut HELD.lock().unwrap(), (keyboard, id), input, |inputs| {
            self.send(inputs)
        })
    }
    pub(super) fn update_held(
        held: &mut BTreeMap<(bool, u32), usize>,
        identity: (bool, u32),
        down: bool,
        owned: bool,
        input: INPUT,
        send: impl FnOnce(&[INPUT]) -> Result<()>,
    ) -> Result<()> {
        let count = held.get(&identity).copied().unwrap_or(0);
        if down {
            // A repeated press of a key or button this client already holds
            // is not sent again, as in Vibepollo: with the host's own repeat
            // it doubled the repeat rate, and a second button-down could
            // become a double-click.
            if count == 0 && !owned {
                send(&[input])?;
            }
            if !owned {
                held.insert(identity, count + 1);
            }
        } else if owned {
            if count <= 1 {
                // Forget the key first: a release Windows refused must not
                // keep it held for every later press and session.
                held.remove(&identity);
                send(&[input])?;
            } else {
                held.insert(identity, count - 1);
            }
        }
        Ok(())
    }
}
/// Inject input, following the input desktop when Windows refuses it: a UAC
/// prompt or the lock screen runs on the secure desktop. Returns how many
/// inputs Windows took, which it takes in order.
pub(super) fn inject(inputs: &[INPUT]) -> usize {
    // SAFETY: The INPUT slice is initialized, remains live throughout SendInput, and uses the exact INPUT structure size.
    let send = |inputs: &[INPUT]| unsafe { SendInput(inputs, size_of::<INPUT>() as i32) };
    let mut sent = send(inputs) as usize;
    if sent < inputs.len() && follow_input_desktop() {
        sent += send(&inputs[sent..]) as usize;
    }
    sent
}
// Moonlight 6.2 adds MODIFIER_EXTENDED (0x10) for keys such as keypad
// Enter that share a VK with a non-extended key. Keep this identity through
// held-key ownership, repeats and disconnect cleanup without changing the VK.
const MODIFIER_EXTENDED: u8 = 0x10;
pub(super) const EXPLICIT_EXTENDED_KEY: u32 = 1 << 16;
fn legacy_extended_key(key: u16) -> bool {
    // Older clients do not mark these VKs extended. Without E0, media
    // scans become letter keys and browser scans become function keys.
    matches!(
        key,
        0x21..=0x28 | 0x2c..=0x2e | 0xa3 | 0xa5..=0xb7 | 0x5b | 0x5c | 0x5d | 0x6f
    )
}
pub(super) fn mapped_keyboard_identity(
    policy: &butterpollo_core::input_policy::Policy,
    key: u16,
    modifiers: u8,
) -> u32 {
    let mapped = policy.key(key);
    // A remapping chooses the destination key's identity; do not turn it
    // into an extended key merely because the source was.
    keyboard_identity(mapped, if mapped == key { modifiers } else { 0 })
}
pub(super) fn keyboard_identity(key: u16, modifiers: u8) -> u32 {
    u32::from(key)
        | if modifiers & MODIFIER_EXTENDED != 0 && !legacy_extended_key(key) {
            EXPLICIT_EXTENDED_KEY
        } else {
            0
        }
}
const MODIFIER_SHIFT: u8 = 0x01;
const MODIFIER_CTRL: u8 = 0x02;
const MODIFIER_ALT: u8 = 0x04;
pub(super) fn is_modifier(key: u32) -> bool {
    matches!(key as u16, 0x10..=0x12 | 0xa0..=0xa5 | 0x5b | 0x5c)
}
fn modifier_held(
    keys: &BTreeSet<u32>,
    held: &BTreeMap<(bool, u32), usize>,
    alternatives: [u32; 3],
) -> bool {
    keys.iter()
        .any(|key| alternatives.contains(&(key & 0xffff)))
        || held
            .keys()
            .any(|(keyboard, key)| *keyboard && alternatives.contains(&(key & 0xffff)))
}
/// The Shift, Ctrl and Alt of `modifiers` that no client holds as keys, to
/// press around `key`; none around a modifier.
fn unheld_modifiers(
    key: u32,
    modifiers: u8,
    keys: &BTreeSet<u32>,
    held: &BTreeMap<(bool, u32), usize>,
) -> u8 {
    if is_modifier(key) {
        return 0;
    }
    let mut synthetic = 0;
    for (mask, alternatives) in [
        (MODIFIER_SHIFT, [0x10, 0xa0, 0xa1]),
        (MODIFIER_CTRL, [0x11, 0xa2, 0xa3]),
        (MODIFIER_ALT, [0x12, 0xa4, 0xa5]),
    ] {
        if modifiers & mask != 0 && !modifier_held(keys, held, alternatives) {
            synthetic |= mask;
        }
    }
    synthetic
}

#[cfg(test)]
mod tests;
