package com.limelight.preferences;

import android.app.Application;
import android.content.Context;
import android.content.SharedPreferences;
import android.preference.PreferenceManager;
import android.widget.EditText;

import androidx.appcompat.app.AlertDialog;

import com.google.android.material.textfield.TextInputLayout;
import com.limelight.R;
import com.limelight.nvstream.http.HostCapabilities;

import org.junit.After;
import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowDialog;
import org.robolectric.shadows.ShadowLooper;

import static org.junit.Assert.*;

@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 30, qualifiers = "w360dp-h800dp-xhdpi", application = Application.class,
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class CustomResolutionTest {
    private final Application app = RuntimeEnvironment.getApplication();

    private SharedPreferences prefs() {
        return PreferenceManager.getDefaultSharedPreferences(app);
    }

    @Before
    public void initialize() {
        PreferenceConfiguration.readPreferences(app);
        prefs().edit().putString(PreferenceConfiguration.RESOLUTION_PREF_STRING, "1280x720")
                .putBoolean(PreferenceConfiguration.RESOLUTION_FOLLOWS_SCREEN_PREF_STRING, false).commit();
    }

    @After
    public void clear() {
        prefs().edit().clear().commit();
        for (String name : new String[] {StreamProfiles.PREFERENCES, "HostStreamProfiles", "AppStreamSettings"}) {
            app.getSharedPreferences(name, Context.MODE_PRIVATE).edit().clear().commit();
        }
    }

    private ResolutionPreference resolution(StreamSettings screen) {
        return (ResolutionPreference) SettingsStructureTest.open(screen, "video")
                .findPreference(PreferenceConfiguration.RESOLUTION_PREF_STRING);
    }

    private AlertDialog editor(ResolutionPreference pref) {
        pref.showDialog(null);
        AlertDialog picker = (AlertDialog) ShadowDialog.getLatestDialog();
        assertEquals(app.getString(R.string.resolution_custom),
                picker.getButton(AlertDialog.BUTTON_POSITIVE).getText().toString());
        picker.getButton(AlertDialog.BUTTON_POSITIVE).performClick();
        ShadowLooper.idleMainLooper();
        return (AlertDialog) ShadowDialog.getLatestDialog();
    }

    private void enter(AlertDialog dialog, String w, String h) {
        ((EditText) dialog.findViewById(R.id.resolution_width)).setText(w);
        ((EditText) dialog.findViewById(R.id.resolution_height)).setText(h);
        dialog.getButton(AlertDialog.BUTTON_POSITIVE).performClick();
    }

    private void save(StreamSettings screen, int w, int h) {
        AlertDialog dialog = editor(resolution(screen));
        enter(dialog, Integer.toString(w), Integer.toString(h));
        assertFalse(dialog.isShowing());
    }

    private void assertSize(PreferenceConfiguration config, int w, int h) {
        assertEquals(w, config.width);
        assertEquals(h, config.height);
        assertFalse(config.resolutionFollowsScreen);
    }

    private PreferenceConfiguration stream(String pc, String game, int id) {
        return PreferenceConfiguration.readPreferences(app, pc, HostCapabilities.UNKNOWN, game, id,
                StreamLayers.Network.WIFI);
    }

    @Test
    public void customSizePersistsAndIsUsedByEveryPcAndApp() {
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            save(controller.get(), 1872, 1080);
            assertSize(stream("pc-a", "game-a", 1), 1872, 1080);
            assertSize(stream("pc-a", "game-b", 2), 1872, 1080);
            assertSize(stream("pc-b", "desktop", 3), 1872, 1080);
        }
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            ResolutionPreference pref = resolution(controller.get());
            assertEquals("1872x1080", pref.getValue());
            assertEquals(app.getString(R.string.resolution_custom_size, "1872×1080"), pref.getEntry());
            AlertDialog dialog = editor(pref);
            assertEquals("1872", ((EditText) dialog.findViewById(R.id.resolution_width)).getText().toString());
            assertEquals("1080", ((EditText) dialog.findViewById(R.id.resolution_height)).getText().toString());
        }
    }

    @Test
    public void pcAndGameOverridesStillTakePriorityAndUnrelatedOverridesInherit() {
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            save(controller.get(), 1872, 1080);
        }
        HostStreamProfile.deserialize("v2;res=2560x1440").save(app, "pc-a");
        HostStreamProfile.deserialize("v2;bitrate=25000").save(app, "pc-b");
        AppStreamSettings.save(app, "pc-a", "game-a", 1, HostStreamProfile.deserialize("v2;res=1280x800"));
        assertSize(stream("pc-a", "game-a", 1), 1280, 800);
        assertSize(stream("pc-a", "game-b", 2), 2560, 1440);
        assertSize(stream("pc-b", "game-c", 3), 1872, 1080);
        assertSize(stream("pc-c", "game-d", 4), 1872, 1080);
    }

    @Test
    public void namedProfilesCanOverrideTheGlobalSize() {
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            save(controller.get(), 1872, 1080);
        }
        String profile = StreamProfiles.create(app, "Home", HostStreamProfile.deserialize("v2;res=2560x1600"));
        StreamProfiles.setActive(app, profile);
        assertSize(stream("pc-a", "game-a", 1), 2560, 1600);
        assertSize(PreferenceConfiguration.readPreferences(app), 1872, 1080);
    }

    @Test
    @Config(sdk = 23)
    public void olderAndroidCanSaveAndReadCustomDimensions() {
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            save(controller.get(), 1872, 1080);
            assertSize(PreferenceConfiguration.readPreferences(app), 1872, 1080);
        }
    }

    @Test
    public void manualSizeEqualToPanelStaysFixedButPickingPanelFollowsScreen() {
        String panel = PreferenceConfiguration.nativeResolution(app);
        int w = PreferenceConfiguration.getWidthFromResolutionString(panel);
        int h = PreferenceConfiguration.getHeightFromResolutionString(panel);
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            save(controller.get(), w, h);
            assertSize(PreferenceConfiguration.readPreferences(app), w, h);
            assertEquals(panel, PreferenceConfiguration.effectiveResolution(prefs(), panel, "2400x1080"));
            ResolutionPreference pref = resolution(controller.get());
            pref.showDialog(null);
            AlertDialog picker = (AlertDialog) ShadowDialog.getLatestDialog();
            picker.getListView().performItemClick(null, pref.findIndexOfValue(panel), 0);
            ShadowLooper.idleMainLooper();
            assertTrue(prefs().getBoolean(PreferenceConfiguration.RESOLUTION_FOLLOWS_SCREEN_PREF_STRING, false));
            assertEquals("2400x1080", PreferenceConfiguration.effectiveResolution(prefs(), panel, "2400x1080"));
        }
    }

    @Test
    public void largePresetEnteredManuallyAndPortraitSizesSurviveReopening() {
        for (int[] size : new int[][] {{3840, 2160}, {720, 1600}, {64, 64}, {16384, 16384}}) {
            try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
                save(controller.get(), size[0], size[1]);
            }
            try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
                ResolutionPreference pref = resolution(controller.get());
                assertEquals(size[0] + "x" + size[1], pref.getValue());
                assertTrue(pref.findIndexOfValue(pref.getValue()) >= 0);
                assertSize(PreferenceConfiguration.readPreferences(app), size[0], size[1]);
            }
        }
    }

    @Test
    public void invalidDimensionsStayOpenAndDoNotChangeSavedSettings() {
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            AlertDialog dialog = editor(resolution(controller.get()));
            for (String[] invalid : new String[][] {{"", "1080"}, {"63", "1080"}, {"1872", "16385"},
                    {"2147483648", "1080"}, {"1872", "0"}, {"-800", "1080"}, {"1.5", "1080"}}) {
                enter(dialog, invalid[0], invalid[1]);
                assertTrue(dialog.isShowing());
                assertEquals("1280x720", prefs().getString(PreferenceConfiguration.RESOLUTION_PREF_STRING, null));
                EditText width = dialog.findViewById(R.id.resolution_width);
                EditText height = dialog.findViewById(R.id.resolution_height);
                TextInputLayout widthContainer = (TextInputLayout) width.getParent().getParent();
                TextInputLayout heightContainer = (TextInputLayout) height.getParent().getParent();
                assertTrue(widthContainer.getError() != null || heightContainer.getError() != null);
            }
            enter(dialog, "1872", "1080");
            assertFalse(dialog.isShowing());
            assertSize(PreferenceConfiguration.readPreferences(app), 1872, 1080);
        }
    }

    @Test
    public void cancellingDoesNotChangeTheResolutionOrBitrate() {
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            AlertDialog dialog = editor(resolution(controller.get()));
            java.util.Map<String, ?> before = prefs().getAll();
            ((EditText) dialog.findViewById(R.id.resolution_width)).setText("1872");
            dialog.getButton(AlertDialog.BUTTON_NEGATIVE).performClick();
            assertEquals(before, prefs().getAll());
        }
    }

    @Test
    public void defaultBitrateFollowsCustomResolutionAndManualBitrateIsPreserved() {
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            prefs().edit().putBoolean(PreferenceConfiguration.BITRATE_USER_SET_PREF_STRING, false).commit();
            save(controller.get(), 1872, 1080);
            assertEquals(PreferenceConfiguration.getDefaultBitrate("1872x1080",
                    prefs().getString(PreferenceConfiguration.FPS_PREF_STRING, PreferenceConfiguration.DEFAULT_FPS)),
                    prefs().getInt(PreferenceConfiguration.BITRATE_PREF_STRING, 0));
            prefs().edit().putInt(PreferenceConfiguration.BITRATE_PREF_STRING, 27000)
                    .putBoolean(PreferenceConfiguration.BITRATE_USER_SET_PREF_STRING, true).commit();
            save(controller.get(), 1280, 800);
            assertEquals(27000, prefs().getInt(PreferenceConfiguration.BITRATE_PREF_STRING, 0));
            assertTrue(PreferenceConfiguration.isBitrateUserSet(prefs()));
        }
    }
}
