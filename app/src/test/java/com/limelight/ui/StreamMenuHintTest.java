package com.limelight.ui;

import android.app.Application;
import android.content.Context;
import android.content.res.Configuration;

import com.limelight.binding.input.StreamHotkeys;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;

import java.util.Locale;

import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class StreamMenuHintTest {
    /** The real TV's virtual-search: keyboard, D-pad and the gamepad bit, no axes, and only a remote in the hand. */
    @Test
    public void aDeviceWithNoAxesIsNoPad() {
        assertFalse(StreamMenuHint.isPad(android.view.InputDevice.SOURCE_KEYBOARD | android.view.InputDevice.SOURCE_DPAD
                | android.view.InputDevice.SOURCE_GAMEPAD));
        assertFalse(StreamMenuHint.isPad(android.view.InputDevice.SOURCE_KEYBOARD | android.view.InputDevice.SOURCE_DPAD));
        assertTrue(StreamMenuHint.isPad(android.view.InputDevice.SOURCE_GAMEPAD | android.view.InputDevice.SOURCE_JOYSTICK
                | android.view.InputDevice.SOURCE_KEYBOARD | android.view.InputDevice.SOURCE_DPAD));
    }

    private static Context context(Locale locale) {
        Context app = RuntimeEnvironment.getApplication();
        Configuration config = new Configuration(app.getResources().getConfiguration());
        config.setLocale(locale);
        return app.createConfigurationContext(config);
    }

    @Test
    public void theHintNamesOnlyTheInputsThisDeviceHas() {
        Context en = context(Locale.US);
        assertEquals("To open the stream menu, swipe back from the screen edge.",
                StreamMenuHint.text(en, StreamMenuHint.Back.GESTURE, false, null));
        assertEquals("To open the stream menu, swipe up from the bottom and tap Back.",
                StreamMenuHint.text(en, StreamMenuHint.Back.BUTTONS, false, null));
        assertEquals("To open the stream menu, swipe back from the screen edge or press Start + Back + LB + RB on the controller.",
                StreamMenuHint.text(en, StreamMenuHint.Back.GESTURE, true, null));
        // A TV remote, a controller and a keyboard.
        assertEquals("To open the stream menu, press Back, press Start + Back + LB + RB on the controller " +
                        "or press Ctrl + Alt + Shift + Tab.",
                StreamMenuHint.text(en, StreamMenuHint.Back.REMOTE, true, "Ctrl + Alt + Shift + Tab"));
    }

    @Test
    public void germanReadsAsOneNaturalSentence() {
        assertEquals("Zum Öffnen des Streammenüs wische vom Bildschirmrand zurück oder drücke am Controller " +
                        "Start + Back + LB + RB.",
                StreamMenuHint.text(context(Locale.GERMANY), StreamMenuHint.Back.GESTURE, true, null));
    }

    /** The keyboard's way is the stream menu's hotkey as the user bound it, in the app's language; none while it is off. */
    @Test
    public void theKeyboardWayIsTheStreamMenusHotkeyAsBound() throws Exception {
        StreamMenuModel.use(new RustStreamMenuModel());
        try {
            android.content.SharedPreferences prefs =
                    android.preference.PreferenceManager.getDefaultSharedPreferences(RuntimeEnvironment.getApplication());
            prefs.edit().clear().commit();
            Context en = context(Locale.US);
            assertEquals("Strg + Alt + Umschalt + Tab", StreamHotkeys.streamMenuLabel(context(Locale.GERMANY), prefs));
            StreamHotkeys.record(prefs, StreamHotkeys.STREAM_MENU, new String[] {"CTRL", "F2"});
            assertEquals("To open the stream menu, press Back or press Ctrl + F2.",
                    StreamMenuHint.text(en, StreamMenuHint.Back.REMOTE, false, StreamHotkeys.streamMenuLabel(en, prefs)));
            StreamHotkeys.turnOff(prefs, StreamHotkeys.STREAM_MENU);
            assertNull(StreamHotkeys.streamMenuLabel(en, prefs));
            assertEquals("To open the stream menu, press Back.",
                    StreamMenuHint.text(en, StreamMenuHint.Back.REMOTE, false, StreamHotkeys.streamMenuLabel(en, prefs)));
        } finally {
            StreamMenuModel.use(null);
        }
    }

    @Test
    public void theSeenFlagIsStoredOnce() {
        Context app = RuntimeEnvironment.getApplication();
        assertFalse(StreamMenuHint.seen(app));
        StreamMenuHint.markSeen(app);
        assertTrue(StreamMenuHint.seen(app));
        assertTrue(app.getSharedPreferences("FirstRun", Context.MODE_PRIVATE).getBoolean("stream_menu_hint_seen", false));
    }
}
