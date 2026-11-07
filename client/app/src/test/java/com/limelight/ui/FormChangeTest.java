package com.limelight.ui;

import static org.junit.Assert.assertEquals;

import android.app.Application;
import android.os.Bundle;

import com.limelight.R;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowLooper;

/** A screen that handles its own configuration changes is laid out again when the window changes form, and only then. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 34, application = Application.class, qualifiers = "w411dp-h891dp-port")
public class FormChangeTest {
    public static class Screen extends NightjarActivity {
        static int created;

        @Override protected void onCreate(Bundle state) {
            setTheme(R.style.AppTheme);
            super.onCreate(state);
            created++;
        }
    }

    @Test
    public void unfoldingIntoTheHandheldFormLaysTheScreenOutAgain() {
        Screen.created = 0;
        try (var controller = Robolectric.buildActivity(Screen.class).setup()) {
            Screen screen = controller.get();
            assertEquals(GlassForm.PHONE, GlassForm.of(screen));

            // A taller phone window in landscape, still under 600 dp wide: the same form.
            RuntimeEnvironment.setQualifiers("w560dp-h411dp-land");
            screen.onConfigurationChanged(screen.getResources().getConfiguration());
            ShadowLooper.idleMainLooper();
            assertEquals("no new form, nothing laid out again", 1, Screen.created);

            RuntimeEnvironment.setQualifiers("w900dp-h700dp-land");
            screen.onConfigurationChanged(screen.getResources().getConfiguration());
            ShadowLooper.idleMainLooper();
            assertEquals(GlassForm.HANDHELD, GlassForm.of(screen));
            assertEquals("the handheld's layout, not the phone's under it", 2, Screen.created);
            org.junit.Assert.assertTrue("a subclass is told, so it does not recreate a second time", screen.recreatingForForm());
        }
    }
}
