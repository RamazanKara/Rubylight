//! Pure chord matcher for live mid-stream monitor switching (`Ctrl + Alt + Shift + F1`–`F12`).

/// Moonlight's keyboard modifier flags (matching `couch_menu::chord`).
pub const MODIFIER_SHIFT: u8 = 0x01;
pub const MODIFIER_CTRL: u8 = 0x02;
pub const MODIFIER_ALT: u8 = 0x04;
pub const MODIFIER_META: u8 = 0x08;

/// Windows virtual key codes recognized by the monitor-switch chord matcher.
pub const VK_F1: u16 = 0x70;
pub const VK_F12: u16 = 0x7B;

pub const VK_SHIFT: u16 = 0x10;
pub const VK_CONTROL: u16 = 0x11;
pub const VK_MENU: u16 = 0x12;
pub const VK_LWIN: u16 = 0x5B;
pub const VK_RWIN: u16 = 0x5C;
pub const VK_LSHIFT: u16 = 0xA0;
pub const VK_RSHIFT: u16 = 0xA1;
pub const VK_LCONTROL: u16 = 0xA2;
pub const VK_RCONTROL: u16 = 0xA3;
pub const VK_LMENU: u16 = 0xA4;
pub const VK_RMENU: u16 = 0xA5;

/// Outcome for one decoded keyboard event in [`Matcher::on_key`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Forward the key event to the guest/host input injector.
    Pass,
    /// Suppress the key event (e.g. swallowed `F1`–`F12` repeat or key-up).
    Swallow,
    /// Trigger a live monitor switch to the 1-based ordered display index `1..=12`.
    Switch(u8),
}

/// Stateful per-peer detector for `Ctrl + Alt + Shift + F1`–`F12`.
///
/// Tracks left/right/generic modifier virtual keys across packets alongside
/// the event's `modifiers` bitmask, always passes modifier key events through so
/// Windows and games never observe stuck modifiers, and suppresses repeats and
/// releases of matched `F1`–`F12` keys.
#[derive(Default, Debug)]
pub struct Matcher {
    shift_vks: u8,
    ctrl_vks: u8,
    alt_vks: u8,
    meta_vks: u8,
    swallowed_fkeys: u16,
}

impl Matcher {
    /// Evaluates one keyboard event and returns whether to pass it through,
    /// swallow it, or trigger a switch to 1-based display `1..=12`.
    pub fn on_key(&mut self, key: u16, modifiers: u8, down: bool, allowed: bool) -> Action {
        if self.update_modifier(key, down) {
            return Action::Pass;
        }
        if !(VK_F1..=VK_F12).contains(&key) {
            return Action::Pass;
        }
        let idx = (key - VK_F1) as u8;
        let bit = 1u16 << idx;
        if !down {
            if (self.swallowed_fkeys & bit) != 0 {
                self.swallowed_fkeys &= !bit;
                return Action::Swallow;
            }
            return Action::Pass;
        }
        if !allowed {
            return Action::Pass;
        }
        if (self.swallowed_fkeys & bit) != 0 {
            return Action::Swallow;
        }
        let effective = self.tracked_modifiers() | (modifiers & 0x0F);
        let required = MODIFIER_CTRL | MODIFIER_ALT | MODIFIER_SHIFT;
        if (effective & required) == required && (effective & MODIFIER_META) == 0 {
            self.swallowed_fkeys |= bit;
            Action::Switch(idx + 1)
        } else {
            Action::Pass
        }
    }

    /// Clears all tracked modifier key state and swallowed F-key bits.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    fn tracked_modifiers(&self) -> u8 {
        let mut out = 0u8;
        if self.shift_vks != 0 {
            out |= MODIFIER_SHIFT;
        }
        if self.ctrl_vks != 0 {
            out |= MODIFIER_CTRL;
        }
        if self.alt_vks != 0 {
            out |= MODIFIER_ALT;
        }
        if self.meta_vks != 0 {
            out |= MODIFIER_META;
        }
        out
    }

    fn update_modifier(&mut self, key: u16, down: bool) -> bool {
        let (slot, bit) = match key {
            VK_SHIFT => (&mut self.shift_vks, 1 << 0),
            VK_LSHIFT => (&mut self.shift_vks, 1 << 1),
            VK_RSHIFT => (&mut self.shift_vks, 1 << 2),
            VK_CONTROL => (&mut self.ctrl_vks, 1 << 0),
            VK_LCONTROL => (&mut self.ctrl_vks, 1 << 1),
            VK_RCONTROL => (&mut self.ctrl_vks, 1 << 2),
            VK_MENU => (&mut self.alt_vks, 1 << 0),
            VK_LMENU => (&mut self.alt_vks, 1 << 1),
            VK_RMENU => (&mut self.alt_vks, 1 << 2),
            VK_LWIN => (&mut self.meta_vks, 1 << 0),
            VK_RWIN => (&mut self.meta_vks, 1 << 1),
            _ => return false,
        };
        if down {
            *slot |= bit;
        } else {
            *slot &= !bit;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CTRL_ALT_SHIFT: u8 = MODIFIER_CTRL | MODIFIER_ALT | MODIFIER_SHIFT;

    #[test]
    fn matches_all_twelve_fkeys_with_modifier_bitmask() {
        let mut matcher = Matcher::default();
        for offset in 0u16..12 {
            let vk = VK_F1 + offset;
            assert_eq!(
                matcher.on_key(vk, CTRL_ALT_SHIFT, true, true),
                Action::Switch((offset + 1) as u8)
            );
            assert_eq!(
                matcher.on_key(vk, CTRL_ALT_SHIFT, true, true),
                Action::Swallow
            );
            assert_eq!(
                matcher.on_key(vk, CTRL_ALT_SHIFT, false, true),
                Action::Swallow
            );
            // Subsequent release without an active swallow passes through.
            assert_eq!(
                matcher.on_key(vk, CTRL_ALT_SHIFT, false, true),
                Action::Pass
            );
        }
    }

    #[test]
    fn tracks_left_right_generic_modifier_vks_and_passes_modifier_keys() {
        let mut matcher = Matcher::default();
        assert_eq!(matcher.on_key(VK_LCONTROL, 0, true, true), Action::Pass);
        assert_eq!(matcher.on_key(VK_RMENU, 0, true, true), Action::Pass);
        assert_eq!(matcher.on_key(VK_LSHIFT, 0, true, true), Action::Pass);
        assert_eq!(matcher.on_key(VK_RSHIFT, 0, true, true), Action::Pass);

        // Releasing LShift leaves RShift held, so the chord still fires.
        assert_eq!(matcher.on_key(VK_LSHIFT, 0, false, true), Action::Pass);
        assert_eq!(matcher.on_key(VK_F1 + 2, 0, true, true), Action::Switch(3));

        // Modifiers released before F3 up: F3 up is still swallowed.
        assert_eq!(matcher.on_key(VK_LCONTROL, 0, false, true), Action::Pass);
        assert_eq!(matcher.on_key(VK_RMENU, 0, false, true), Action::Pass);
        assert_eq!(matcher.on_key(VK_RSHIFT, 0, false, true), Action::Pass);
        assert_eq!(matcher.on_key(VK_F1 + 2, 0, false, true), Action::Swallow);

        // After all modifiers released, F3 down passes normally.
        assert_eq!(matcher.on_key(VK_F1 + 2, 0, true, true), Action::Pass);
    }

    #[test]
    fn meta_modifier_or_incomplete_chord_passes() {
        let mut matcher = Matcher::default();
        assert_eq!(
            matcher.on_key(VK_F1, MODIFIER_CTRL | MODIFIER_ALT, true, true),
            Action::Pass
        );
        assert_eq!(
            matcher.on_key(VK_F1, CTRL_ALT_SHIFT | MODIFIER_META, true, true),
            Action::Pass
        );

        assert_eq!(matcher.on_key(VK_LWIN, 0, true, true), Action::Pass);
        assert_eq!(matcher.on_key(VK_F2, CTRL_ALT_SHIFT, true, true), Action::Pass);
        assert_eq!(matcher.on_key(VK_LWIN, 0, false, true), Action::Pass);
        assert_eq!(
            matcher.on_key(VK_F2, CTRL_ALT_SHIFT, true, true),
            Action::Switch(2)
        );
    }

    #[test]
    fn disallowed_session_passes_new_presses_but_finishes_swallowed_release() {
        let mut matcher = Matcher::default();
        assert_eq!(matcher.on_key(VK_F1, CTRL_ALT_SHIFT, true, false), Action::Pass);
        assert_eq!(matcher.on_key(VK_F1, CTRL_ALT_SHIFT, false, false), Action::Pass);

        assert_eq!(
            matcher.on_key(VK_F1, CTRL_ALT_SHIFT, true, true),
            Action::Switch(1)
        );
        assert_eq!(
            matcher.on_key(VK_F1, 0, false, false),
            Action::Swallow
        );
    }

    #[test]
    fn non_fkey_and_reset_behavior() {
        let mut matcher = Matcher::default();
        assert_eq!(matcher.on_key(VK_SHIFT, 0, true, true), Action::Pass);
        assert_eq!(matcher.on_key(VK_CONTROL, 0, true, true), Action::Pass);
        assert_eq!(matcher.on_key(VK_MENU, 0, true, true), Action::Pass);
        assert_eq!(matcher.on_key(0x41, CTRL_ALT_SHIFT, true, true), Action::Pass);
        assert_eq!(matcher.on_key(VK_F12 + 1, CTRL_ALT_SHIFT, true, true), Action::Pass);

        assert_eq!(matcher.on_key(VK_F4, 0, true, true), Action::Switch(4));
        matcher.reset();
        // Both tracked modifiers and swallowed F4 are cleared.
        assert_eq!(matcher.on_key(VK_F4, 0, false, true), Action::Pass);
        assert_eq!(matcher.on_key(VK_F4, 0, true, true), Action::Pass);
    }
}
