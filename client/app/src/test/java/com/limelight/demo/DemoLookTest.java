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
        assertFalse(context().getResources().getBoolean(R.bool.light_system_bars));
    }
}
