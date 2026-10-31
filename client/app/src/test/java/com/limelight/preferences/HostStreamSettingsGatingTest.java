package com.limelight.preferences;

import android.app.Application;
import android.content.Intent;
import android.preference.Preference;

import androidx.appcompat.app.AlertDialog;

import com.limelight.R;
import com.limelight.computers.HostCapabilityCache;
import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.http.TestHosts;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowDialog;
import org.robolectric.util.ReflectionHelpers;

import java.util.function.Consumer;

import static org.junit.Assert.*;

/** Per-PC settings show what the PC can use, explain what it can't yet, and leave out the rest. */
@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class,
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class HostStreamSettingsGatingTest {
    private final Application app = RuntimeEnvironment.getApplication();

    private void withSettings(String uuid, HostCapabilities caps, Consumer<HostStreamSettings.SettingsFragment> check) {
        if (caps != null) {
            HostCapabilityCache.save(app, uuid, caps);
        }
        Intent intent = new Intent(app, HostStreamSettings.class).putExtra("host_uuid", uuid).putExtra("host_name", "PC");
        ActivityController<HostStreamSettings> controller = Robolectric.buildActivity(HostStreamSettings.class, intent).setup();
        try {
            check.accept((HostStreamSettings.SettingsFragment)
                    controller.get().getFragmentManager().findFragmentById(R.id.host_profile_settings));
        } finally {
            controller.pause().stop().destroy();
        }
    }

    private static String summary(Preference preference) {
        return preference.getSummary() == null ? "" : preference.getSummary().toString();
    }

    private int offeredCodecs(HostStreamSettings.SettingsFragment fragment) {
        ReflectionHelpers.callInstanceMethod(fragment, "showCodec");
        AlertDialog dialog = (AlertDialog) ShadowDialog.getLatestDialog();
        int count = dialog.getListView().getAdapter().getCount();
        dialog.dismiss();
        return count;
    }

    @Test
    public void sunshineHasNoDisplayRowsAndNoPyroWaveChoice() {
        withSettings("sunshine-pc", TestHosts.fixture("sunshine-serverinfo.xml"), fragment -> {
            assertNull(fragment.findPreference("host_display"));
            assertNull(fragment.findPreference("virtual"));
            assertNull(fragment.findPreference("scale"));
            assertNull(fragment.findPreference("dpi"));
            assertEquals(app.getString(R.string.settings_section_video),
                    fragment.getPreferenceScreen().getPreference(0).getTitle().toString());
            // "Same as all PCs", Automatic, AV1, HEVC and H.264
            assertEquals(5, offeredCodecs(fragment));
        });
    }

    @Test
    public void apolloGetsAPlainVirtualDisplaySwitchEvenWithASavedDisplayMode() {
        HostDisplayChoice.save(app, "apollo-pc", HostDisplayChoice.EXCLUSIVE);
        withSettings("apollo-pc", TestHosts.fixture("apollo-serverinfo.xml"), fragment -> {
            assertNull(fragment.findPreference("host_display"));
            Preference virtual = fragment.findPreference("virtual");
            assertTrue(virtual.isEnabled());
            assertFalse(summary(virtual).contains(app.getString(R.string.host_display_exclusive)));
            assertTrue(fragment.findPreference("scale").isEnabled());
            // Apollo has a virtual display but never reads rlDisplayScale.
            assertNull(fragment.findPreference("dpi"));
        });
    }

    @Test
    public void rubylightGetsThePickerAndTheSwitchWithoutNotes() {
        withSettings("rubylight-pc", TestHosts.rubylight(), fragment -> {
            Preference display = fragment.findPreference("host_display");
            assertTrue(display.isEnabled());
            assertFalse(summary(display).contains(app.getString(R.string.host_feature_unknown)));
            assertTrue(fragment.findPreference("virtual").isEnabled());
            assertEquals(6, offeredCodecs(fragment));
        });
    }

    @Test
    public void pyroWaveSaysWhyWhenThisDeviceCanNeverRunIt() {
        // The JVM has no PyroWave renderer library, like a 32-bit app: the list for all PCs labels
        // PyroWave with the reason, and so does the per-PC picker.
        int reason = com.limelight.binding.video.PyroWaveDecoderRenderer.neverAvailableReason();
        assertNotEquals(0, reason);
        withSettings("rubylight-pc", TestHosts.rubylight(), fragment -> {
            ReflectionHelpers.callInstanceMethod(fragment, "showCodec");
            AlertDialog dialog = (AlertDialog) ShadowDialog.getLatestDialog();
            int pyroWave = PreferenceConfiguration.FormatOption.FORCE_PYROWAVE.ordinal() + 1;
            String item = dialog.getListView().getAdapter().getItem(pyroWave).toString();
            String other = dialog.getListView().getAdapter().getItem(pyroWave - 1).toString();
            dialog.dismiss();
            assertTrue(item, item.endsWith(" · " + app.getString(reason)));
            assertFalse(other, other.contains(app.getString(reason)));
        });
    }

    @Test
    public void aPcChoosesItsOwnDisplayScaleApartFromTheRenderResolution() {
        withSettings("rubylight-dpi-pc", TestHosts.rubylightWithDisplayScale(), fragment -> {
            Preference dpi = fragment.findPreference("dpi");
            assertTrue(dpi.isEnabled());
            assertTrue(summary(dpi).startsWith(app.getString(R.string.host_profile_follows,
                    app.getString(R.string.virtual_display_dpi_pc_own))));
            ReflectionHelpers.callInstanceMethod(fragment, "showDisplayScale");
            AlertDialog dialog = (AlertDialog) ShadowDialog.getLatestDialog();
            // "Same as all PCs", the PC's own and the twelve Windows scales
            assertEquals(14, dialog.getListView().getAdapter().getCount());
            assertEquals("175%", dialog.getListView().getAdapter().getItem(5).toString());
            dialog.getListView().performItemClick(null, 5, 5);
            assertTrue(summary(dpi), summary(dpi).startsWith("175%\n"));
            assertTrue(summary(fragment.findPreference("scale")).startsWith(
                    app.getString(R.string.host_profile_follows, "100%")));
        });
    }

    @Test
    public void aRubylightWithoutDisplayScaleExplainsWhyTheRowIsUnavailable() {
        // 2.6.x and 3.0.0-rc.5 send no DisplayScaleVersion and ignore rlDisplayScale.
        withSettings("no-dpi-pc", TestHosts.rubylight(), fragment -> {
            Preference dpi = fragment.findPreference("dpi");
            assertFalse(dpi.isEnabled());
            assertTrue(summary(dpi), summary(dpi).endsWith(app.getString(R.string.host_feature_reason_update)));
            assertTrue(fragment.findPreference("scale").isEnabled());
        });
    }

    @Test
    public void olderRubylightExplainsWhyThePickerIsUnavailable() {
        withSettings("old-rubylight-pc", TestHosts.fixture("butterpollo-serverinfo.xml"), fragment -> {
            Preference display = fragment.findPreference("host_display");
            assertFalse(display.isEnabled());
            assertTrue(summary(display).endsWith(app.getString(R.string.host_feature_reason_update)));
            assertTrue(fragment.findPreference("virtual").isEnabled());
        });
    }

    @Test
    public void aVirtualDisplayTurnedOffOnTheHostDisablesItsRowsWithTheReason() {
        HostCapabilities off = TestHosts.caps(TestHosts.serverInfo("rubylight-serverinfo.xml")
                .replace("<VirtualDisplayCapable>true", "<VirtualDisplayCapable>false")
                .replace("</root>", "<DisplayScaleVersion>1</DisplayScaleVersion></root>"));
        withSettings("display-off-pc", off, fragment -> {
            String reason = app.getString(R.string.host_feature_reason_virtual_display_off);
            for (String key : new String[] {"host_display", "virtual", "scale", "dpi"}) {
                Preference row = fragment.findPreference(key);
                assertFalse(key, row.isEnabled());
                assertTrue(key, summary(row).endsWith(reason));
            }
        });
    }

    @Test
    public void aPcNeverReachedKeepsEveryRowWithANote() {
        withSettings("new-pc", null, fragment -> {
            String note = app.getString(R.string.host_feature_unknown);
            for (String key : new String[] {"host_display", "virtual", "scale", "dpi"}) {
                Preference row = fragment.findPreference(key);
                assertTrue(key, row.isEnabled());
                assertTrue(key, summary(row).endsWith(note));
            }
            assertEquals(6, offeredCodecs(fragment));
        });
    }

    @Test
    public void aPyroWaveChoiceOnAHostWithoutItSaysWhichCodecIsUsed() {
        HostStreamProfile profile = new HostStreamProfile();
        profile.codec = PreferenceConfiguration.FormatOption.FORCE_PYROWAVE;
        profile.save(app, "no-pyrowave-pc");
        withSettings("no-pyrowave-pc", TestHosts.fixture("apollo-serverinfo.xml"), fragment -> {
            String automatic = app.getResources().getStringArray(R.array.host_profile_codecs)[0];
            assertTrue(summary(fragment.findPreference("codec"))
                    .endsWith(app.getString(R.string.host_pyrowave_unavailable, automatic)));
        });
    }

    @Test
    @Config(qualifiers = "television")
    public void withARemoteTheFirstRowTakesFocusNotTheHelpButton() {
        Intent intent = new Intent(app, HostStreamSettings.class).putExtra("host_uuid", "focus-pc").putExtra("host_name", "PC");
        ActivityController<HostStreamSettings> controller = Robolectric.buildActivity(HostStreamSettings.class, intent)
                .setup().visible();
        try {
            org.robolectric.Shadows.shadowOf(android.os.Looper.getMainLooper()).idle();
            android.view.View list = controller.get().findViewById(android.R.id.list);
            assertFalse(list.isInTouchMode());
            assertTrue("the rows have focus", list.hasFocus());
        } finally {
            controller.pause().stop().destroy();
        }
    }
}
