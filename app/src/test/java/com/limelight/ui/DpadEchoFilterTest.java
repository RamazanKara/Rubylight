package com.limelight.ui;

import android.app.Application;
import android.os.Bundle;
import android.view.InputDevice;
import android.view.KeyEvent;
import android.view.View;
import android.widget.FrameLayout;

import com.limelight.PcViewLifecycleTest;
import com.limelight.R;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;

import java.util.ArrayList;
import java.util.List;

import static org.junit.Assert.*;

/**
 * Android turns an unhandled joystick hat into FLAG_FALLBACK DPAD keys. A pad that also sends real DPAD
 * keys (the AOSP layout for the Switch Pro Controller declares both) would move focus twice.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class,
        shadows = PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class DpadEchoFilterTest {
    private static final int PAD = 7;

    private static KeyEvent key(int action, int keyCode, int repeat, boolean fallback, int device) {
        return new KeyEvent(0, 0, action, keyCode, repeat, 0, device, 0,
                fallback ? KeyEvent.FLAG_FALLBACK : 0, InputDevice.SOURCE_JOYSTICK | InputDevice.SOURCE_GAMEPAD);
    }

    private static boolean drop(DpadEchoFilter filter, int action, int keyCode, int repeat, boolean fallback) {
        return filter.drop(key(action, keyCode, repeat, fallback, PAD));
    }

    @Test
    public void aSingleKindPassesUntouchedIncludingItsRepeats() {
        DpadEchoFilter filter = new DpadEchoFilter();
        for (boolean fallback : new boolean[] {false, true}) {
            assertFalse(drop(filter, KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_DPAD_DOWN, 0, fallback));
            assertFalse(drop(filter, KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_DPAD_DOWN, 1, fallback));
            assertFalse(drop(filter, KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_DPAD_DOWN, 2, fallback));
            assertFalse(drop(filter, KeyEvent.ACTION_UP, KeyEvent.KEYCODE_DPAD_DOWN, 0, fallback));
        }
    }

    @Test
    public void theFallbackCopyOfARealPressIsDroppedWithItsRepeatsAndRelease() {
        DpadEchoFilter filter = new DpadEchoFilter();
        assertFalse(drop(filter, KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_DPAD_RIGHT, 0, false));
        assertTrue(drop(filter, KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_DPAD_RIGHT, 0, true));
        // The real key repeats; the hat's repeats are the echo's and go too.
        assertFalse(drop(filter, KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_DPAD_RIGHT, 1, false));
        assertTrue(drop(filter, KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_DPAD_RIGHT, 1, true));
        // The real key comes up first; the hat's repeat that arrives after is still the echo.
        assertFalse(drop(filter, KeyEvent.ACTION_UP, KeyEvent.KEYCODE_DPAD_RIGHT, 0, false));
        assertTrue(drop(filter, KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_DPAD_RIGHT, 2, true));
        assertTrue(drop(filter, KeyEvent.ACTION_UP, KeyEvent.KEYCODE_DPAD_RIGHT, 0, true));
        // A fresh press after both are up is a new press, of either kind.
        assertFalse(drop(filter, KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_DPAD_RIGHT, 0, true));
        assertFalse(drop(filter, KeyEvent.ACTION_UP, KeyEvent.KEYCODE_DPAD_RIGHT, 0, true));
    }

    @Test
    public void theRealCopyOfAHatPressIsDroppedWhenTheHatComesFirst() {
        DpadEchoFilter filter = new DpadEchoFilter();
        assertFalse(drop(filter, KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_DPAD_UP, 0, true));
        assertTrue(drop(filter, KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_DPAD_UP, 0, false));
        assertTrue(drop(filter, KeyEvent.ACTION_UP, KeyEvent.KEYCODE_DPAD_UP, 0, false));
        // The hat's own release still gets through, or focus would believe the key stuck down.
        assertFalse(drop(filter, KeyEvent.ACTION_UP, KeyEvent.KEYCODE_DPAD_UP, 0, true));
    }

    @Test
    public void directionsAndDevicesAreIndependent() {
        DpadEchoFilter filter = new DpadEchoFilter();
        assertFalse(filter.drop(key(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_DPAD_LEFT, 0, false, PAD)));
        // Another direction, and the same direction from another pad, are not echoes.
        assertFalse(filter.drop(key(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_DPAD_UP, 0, true, PAD)));
        assertFalse(filter.drop(key(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_DPAD_LEFT, 0, true, PAD + 1)));
        assertTrue(filter.drop(key(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_DPAD_LEFT, 0, true, PAD)));
    }

    @Test
    public void otherKeysAndCancelledReleasesAreNeverFiltered() {
        DpadEchoFilter filter = new DpadEchoFilter();
        assertFalse(drop(filter, KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_BUTTON_A, 0, false));
        assertFalse(drop(filter, KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_BUTTON_A, 0, true));
        assertFalse(drop(filter, KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_DPAD_CENTER, 0, false));
        assertFalse(drop(filter, KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_DPAD_CENTER, 0, true));
        // The framework's cancelling release for a hat key, on focus loss, must always arrive.
        assertFalse(drop(filter, KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_DPAD_DOWN, 0, true));
        KeyEvent cancel = new KeyEvent(0, 0, KeyEvent.ACTION_UP, KeyEvent.KEYCODE_DPAD_DOWN, 0, 0, PAD, 0,
                KeyEvent.FLAG_FALLBACK | KeyEvent.FLAG_CANCELED, InputDevice.SOURCE_JOYSTICK);
        assertFalse(filter.drop(cancel));
    }

    @Test
    public void resetForgetsWhatWasHeld() {
        DpadEchoFilter filter = new DpadEchoFilter();
        assertFalse(drop(filter, KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_DPAD_DOWN, 0, false));
        assertTrue(drop(filter, KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_DPAD_DOWN, 0, true));
        filter.reset();
        assertFalse(drop(filter, KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_DPAD_DOWN, 0, true));
    }

    /** A NightjarActivity and a sheet both put the filter in front of their views. */
    public static class Probe extends NightjarActivity {
        final List<Integer> downs = new ArrayList<>();
        @Override protected void onCreate(Bundle state) {
            setTheme(R.style.AppTheme);
            super.onCreate(state);
            View view = new View(this);
            view.setFocusable(true);
            view.setOnKeyListener((v, code, event) -> {
                if (event.getAction() == KeyEvent.ACTION_DOWN) downs.add(code);
                return true;
            });
            setContentView(new FrameLayout(this) {{ addView(view); }});
            view.requestFocus();
        }
    }

    @Test
    public void anActivityMovesFocusOnceForAPadThatSendsKeysAndAHat() {
        try (var controller = Robolectric.buildActivity(Probe.class).setup().visible()) {
            Probe probe = controller.get();
            assertTrue(probe.dispatchKeyEvent(key(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_DPAD_DOWN, 0, false, PAD)));
            assertTrue(probe.dispatchKeyEvent(key(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_DPAD_DOWN, 0, true, PAD)));
            assertEquals(1, probe.downs.size());
            probe.dispatchKeyEvent(key(KeyEvent.ACTION_UP, KeyEvent.KEYCODE_DPAD_DOWN, 0, false, PAD));
            probe.dispatchKeyEvent(key(KeyEvent.ACTION_UP, KeyEvent.KEYCODE_DPAD_DOWN, 0, true, PAD));
            // A keyboard's arrow key (no fallback flag) and a remote are untouched.
            probe.dispatchKeyEvent(key(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_DPAD_DOWN, 0, false, PAD));
            assertEquals(2, probe.downs.size());
        }
    }

    @Test
    public void pausingTheActivityForgetsHeldKeys() {
        try (var controller = Robolectric.buildActivity(Probe.class).setup().visible()) {
            Probe probe = controller.get();
            probe.dispatchKeyEvent(key(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_DPAD_DOWN, 0, false, PAD));
            controller.pause().resume();
            probe.dispatchKeyEvent(key(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_DPAD_DOWN, 0, true, PAD));
            assertEquals("The stale real key must not eat the hat afterwards", 2, probe.downs.size());
        }
    }
}
