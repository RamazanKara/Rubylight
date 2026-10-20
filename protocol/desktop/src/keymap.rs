//! SDL scancodes to the Windows virtual-key codes hosts take. Positional, as a US layout:
//! the host's own layout turns them into characters.
//!
//! The table is moonlight-qt's `SdlInputHandler::handleKeyEvent` (app/streaming/input/
//! keyboard.cpp, GPL-3.0, Cameron Gutman and contributors).

use sdl3_sys::everything::*;

/// Sunshine's `SS_KBE_FLAG_NON_NORMALIZED`: the code is not a US-layout position.
pub const FLAG_NON_NORMALIZED: u8 = 0x01;

/// The virtual-key code and the Sunshine flags for a scancode; None for keys hosts do not
/// take. `system_keys` allows the Windows keys through.
pub fn virtual_key(scancode: SDL_Scancode, system_keys: bool) -> Option<(u16, u8)> {
    let s = scancode.0;
    let range = |a: SDL_Scancode, b: SDL_Scancode| (a.0..=b.0).contains(&s);
    if range(SDL_SCANCODE_1, SDL_SCANCODE_9) {
        return Some(((s - SDL_SCANCODE_1.0) as u16 + 0x31, 0));
    }
    if range(SDL_SCANCODE_A, SDL_SCANCODE_Z) {
        return Some(((s - SDL_SCANCODE_A.0) as u16 + 0x41, 0));
    }
    if range(SDL_SCANCODE_F1, SDL_SCANCODE_F12) {
        return Some(((s - SDL_SCANCODE_F1.0) as u16 + 0x70, 0));
    }
    if range(SDL_SCANCODE_F13, SDL_SCANCODE_F24) {
        return Some(((s - SDL_SCANCODE_F13.0) as u16 + 0x7C, 0));
    }
    if range(SDL_SCANCODE_KP_1, SDL_SCANCODE_KP_9) {
        return Some(((s - SDL_SCANCODE_KP_1.0) as u16 + 0x61, 0));
    }
    let code: u16 = match scancode {
        SDL_SCANCODE_BACKSPACE => 0x08,
        SDL_SCANCODE_TAB => 0x09,
        SDL_SCANCODE_CLEAR => 0x0C,
        SDL_SCANCODE_KP_ENTER | SDL_SCANCODE_RETURN => 0x0D,
        SDL_SCANCODE_PAUSE => 0x13,
        SDL_SCANCODE_CAPSLOCK => 0x14,
        SDL_SCANCODE_ESCAPE => 0x1B,
        SDL_SCANCODE_SPACE => 0x20,
        SDL_SCANCODE_PAGEUP => 0x21,
        SDL_SCANCODE_PAGEDOWN => 0x22,
        SDL_SCANCODE_END => 0x23,
        SDL_SCANCODE_HOME => 0x24,
        SDL_SCANCODE_LEFT => 0x25,
        SDL_SCANCODE_UP => 0x26,
        SDL_SCANCODE_RIGHT => 0x27,
        SDL_SCANCODE_DOWN => 0x28,
        SDL_SCANCODE_SELECT => 0x29,
        SDL_SCANCODE_EXECUTE => 0x2B,
        SDL_SCANCODE_PRINTSCREEN => 0x2C,
        SDL_SCANCODE_INSERT => 0x2D,
        SDL_SCANCODE_DELETE => 0x2E,
        SDL_SCANCODE_HELP => 0x2F,
        SDL_SCANCODE_KP_0 => 0x60,
        SDL_SCANCODE_0 => 0x30,
        SDL_SCANCODE_KP_MULTIPLY => 0x6A,
        SDL_SCANCODE_KP_PLUS => 0x6B,
        SDL_SCANCODE_KP_COMMA => 0x6C,
        SDL_SCANCODE_KP_MINUS => 0x6D,
        SDL_SCANCODE_KP_PERIOD => 0x6E,
        SDL_SCANCODE_KP_DIVIDE => 0x6F,
        SDL_SCANCODE_NUMLOCKCLEAR => 0x90,
        SDL_SCANCODE_SCROLLLOCK => 0x91,
        SDL_SCANCODE_LSHIFT => 0xA0,
        SDL_SCANCODE_RSHIFT => 0xA1,
        SDL_SCANCODE_LCTRL => 0xA2,
        SDL_SCANCODE_RCTRL => 0xA3,
        SDL_SCANCODE_LALT => 0xA4,
        SDL_SCANCODE_RALT => 0xA5,
        SDL_SCANCODE_LGUI if system_keys => 0x5B,
        SDL_SCANCODE_RGUI if system_keys => 0x5C,
        SDL_SCANCODE_APPLICATION => 0x5D,
        SDL_SCANCODE_AC_BACK => 0xA6,
        SDL_SCANCODE_AC_FORWARD => 0xA7,
        SDL_SCANCODE_AC_REFRESH => 0xA8,
        SDL_SCANCODE_AC_STOP => 0xA9,
        SDL_SCANCODE_AC_SEARCH => 0xAA,
        SDL_SCANCODE_AC_BOOKMARKS => 0xAB,
        SDL_SCANCODE_AC_HOME => 0xAC,
        SDL_SCANCODE_SEMICOLON => 0xBA,
        SDL_SCANCODE_EQUALS => 0xBB,
        SDL_SCANCODE_COMMA => 0xBC,
        SDL_SCANCODE_MINUS => 0xBD,
        SDL_SCANCODE_PERIOD => 0xBE,
        SDL_SCANCODE_SLASH => 0xBF,
        SDL_SCANCODE_GRAVE => 0xC0,
        SDL_SCANCODE_LEFTBRACKET => 0xDB,
        SDL_SCANCODE_INTERNATIONAL3 => return Some((0xDC, FLAG_NON_NORMALIZED)),
        SDL_SCANCODE_BACKSLASH => 0xDC,
        SDL_SCANCODE_RIGHTBRACKET => 0xDD,
        SDL_SCANCODE_APOSTROPHE => 0xDE,
        SDL_SCANCODE_INTERNATIONAL1 => return Some((0xE2, FLAG_NON_NORMALIZED)),
        SDL_SCANCODE_NONUSBACKSLASH => 0xE2,
        SDL_SCANCODE_LANG1 => 0x1C,
        SDL_SCANCODE_LANG2 => 0x1D,
        _ => return None,
    };
    Some((code, 0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_digits_and_modifiers() {
        assert_eq!(virtual_key(SDL_SCANCODE_A, false), Some((0x41, 0)));
        assert_eq!(virtual_key(SDL_SCANCODE_Z, false), Some((0x5A, 0)));
        assert_eq!(virtual_key(SDL_SCANCODE_1, false), Some((0x31, 0)));
        assert_eq!(virtual_key(SDL_SCANCODE_0, false), Some((0x30, 0)));
        assert_eq!(virtual_key(SDL_SCANCODE_KP_0, false), Some((0x60, 0)));
        assert_eq!(virtual_key(SDL_SCANCODE_KP_9, false), Some((0x69, 0)));
        assert_eq!(virtual_key(SDL_SCANCODE_F12, false), Some((0x7B, 0)));
        assert_eq!(virtual_key(SDL_SCANCODE_RALT, false), Some((0xA5, 0)));
        assert_eq!(virtual_key(SDL_SCANCODE_LGUI, false), None);
        assert_eq!(virtual_key(SDL_SCANCODE_LGUI, true), Some((0x5B, 0)));
        assert_eq!(virtual_key(SDL_SCANCODE_INTERNATIONAL1, false), Some((0xE2, FLAG_NON_NORMALIZED)));
    }
}
