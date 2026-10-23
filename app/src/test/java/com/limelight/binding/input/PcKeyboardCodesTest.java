package com.limelight.binding.input;

import static org.junit.Assert.assertEquals;

import android.view.KeyEvent;

import com.limelight.binding.input.keyboard.PcKey;
import com.limelight.binding.input.keyboard.PcKeyboardLayout;

import org.junit.Test;

/** The on-screen keyboard must send what the physical-keyboard path sends for the same key. */
public class PcKeyboardCodesTest {
    private static void assertSameAsPhysical(PcKeyboardLayout layout, String id, int androidKeyCode) {
        PcKey key = layout.key(id);
        assertEquals(id + " in " + layout.id, KeyboardTranslator.translateKeyCode(androidKeyCode),
                (short) (0x8000 | key.vk));
    }

    @Test
    public void everyKeyMatchesTheTranslatorForItsAndroidKey() {
        for (String id : PcKeyboardLayout.IDS) {
            PcKeyboardLayout layout = PcKeyboardLayout.byId(id);
            for (char c = 'A'; c <= 'Z'; c++) assertSameAsPhysical(layout, String.valueOf(c), KeyEvent.KEYCODE_A + c - 'A');
            for (char c = '0'; c <= '9'; c++) assertSameAsPhysical(layout, String.valueOf(c), KeyEvent.KEYCODE_0 + c - '0');
            for (int i = 0; i < 12; i++) assertSameAsPhysical(layout, "F" + (i + 1), KeyEvent.KEYCODE_F1 + i);
            assertSameAsPhysical(layout, "Esc", KeyEvent.KEYCODE_ESCAPE);
            assertSameAsPhysical(layout, "Tab", KeyEvent.KEYCODE_TAB);
            assertSameAsPhysical(layout, "Caps", KeyEvent.KEYCODE_CAPS_LOCK);
            assertSameAsPhysical(layout, "Enter", KeyEvent.KEYCODE_ENTER);
            assertSameAsPhysical(layout, "Backspace", KeyEvent.KEYCODE_DEL);
            assertSameAsPhysical(layout, "Space", KeyEvent.KEYCODE_SPACE);
            assertSameAsPhysical(layout, "Up", KeyEvent.KEYCODE_DPAD_UP);
            assertSameAsPhysical(layout, "Down", KeyEvent.KEYCODE_DPAD_DOWN);
            assertSameAsPhysical(layout, "Left", KeyEvent.KEYCODE_DPAD_LEFT);
            assertSameAsPhysical(layout, "Right", KeyEvent.KEYCODE_DPAD_RIGHT);
            assertSameAsPhysical(layout, "Home", KeyEvent.KEYCODE_MOVE_HOME);
            assertSameAsPhysical(layout, "End", KeyEvent.KEYCODE_MOVE_END);
            assertSameAsPhysical(layout, "PgUp", KeyEvent.KEYCODE_PAGE_UP);
            assertSameAsPhysical(layout, "PgDn", KeyEvent.KEYCODE_PAGE_DOWN);
            assertSameAsPhysical(layout, "Ins", KeyEvent.KEYCODE_INSERT);
            assertSameAsPhysical(layout, "Del", KeyEvent.KEYCODE_FORWARD_DEL);
            assertSameAsPhysical(layout, "PrtSc", KeyEvent.KEYCODE_SYSRQ);
            assertSameAsPhysical(layout, "ScrLk", KeyEvent.KEYCODE_SCROLL_LOCK);
            assertSameAsPhysical(layout, "Pause", KeyEvent.KEYCODE_BREAK);
            assertSameAsPhysical(layout, "Menu", KeyEvent.KEYCODE_MENU);
            assertSameAsPhysical(layout, "Backquote", KeyEvent.KEYCODE_GRAVE);
            assertSameAsPhysical(layout, "Minus", KeyEvent.KEYCODE_MINUS);
            assertSameAsPhysical(layout, "Equal", KeyEvent.KEYCODE_EQUALS);
            assertSameAsPhysical(layout, "BracketLeft", KeyEvent.KEYCODE_LEFT_BRACKET);
            assertSameAsPhysical(layout, "BracketRight", KeyEvent.KEYCODE_RIGHT_BRACKET);
            assertSameAsPhysical(layout, "Backslash", KeyEvent.KEYCODE_BACKSLASH);
            assertSameAsPhysical(layout, "Semicolon", KeyEvent.KEYCODE_SEMICOLON);
            assertSameAsPhysical(layout, "Quote", KeyEvent.KEYCODE_APOSTROPHE);
            assertSameAsPhysical(layout, "Comma", KeyEvent.KEYCODE_COMMA);
            assertSameAsPhysical(layout, "Period", KeyEvent.KEYCODE_PERIOD);
            assertSameAsPhysical(layout, "Slash", KeyEvent.KEYCODE_SLASH);
            // Modifier keys are sticky and always go out as the left-hand code, so only that side is compared.
            assertSameAsPhysical(layout, "ShiftL", KeyEvent.KEYCODE_SHIFT_LEFT);
            assertSameAsPhysical(layout, "CtrlL", KeyEvent.KEYCODE_CTRL_LEFT);
            assertSameAsPhysical(layout, "AltL", KeyEvent.KEYCODE_ALT_LEFT);
            assertSameAsPhysical(layout, "WinL", KeyEvent.KEYCODE_META_LEFT);
        }
        assertSameAsPhysical(PcKeyboardLayout.byId(PcKeyboardLayout.DE), "AltGr", KeyEvent.KEYCODE_ALT_RIGHT);
    }
}
