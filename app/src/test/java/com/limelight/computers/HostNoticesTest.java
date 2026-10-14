package com.limelight.computers;

import android.app.Application;
import android.content.Context;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;

import static com.limelight.computers.HostNotices.Notice.*;
import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class HostNoticesTest {
    private final Context context = RuntimeEnvironment.getApplication();

    @Test
    public void eachNoticeShowsOncePerPc() {
        assertTrue(HostNotices.claim(context, "pc", VIRTUAL_DISPLAY_UNAVAILABLE));
        for (int stream = 0; stream < 3; stream++) {
            assertFalse(HostNotices.claim(context, "pc", VIRTUAL_DISPLAY_UNAVAILABLE));
        }
        assertTrue(HostNotices.claim(context, "pc", AUTOMATIC_QUALITY_UNAVAILABLE));
        assertTrue(HostNotices.claim(context, "other", VIRTUAL_DISPLAY_UNAVAILABLE));
        assertFalse(HostNotices.claim(context, "pc", AUTOMATIC_QUALITY_UNAVAILABLE));
    }

    @Test
    public void aPcThatGainsAndLosesAFeatureIsToldAgain() {
        assertTrue(HostNotices.claim(context, "pc", PYROWAVE_UNAVAILABLE));
        assertTrue(HostNotices.claim(context, "pc", VIRTUAL_DISPLAY_UNAVAILABLE));
        HostNotices.reset(context, "pc", PYROWAVE_UNAVAILABLE);
        assertTrue(HostNotices.claim(context, "pc", PYROWAVE_UNAVAILABLE));
        assertFalse(HostNotices.claim(context, "pc", VIRTUAL_DISPLAY_UNAVAILABLE));
        HostNotices.reset(context, "pc", AUTOMATIC_QUALITY_UNAVAILABLE);
        assertFalse(HostNotices.claim(context, "pc", PYROWAVE_UNAVAILABLE));
    }

    @Test
    public void forgettingAPcClearsOnlyItsNotices() {
        HostNotices.claim(context, "pc", VIRTUAL_DISPLAY_UNAVAILABLE);
        HostNotices.claim(context, "other", VIRTUAL_DISPLAY_UNAVAILABLE);
        HostNotices.forget(context, "pc");
        assertTrue(HostNotices.claim(context, "pc", VIRTUAL_DISPLAY_UNAVAILABLE));
        assertFalse(HostNotices.claim(context, "other", VIRTUAL_DISPLAY_UNAVAILABLE));
    }

    @Test
    public void streamsWithoutAKnownPcStillGetTheMessage() {
        assertTrue(HostNotices.claim(context, null, VIRTUAL_DISPLAY_UNAVAILABLE));
        assertTrue(HostNotices.claim(context, null, VIRTUAL_DISPLAY_UNAVAILABLE));
    }
}
