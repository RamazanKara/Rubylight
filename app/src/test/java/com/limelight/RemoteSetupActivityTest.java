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
