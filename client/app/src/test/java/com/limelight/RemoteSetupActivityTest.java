package com.limelight;

import android.content.Intent;
import android.content.ServiceConnection;
import android.os.Bundle;
import android.view.View;
import android.widget.TextView;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 28, application = android.app.Application.class,
        shadows = PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class RemoteSetupActivityTest {
    @Test public void sameCertificateAndUuidDoNotBypassRevokedPairingOrViewingRights() {
        com.limelight.nvstream.http.ComputerDetails expected = new com.limelight.nvstream.http.ComputerDetails();
        expected.uuid = "paired-pc";
        com.limelight.nvstream.http.ComputerDetails candidate = new com.limelight.nvstream.http.ComputerDetails(expected);
        assertEquals(R.string.remote_pair_revoked, RemoteSetupActivity.verificationProblem(expected, candidate));
        candidate.pairState = com.limelight.nvstream.http.PairingManager.PairState.PAIRED;
        candidate.permission = 0;
        assertEquals(R.string.remote_permission_denied, RemoteSetupActivity.verificationProblem(expected, candidate));
        candidate.permission = com.limelight.nvstream.http.ComputerDetails.PERMISSION_LAUNCH;
        assertEquals(0, RemoteSetupActivity.verificationProblem(expected, candidate));
        candidate.permission = -1; // Sunshine does not advertise the permissions extension.
        assertEquals(0, RemoteSetupActivity.verificationProblem(expected, candidate));
        candidate.uuid = "another-pc";
        assertEquals(R.string.remote_failed, RemoteSetupActivity.verificationProblem(expected, candidate));
    }
    /** The text of {@code id} for {@code locale} on a phone or on a TV. */
    private static String text(int id, String locale, boolean television) {
        android.content.res.Configuration config = new android.content.res.Configuration(
                org.robolectric.RuntimeEnvironment.getApplication().getResources().getConfiguration());
        config.setLocale(java.util.Locale.forLanguageTag(locale));
        config.uiMode = (config.uiMode & ~android.content.res.Configuration.UI_MODE_TYPE_MASK) |
                (television ? android.content.res.Configuration.UI_MODE_TYPE_TELEVISION : android.content.res.Configuration.UI_MODE_TYPE_NORMAL);
        return org.robolectric.RuntimeEnvironment.getApplication().createConfigurationContext(config).getString(id);
    }

    /**
     * TV run on a TCL C6K (German): step 1 said "Das Telefon allein…". On a TV, Prepare and Connect speak of this
     * device in every language the guide has, and phones keep their text.
     */
    @Test public void onATvTheGuideSpeaksOfThisDeviceNotAPhone() {
        java.io.File[] dirs = new java.io.File("src/main/res").listFiles((dir, name) ->
                name.matches("values(-[a-z]{2}(-r[A-Z]{2})?)?") && new java.io.File(dir, name + "/strings_remote_setup.xml").exists());
        assertNotNull(dirs);
        assertEquals("the guide's languages", 15, dirs.length);
        for (java.io.File dir : dirs) {
            String qualifier = dir.getName().equals("values") ? "" : dir.getName().substring("values-".length());
            assertTrue(qualifier + " has a TV override",
                    new java.io.File(dir.getParentFile(), dir.getName() + "-television/strings_remote_setup.xml").exists());
            String locale = qualifier.isEmpty() ? "en" : qualifier.replace("-r", "-");
            for (int id : new int[] {R.string.remote_prepare, R.string.remote_vpn}) {
                assertNotEquals(locale + " TV text differs from the phone's", text(id, locale, false), text(id, locale, true));
            }
        }
        assertFalse(text(R.string.remote_prepare, "en", true).toLowerCase(java.util.Locale.ROOT).contains("phone"));
        assertFalse(text(R.string.remote_vpn, "en", true).toLowerCase(java.util.Locale.ROOT).contains("phone"));
        assertFalse(text(R.string.remote_prepare, "de", true).contains("Telefon"));
        assertFalse(text(R.string.remote_vpn, "de", true).contains("Telefon"));
        assertTrue("phones keep their text", text(R.string.remote_prepare, "de", false).contains("Das Telefon allein"));
    }

    public static class OfflineActivity extends RemoteSetupActivity {
        @Override public boolean bindService(Intent intent, ServiceConnection connection, int flags) { return false; }
    }
    @Test public void guideNavigatesAndPreservesStepWithoutAnyServiceOrNetwork() {
        try (ActivityController<OfflineActivity> controller = Robolectric.buildActivity(OfflineActivity.class).setup()) {
            OfflineActivity activity = controller.get();
            assertFalse(activity.findViewById(R.id.remote_previous).isEnabled());
            activity.findViewById(R.id.remote_next).performClick();
            assertEquals(View.VISIBLE, activity.findViewById(R.id.remote_link).getVisibility());
            activity.findViewById(R.id.remote_next).performClick();
            assertEquals(View.GONE, activity.findViewById(R.id.remote_next).getVisibility());
            assertFalse(activity.findViewById(R.id.remote_test).isEnabled());
            assertTrue(((TextView) activity.findViewById(R.id.remote_description)).getText().toString().contains("another network"));
            Bundle saved = new Bundle();
            controller.saveInstanceState(saved);
            assertEquals(2, saved.getInt("step"));
            activity.findViewById(R.id.remote_previous).performClick();
            assertEquals(View.VISIBLE, activity.findViewById(R.id.remote_next).getVisibility());
        }
    }
}
