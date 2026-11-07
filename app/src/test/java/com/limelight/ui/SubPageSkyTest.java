package com.limelight.ui;

import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import android.app.Activity;
import android.content.Intent;
import android.view.ViewGroup;

import com.limelight.pin.PinLockSettingsActivity;
import com.limelight.preferences.HostStreamSettings;
import com.limelight.preferences.StreamProfilesActivity;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;

/** Settings' sub-pages stood on the bare canvas while Settings has the sky: art-less pages get the night sky. */
@RunWith(RobolectricTestRunner.class)
@org.robolectric.annotation.Config(sdk = 34, application = android.app.Application.class,
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class SubPageSkyTest {
    private static void assertSkyBehind(Activity page) {
        ViewGroup content = page.findViewById(android.R.id.content);
        assertTrue(page.getClass().getSimpleName() + " has the sky at the back", content.getChildAt(0) instanceof NightSkyView);
        assertFalse("rows fill the page: no moon behind their text", ((NightSkyView) content.getChildAt(0)).isMoonShown());
    }

    @Test
    public void profiles() {
        assertSkyBehind(Robolectric.buildActivity(StreamProfilesActivity.class).setup().get());
    }

    @Test
    public void pinLock() {
        assertSkyBehind(Robolectric.buildActivity(PinLockSettingsActivity.class).setup().get());
    }

    @Test
    public void hostSettings() {
        Intent intent = new Intent(org.robolectric.RuntimeEnvironment.getApplication(), HostStreamSettings.class)
                .putExtra("host_uuid", "sky-pc").putExtra("host_name", "PC");
        assertSkyBehind(Robolectric.buildActivity(HostStreamSettings.class, intent).setup().get());
    }
}
