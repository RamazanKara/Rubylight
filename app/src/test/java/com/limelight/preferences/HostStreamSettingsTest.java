package com.limelight.preferences;

import android.app.Application;
import android.content.Context;
import android.content.Intent;
import android.os.Looper;
import android.preference.Preference;

import androidx.appcompat.app.AlertDialog;

import com.limelight.PcViewLifecycleTest;
import com.limelight.R;
import com.limelight.computers.HostCapabilityCache;
import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.http.TestHosts;

import org.junit.After;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.Shadows;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowDialog;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertTrue;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class,
        shadows = PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class HostStreamSettingsTest {
    private final Application app = RuntimeEnvironment.getApplication();

    @After
    public void clear() {
        for (String name : new String[] {StreamProfiles.PREFERENCES, "AppStreamSettings", "HostStreamProfiles"}) {
            app.getSharedPreferences(name, Context.MODE_PRIVATE).edit().clear().commit();
        }
    }

    private static HostCapabilities rubylightWithNvidiaEnhancements() {
        return TestHosts.caps(TestHosts.serverInfo("rubylight-serverinfo.xml")
                .replace("</root>", "<NvidiaEnhancements>3</NvidiaEnhancements></root>"));
    }

    private static String summary(Preference row) {
        assertNotNull(row);
        return row.getSummary() == null ? "" : row.getSummary().toString();
    }

    private static void click(Preference preference) {
        preference.getOnPreferenceClickListener().onPreferenceClick(preference);
        Shadows.shadowOf(Looper.getMainLooper()).idle();
    }

    @Test
    public void gameSettingsOfferNvidiaEnhancementsAndSavePerGameOverrides() {
        HostCapabilityCache.save(app, "nvidia-pc", rubylightWithNvidiaEnhancements());
        HostStreamProfile.deserialize("v2;rtx_hdr=true").save(app, "nvidia-pc");
        Intent intent = HostStreamSettings.intentForGame(app, "nvidia-pc", "Den PC", "game-uuid", 7, "Racer");
        ActivityController<HostStreamSettings> controller = Robolectric.buildActivity(HostStreamSettings.class, intent).setup();
        try {
            HostStreamSettings screen = controller.get();
            HostStreamSettings.SettingsFragment fragment = (HostStreamSettings.SettingsFragment)
                    screen.getFragmentManager().findFragmentById(R.id.host_profile_settings);
            Preference rtxHdr = fragment.findPreference("rtx_hdr");
            Preference rtxVideo = fragment.findPreference("rtx_video");
            assertNotNull(rtxHdr);
            assertNotNull(rtxVideo);

            String expectedFromPc = app.getString(R.string.layers_value_from,
                    app.getString(R.string.host_profile_on), app.getString(R.string.layers_from_pc));
            assertTrue(summary(rtxHdr), summary(rtxHdr).startsWith(expectedFromPc + "\n"));

            String expectedDefault = app.getString(R.string.layers_value_from,
                    app.getString(R.string.fec_choice_host), app.getString(R.string.layers_from_main));
            assertTrue(summary(rtxVideo), summary(rtxVideo).startsWith(expectedDefault + "\n"));

            click(rtxHdr);
            AlertDialog hdrDialog = (AlertDialog) ShadowDialog.getLatestDialog();
            assertEquals(app.getString(R.string.layers_not_set_value, app.getString(R.string.host_profile_on)),
                    hdrDialog.getListView().getAdapter().getItem(0).toString());
            hdrDialog.getListView().performItemClick(null, 2, 2);
            Shadows.shadowOf(Looper.getMainLooper()).idle();
            assertTrue(summary(rtxHdr), summary(rtxHdr).startsWith(app.getString(R.string.host_profile_off) + "\n"));

            click(rtxVideo);
            AlertDialog videoDialog = (AlertDialog) ShadowDialog.getLatestDialog();
            assertEquals(app.getString(R.string.layers_not_set_value, app.getString(R.string.fec_choice_host)),
                    videoDialog.getListView().getAdapter().getItem(0).toString());
            videoDialog.getListView().performItemClick(null, 1, 1);
            Shadows.shadowOf(Looper.getMainLooper()).idle();
            assertTrue(summary(rtxVideo), summary(rtxVideo).startsWith(app.getString(R.string.host_profile_on) + "\n"));

            screen.findViewById(R.id.host_profile_save).performClick();
            HostStreamProfile saved = AppStreamSettings.load(app, "nvidia-pc", "game-uuid", 7);
            assertEquals(Boolean.FALSE, saved.rtxHdr);
            assertEquals(Boolean.TRUE, saved.rtxVideo);
        } finally {
            controller.pause().stop().destroy();
        }
    }
}
