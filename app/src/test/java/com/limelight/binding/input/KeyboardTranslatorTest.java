package com.limelight.binding.input;

import android.view.InputDevice;
import android.view.KeyEvent;
import com.limelight.nvstream.input.KeyboardPacket;
import org.junit.Test;

import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;

import static org.junit.Assert.*;

public class KeyboardTranslatorTest {
    @Test
    public void numpadEqualsUsesTheEqualsVirtualKey() {
        assertEquals((short) 0x80BB, KeyboardTranslator.translateKeyCode(KeyEvent.KEYCODE_NUMPAD_EQUALS));
    }

    @Test
    public void numpadAndPrintScreenHaveWindowsVirtualKeys() {
        assertEquals((short) 0x800D, KeyboardTranslator.translateKeyCode(KeyEvent.KEYCODE_NUMPAD_ENTER));
        assertEquals((short) 0x806C, KeyboardTranslator.translateKeyCode(KeyEvent.KEYCODE_NUMPAD_COMMA));
        assertEquals((short) 0x802C, KeyboardTranslator.translateKeyCode(KeyEvent.KEYCODE_SYSRQ));
        assertEquals(KeyboardTranslator.translateKeyCode(KeyEvent.KEYCODE_SCREENSHOT),
                KeyboardTranslator.translateKeyCode(KeyEvent.KEYCODE_SYSRQ));
        assertEquals(0, KeyboardTranslator.translateKeyCode(KeyEvent.KEYCODE_UNKNOWN));
    }

    @Test
    public void leftAndRightModifiersRemainDistinctIncludingAltGr() {
        assertEquals((short) 0x80A4, KeyboardTranslator.translateKeyCode(KeyEvent.KEYCODE_ALT_LEFT));
        assertEquals((short) 0x80A5, KeyboardTranslator.translateKeyCode(KeyEvent.KEYCODE_ALT_RIGHT));
        assertEquals((short) 0x80A2, KeyboardTranslator.translateKeyCode(KeyEvent.KEYCODE_CTRL_LEFT));
        assertEquals((short) 0x80A3, KeyboardTranslator.translateKeyCode(KeyEvent.KEYCODE_CTRL_RIGHT));
    }

    @Test
    public void unicodeKeepsSupplementaryCharactersAndRejectsDeadKeys() {
        assertEquals("é", KeyboardTranslator.textForCodePoint(0xe9));
        assertEquals("\uD83D\uDE00", KeyboardTranslator.textForCodePoint(0x1f600));
        for (int invalid : new int[] {0, -1, 0x80000065, 0xd800, 0xdfff, 0x110000}) {
            assertNull(KeyboardTranslator.textForCodePoint(invalid));
        }
    }

    @Test
    public void committedAndMultipleTextKeepsUnicodeIntact() {
        List<String> text = new ArrayList<>();
        assertTrue(KeyboardTranslator.sendTextInput("hello", text::add,
                (key, direction) -> fail("Printable text became a raw key")));
        assertTrue(KeyboardTranslator.sendTextInput(new StringBuilder("é\uD83D\uDE00中文"), text::add,
                (key, direction) -> fail("Unicode text became a raw key")));
        assertEquals(Arrays.asList("hello", "é\uD83D\uDE00中文"), text);
    }

    @Test
    public void backspaceAndEnterSendBalancedKeysInTextOrder() {
        List<Object> events = new ArrayList<>();
        assertTrue(KeyboardTranslator.sendTextInput("hello\b\nworld", events::add,
                (key, direction) -> {
                    events.add(key);
                    events.add(direction);
                }));
        assertEquals(Arrays.asList("hello", (short) 0x8008, KeyboardPacket.KEY_DOWN,
                (short) 0x8008, KeyboardPacket.KEY_UP, (short) 0x800D, KeyboardPacket.KEY_DOWN,
                (short) 0x800D, KeyboardPacket.KEY_UP, "world"), events);
        assertEquals((short) 0x8008, KeyboardTranslator.translateKeyCode(KeyEvent.KEYCODE_DEL));
        assertEquals((short) 0x800D, KeyboardTranslator.translateKeyCode(KeyEvent.KEYCODE_ENTER));
    }

    @Test
    public void carriageReturnLineFeedSendsOneEnterPerLineBreak() {
        List<Byte> directions = new ArrayList<>();
        assertTrue(KeyboardTranslator.sendTextInput("\r\n\r\n", text -> fail("Sent control text"),
                (key, direction) -> {
                    assertEquals((short) 0x800D, key.shortValue());
                    directions.add(direction);
                }));
        assertEquals(Arrays.asList(KeyboardPacket.KEY_DOWN, KeyboardPacket.KEY_UP,
                KeyboardPacket.KEY_DOWN, KeyboardPacket.KEY_UP), directions);
    }

    @Test
    public void missingOrEmptyTextSendsNothing() {
        assertFalse(KeyboardTranslator.sendTextInput(null, text -> fail("Sent null text"),
                (key, direction) -> fail("Sent a key for null text")));
        assertTrue(KeyboardTranslator.sendTextInput("", text -> fail("Sent empty text"),
                (key, direction) -> fail("Sent a key for empty text")));
    }

    @Test
    public void keyboardBackBecomesTheWindowsKeyOnlyWhenEnabled() {
        assertEquals(KeyEvent.KEYCODE_META_LEFT, KeyboardTranslator.remapPhysicalKey(KeyEvent.KEYCODE_BACK, true, true, false));
        assertEquals(KeyEvent.KEYCODE_BACK, KeyboardTranslator.remapPhysicalKey(KeyEvent.KEYCODE_BACK, true, false, false));
        // Back that is not from a keyboard keeps opening the stream menu.
        assertEquals(KeyEvent.KEYCODE_BACK, KeyboardTranslator.remapPhysicalKey(KeyEvent.KEYCODE_BACK, false, true, true));
        assertEquals((short) 0x805B, KeyboardTranslator.translateKeyCode(
                KeyboardTranslator.remapPhysicalKey(KeyEvent.KEYCODE_BACK, true, true, false)));
    }

    @Test
    public void rightAltBecomesCommandAndLeftAltStaysAlt() {
        assertEquals(KeyEvent.KEYCODE_META_LEFT, KeyboardTranslator.remapPhysicalKey(KeyEvent.KEYCODE_ALT_RIGHT, false, false, true));
        assertEquals(KeyEvent.KEYCODE_ALT_RIGHT, KeyboardTranslator.remapPhysicalKey(KeyEvent.KEYCODE_ALT_RIGHT, false, false, false));
        assertEquals(KeyEvent.KEYCODE_ALT_LEFT, KeyboardTranslator.remapPhysicalKey(KeyEvent.KEYCODE_ALT_LEFT, false, true, true));
        assertEquals(KeyEvent.KEYCODE_C, KeyboardTranslator.remapPhysicalKey(KeyEvent.KEYCODE_C, true, true, true));
    }

    @Test
    public void onlyARealAlphabeticKeyboardCountsAsKeyboardBack() {
        int keyboard = InputDevice.SOURCE_KEYBOARD;
        assertTrue(KeyboardTranslator.isKeyboardBack(KeyEvent.KEYCODE_BACK, false, false, true, false, keyboard));
        // Navigation bar or gesture and fallback keys, virtual devices, remotes and controllers.
        assertFalse(KeyboardTranslator.isKeyboardBack(KeyEvent.KEYCODE_BACK, true, false, true, false, keyboard));
        assertFalse(KeyboardTranslator.isKeyboardBack(KeyEvent.KEYCODE_BACK, false, true, true, false, keyboard));
        assertFalse(KeyboardTranslator.isKeyboardBack(KeyEvent.KEYCODE_BACK, false, false, false, false, keyboard));
        assertFalse(KeyboardTranslator.isKeyboardBack(KeyEvent.KEYCODE_BACK, false, false, true, true, keyboard));
        // The right-click Back some devices synthesize comes from the mouse.
        assertFalse(KeyboardTranslator.isKeyboardBack(KeyEvent.KEYCODE_BACK, false, false, true, false, InputDevice.SOURCE_MOUSE));
        assertFalse(KeyboardTranslator.isKeyboardBack(KeyEvent.KEYCODE_ESCAPE, false, false, true, false, keyboard));
    }

    @Test
    public void rightAltAsCommandDropsAltOnlyForRightAlt() {
        int right = KeyEvent.META_ALT_ON | KeyEvent.META_ALT_RIGHT_ON;
        int left = KeyEvent.META_ALT_ON | KeyEvent.META_ALT_LEFT_ON;
        assertTrue(KeyboardTranslator.altHeld(right, false));
        assertFalse(KeyboardTranslator.altHeld(right, true));
        assertTrue(KeyboardTranslator.altHeld(left, true));
        assertTrue(KeyboardTranslator.altHeld(left | right, true));
        // Some on-screen keyboards report Alt without a side.
        assertTrue(KeyboardTranslator.altHeld(KeyEvent.META_ALT_ON, true));
        assertFalse(KeyboardTranslator.altHeld(KeyEvent.META_SHIFT_ON, false));
    }
}
