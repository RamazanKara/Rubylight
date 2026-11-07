package com.limelight.demo;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import android.content.Context;
import android.content.res.Configuration;

import org.robolectric.RuntimeEnvironment;

import com.limelight.R;
import com.limelight.ui.AppTheme;

import org.junit.After;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;

/** "--es look day" must reach the day palette on a device whose system is dark, or day cannot be looked at there. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 34, qualifiers = "night")
public class DemoLookTest {
    @After public void reset() {
        DemoContext.forceNight = Configuration.UI_MODE_NIGHT_UNDEFINED;
    }

    private static Context context() {
        return RuntimeEnvironment.getApplication();
    }

    /** The showcase is shot from the demo: the failed start speaks the device's language and no real device is named. */
    @Test @Config(sdk = 34, qualifiers = "de-night")
    public void theDemoFailureSpeaksGermanAndNamesNoRealDevice() {
        android.content.Intent intent = new android.content.Intent(context(), DemoConnecting.class).putExtra("state", "conn-error");
        try (var screen = org.robolectric.Robolectric.buildActivity(DemoConnecting.class, intent).setup()) {
            android.app.Dialog dialog = org.robolectric.shadows.ShadowDialog.getLatestDialog();
            CharSequence message = ((android.widget.TextView) dialog.findViewById(R.id.conn_error_message)).getText();
            assertTrue(message.toString(), message.toString().startsWith("Stream konnte nicht starten"));
            assertTrue(message.toString(), message.toString().contains("Fehlercode: 5031"));
        }
        assertFalse(DemoConsole.PAIRING_DEVICE, DemoConsole.PAIRING_DEVICE.contains("Fold"));
    }

    @Test public void aDarkSystemKeepsNightWithoutTheForce() {
        assertEquals(AppTheme.Look.NIGHT, AppTheme.look(new DemoContext(context())));
    }

    @Test public void lookDayGivesTheDayPaletteOnADarkSystem() {
        DemoContext.forceNight = DemoContext.nightOf("day");
        Context demo = new DemoContext(context());
        assertEquals(Configuration.UI_MODE_NIGHT_NO, demo.getResources().getConfiguration().uiMode & Configuration.UI_MODE_NIGHT_MASK);
        assertTrue(demo.getResources().getBoolean(R.bool.light_system_bars));
        assertEquals(AppTheme.Look.DAY, AppTheme.look(demo));
    }

    @Test public void lookDarkAsksForNightAndOtherLooksLeaveTheSystemAlone() {
        assertEquals(Configuration.UI_MODE_NIGHT_YES, DemoContext.nightOf("dark"));
        assertEquals(Configuration.UI_MODE_NIGHT_UNDEFINED, DemoContext.nightOf("oled"));
        assertEquals(Configuration.UI_MODE_NIGHT_UNDEFINED, DemoContext.nightOf(null));
        android.content.SharedPreferences prefs = android.preference.PreferenceManager.getDefaultSharedPreferences(context());
        DemoContext.putLook(prefs.edit(), "oled").commit();
        assertTrue(com.limelight.ui.AppTheme.isOledBlack(context()));
        DemoContext.putLook(prefs.edit(), "day").commit();
        assertFalse(com.limelight.ui.AppTheme.isOledBlack(context()));
        // A real screen the demo reaches, as the PIN pad, has no DemoContext and takes the look from AppCompat.
        assertEquals(androidx.appcompat.app.AppCompatDelegate.MODE_NIGHT_NO, DemoContext.appCompatNightOf("day"));
        assertEquals(androidx.appcompat.app.AppCompatDelegate.MODE_NIGHT_YES, DemoContext.appCompatNightOf("dark"));
        assertEquals(androidx.appcompat.app.AppCompatDelegate.MODE_NIGHT_FOLLOW_SYSTEM, DemoContext.appCompatNightOf("oled"));
        assertFalse(context().getResources().getBoolean(R.bool.light_system_bars));
    }
}
