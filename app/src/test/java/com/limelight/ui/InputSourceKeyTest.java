package com.limelight.ui;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNull;

import android.app.Application;
import android.view.InputDevice;
import android.view.KeyCharacterMap;
import android.view.KeyEvent;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;

/** The hints on the real TV went away under a plain {@code adb shell input keyevent}: its keys have no source. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class InputSourceKeyTest {
    private static KeyEvent key(int source) {
        return new KeyEvent(0, 0, KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_DPAD_RIGHT, 0, 0, KeyCharacterMap.VIRTUAL_KEYBOARD, 0, 0, source);
    }

    @Test
    public void aKeyWithNoSourceSaysNothingAboutTheHand() {
        assertNull(InputSource.of(key(InputDevice.SOURCE_UNKNOWN)));
    }

    @Test
    public void aRemotesAndAPadsKeysNameThem() {
        assertEquals(InputSource.REMOTE, InputSource.of(key(InputDevice.SOURCE_DPAD)));
        assertEquals(InputSource.PAD, InputSource.of(key(InputDevice.SOURCE_GAMEPAD | InputDevice.SOURCE_KEYBOARD)));
        assertEquals(InputSource.PAD, InputSource.of(key(InputDevice.SOURCE_JOYSTICK)));
    }
}
