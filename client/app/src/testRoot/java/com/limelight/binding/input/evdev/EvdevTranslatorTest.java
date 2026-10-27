package com.limelight.binding.input.evdev;

import android.view.KeyEvent;
import org.junit.Test;

import static org.junit.Assert.*;

public class EvdevTranslatorTest {
    @Test
    public void homeIsAnEditingKeyInsteadOfTheAndroidHomeButton() {
        assertEquals(KeyEvent.KEYCODE_MOVE_HOME, EvdevTranslator.translateEvdevKeyCode((short) 102));
    }

    @Test
    public void outOfRangeCodesAreIgnored() {
        assertEquals(0, EvdevTranslator.translateEvdevKeyCode((short) -1));
        assertEquals(0, EvdevTranslator.translateEvdevKeyCode(Short.MAX_VALUE));
    }
}
