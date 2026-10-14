package com.limelight.ui;

import android.app.Application;
import android.content.Context;
import android.content.res.Configuration;

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
                StreamMenuHint.text(en, StreamMenuHint.Back.GESTURE, false, false));
        assertEquals("To open the stream menu, swipe up from the bottom and tap Back.",
                StreamMenuHint.text(en, StreamMenuHint.Back.BUTTONS, false, false));
        assertEquals("To open the stream menu, swipe back from the screen edge or press Start + Back + LB + RB on the controller.",
                StreamMenuHint.text(en, StreamMenuHint.Back.GESTURE, true, false));
        // A TV remote, a controller and a keyboard.
        assertEquals("To open the stream menu, press Back, press Start + Back + LB + RB on the controller " +
                        "or press Ctrl + Alt + Shift + M.",
                StreamMenuHint.text(en, StreamMenuHint.Back.REMOTE, true, true));
    }

    @Test
    public void germanReadsAsOneNaturalSentence() {
        assertEquals("Zum Öffnen des Streammenüs wische vom Bildschirmrand zurück oder drücke am Controller " +
                        "Start + Back + LB + RB.",
                StreamMenuHint.text(context(Locale.GERMANY), StreamMenuHint.Back.GESTURE, true, false));
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
