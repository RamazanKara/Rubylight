package com.limelight;

import android.app.Application;
import android.content.Intent;
import android.os.Build;
import android.view.View;

import com.google.android.material.appbar.MaterialToolbar;
import com.limelight.preferences.GlPreferences;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;

import static org.junit.Assert.*;
import static org.robolectric.Shadows.shadowOf;

@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class, shadows = PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class HelpTopicsActivityTest {
    @Test
    public void topicsAreBundledAndEachLinksToItsWebPage() {
        try (ActivityController<HelpTopicsActivity> controller = Robolectric.buildActivity(HelpTopicsActivity.class).setup()) {
            HelpTopicsActivity page = controller.get();
            assertEquals(page.getString(R.string.help),
                    ((MaterialToolbar) page.findViewById(R.id.page_toolbar)).getTitle().toString());
            String[][] links = {
                    {Integer.toString(R.id.help_connect_link), "quick-start.md"},
                    {Integer.toString(R.id.help_pairing_link), "pairing.md"},
                    {Integer.toString(R.id.help_controls_link), "controls.md"},
                    {Integer.toString(R.id.help_problems_link), "TROUBLESHOOTING.md"},
                    {Integer.toString(R.id.help_report_link), "TROUBLESHOOTING.md#export-a-problem-report"},
            };
            for (String[] link : links) {
                View button = page.findViewById(Integer.parseInt(link[0]));
                assertEquals(View.VISIBLE, button.getVisibility());
                button.performClick();
                Intent opened = shadowOf(page).getNextStartedActivity();
                assertEquals(Intent.ACTION_VIEW, opened.getAction());
                assertEquals(HelpTopicsActivity.DOCS_URL + link[1], opened.getDataString());
                assertTrue(opened.getDataString().startsWith("https://github.com/RamazanKara/nightjar/blob/main/docs/"));
            }
            // The topics themselves need no network.
            assertNotNull(page.findViewById(R.id.help_report));
        }
    }

    @Test
    public void homeHelpOpensTheBundledPage() {
        Application application = RuntimeEnvironment.getApplication();
        GlPreferences gl = GlPreferences.readPreferences(application);
        gl.savedFingerprint = Build.FINGERPRINT;
        gl.glRenderer = "Test renderer";
        gl.writePreferences();
        application.getSharedPreferences("FirstRun", 0).edit().putBoolean("pairing_guide_shown", true).commit();
        ActivityController<PcViewLifecycleTest.Library> home = Robolectric.buildActivity(PcViewLifecycleTest.Library.class).setup();
        try {
            MaterialToolbar toolbar = home.get().findViewById(R.id.home_toolbar);
            assertTrue(toolbar.getMenu().performIdentifierAction(R.id.helpButton, 0));
            assertEquals(HelpTopicsActivity.class.getName(),
                    shadowOf(home.get()).getNextStartedActivity().getComponent().getClassName());
        } finally {
            home.pause().stop().destroy();
        }
    }
}
