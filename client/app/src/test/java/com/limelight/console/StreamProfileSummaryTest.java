package com.limelight.console;

import static org.junit.Assert.assertEquals;

import android.app.Application;
import android.content.Context;
import android.preference.PreferenceManager;

import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class StreamProfileSummaryTest {
    private Context context;

    @Before
    public void setUp() {
        context = RuntimeEnvironment.getApplication();
        PreferenceManager.getDefaultSharedPreferences(context).edit().clear().commit();
    }

    @Test
    public void theValuesLineNamesResolutionFrameRateAndBitrate() {
        assertEquals("1920 × 1080, 60 FPS, 40 Mbps", StreamProfileSummary.values(context, 1920, 1080, 60, 40000));
        assertEquals("1280 × 720, 120 FPS, 12.5 Mbps", StreamProfileSummary.values(context, 1280, 720, 120, 12500));
    }
}
