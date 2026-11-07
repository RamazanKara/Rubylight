package com.limelight.pin;

import static org.junit.Assert.assertEquals;

import android.view.KeyEvent;

import org.junit.Test;

public class PinKeysTest {
    private static PinKeys.Action act(int code) {
        return PinKeys.of(code, false);
    }

    @Test
    public void theDpadAndSelectAreFocusesBusinessSoTheyMoveOverAndPressTheKeys() {
        for (int code : new int[] {KeyEvent.KEYCODE_DPAD_UP, KeyEvent.KEYCODE_DPAD_DOWN, KeyEvent.KEYCODE_DPAD_LEFT,
                KeyEvent.KEYCODE_DPAD_RIGHT, KeyEvent.KEYCODE_DPAD_CENTER, KeyEvent.KEYCODE_BUTTON_A}) {
            assertEquals(PinKeys.Kind.NONE, act(code).kind);
        }
    }

    @Test
    public void enterIsSubmitOnAKeyboardAndSelectOnARemote() {
        assertEquals(PinKeys.Kind.NONE, PinKeys.of(KeyEvent.KEYCODE_ENTER, false).kind);
        assertEquals(PinKeys.Kind.SUBMIT, PinKeys.of(KeyEvent.KEYCODE_ENTER, true).kind);
        assertEquals(PinKeys.Kind.SUBMIT, act(KeyEvent.KEYCODE_NUMPAD_ENTER).kind);
    }

    @Test
    public void numberKeysTypeTheirDigit() {
        for (int digit = 0; digit <= 9; digit++) {
            assertEquals(new PinKeys.Action(PinKeys.Kind.DIGIT, digit), act(KeyEvent.KEYCODE_0 + digit));
            assertEquals(new PinKeys.Action(PinKeys.Kind.DIGIT, digit), act(KeyEvent.KEYCODE_NUMPAD_0 + digit));
        }
    }

    @Test
    public void deleteAndLeave() {
        assertEquals(PinKeys.Kind.DELETE, act(KeyEvent.KEYCODE_DEL).kind);
        assertEquals(PinKeys.Kind.DELETE, act(KeyEvent.KEYCODE_BUTTON_X).kind);
        assertEquals(PinKeys.Kind.BACK, act(KeyEvent.KEYCODE_BUTTON_B).kind);
        assertEquals(PinKeys.Kind.BACK, act(KeyEvent.KEYCODE_ESCAPE).kind);
    }

    @Test
    public void otherKeysAreLeftAlone() {
        assertEquals(PinKeys.Kind.NONE, act(KeyEvent.KEYCODE_BACK).kind);
        assertEquals(PinKeys.Kind.NONE, act(KeyEvent.KEYCODE_A).kind);
        assertEquals(PinKeys.Kind.NONE, act(KeyEvent.KEYCODE_VOLUME_UP).kind);
    }
}
