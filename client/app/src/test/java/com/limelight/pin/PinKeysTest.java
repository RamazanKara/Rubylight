package com.limelight.pin;

import static org.junit.Assert.assertEquals;

import android.view.KeyEvent;

import org.junit.Test;

public class PinKeysTest {
    private static PinKeys.Action act(int code) {
        return PinKeys.of(code, false);
    }

    @Test
    public void upAndDownPickTheRow() {
        assertEquals(PinKeys.Kind.ROW_UP, act(KeyEvent.KEYCODE_DPAD_UP).kind);
        assertEquals(PinKeys.Kind.ROW_DOWN, act(KeyEvent.KEYCODE_DPAD_DOWN).kind);
    }

    @Test
    public void leftSelectAndRightTypeTheFirstSecondAndThirdOfTheRow() {
        assertEquals(new PinKeys.Action(PinKeys.Kind.CELL, 0), act(KeyEvent.KEYCODE_DPAD_LEFT));
        assertEquals(new PinKeys.Action(PinKeys.Kind.CELL, 1), act(KeyEvent.KEYCODE_DPAD_CENTER));
        assertEquals(new PinKeys.Action(PinKeys.Kind.CELL, 1), act(KeyEvent.KEYCODE_BUTTON_A));
        assertEquals(new PinKeys.Action(PinKeys.Kind.CELL, 2), act(KeyEvent.KEYCODE_DPAD_RIGHT));
    }

    @Test
    public void enterIsSelectOnARemoteAndDoneOnAKeyboard() {
        assertEquals(new PinKeys.Action(PinKeys.Kind.CELL, 1), PinKeys.of(KeyEvent.KEYCODE_ENTER, false));
        assertEquals(PinKeys.Kind.SUBMIT, PinKeys.of(KeyEvent.KEYCODE_ENTER, true).kind);
        assertEquals(PinKeys.Kind.SUBMIT, act(KeyEvent.KEYCODE_NUMPAD_ENTER).kind);
    }

    @Test
    public void numberKeysTypeDigitsFromTheRowAndTheNumpad() {
        for (int digit = 0; digit <= 9; digit++) {
            assertEquals(new PinKeys.Action(PinKeys.Kind.DIGIT, digit), act(KeyEvent.KEYCODE_0 + digit));
            assertEquals(new PinKeys.Action(PinKeys.Kind.DIGIT, digit), act(KeyEvent.KEYCODE_NUMPAD_0 + digit));
        }
    }

    @Test
    public void deleteAndBackKeys() {
        assertEquals(PinKeys.Kind.DELETE, act(KeyEvent.KEYCODE_DEL).kind);
        assertEquals(PinKeys.Kind.DELETE, act(KeyEvent.KEYCODE_BUTTON_X).kind);
        assertEquals(PinKeys.Kind.BACK, act(KeyEvent.KEYCODE_BUTTON_B).kind);
        assertEquals(PinKeys.Kind.BACK, act(KeyEvent.KEYCODE_ESCAPE).kind);
    }

    @Test
    public void theSystemBackKeyAndOtherKeysAreLeftAlone() {
        assertEquals(PinKeys.Kind.NONE, act(KeyEvent.KEYCODE_BACK).kind);
        assertEquals(PinKeys.Kind.NONE, act(KeyEvent.KEYCODE_A).kind);
        assertEquals(PinKeys.Kind.NONE, act(KeyEvent.KEYCODE_VOLUME_UP).kind);
    }

    @Test
    public void rowsWrapAtTheEnds() {
        assertEquals(3, PinKeys.moveRow(0, false));
        assertEquals(0, PinKeys.moveRow(3, true));
        assertEquals(2, PinKeys.moveRow(1, true));
        assertEquals(0, PinKeys.moveRow(1, false));
    }

    @Test
    public void everyKeyOfThePadIsReachableFromTheRowScheme() {
        int[] expected = {1, 2, 3, 4, 5, 6, 7, 8, 9, PinKeys.DELETE, 0, PinKeys.CONFIRM};
        for (int row = 0; row < 4; row++) {
            for (int column = 0; column < 3; column++) {
                assertEquals(expected[row * 3 + column], PinKeys.cell(row, column));
            }
        }
    }
}
