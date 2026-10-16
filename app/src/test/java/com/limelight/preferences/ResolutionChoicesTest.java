package com.limelight.preferences;

import android.app.Application;
import android.content.SharedPreferences;
import android.graphics.Insets;
import android.graphics.Rect;
import android.preference.ListPreference;
import android.preference.PreferenceManager;
import android.view.Display;
import android.view.DisplayCutout;

import com.limelight.R;

import org.junit.After;
import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowDisplay;
import org.robolectric.shadows.ShadowDisplayManager;
import org.robolectric.util.ReflectionHelpers;
import org.robolectric.util.ReflectionHelpers.ClassParameter;

import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;

import static org.junit.Assert.*;
import static org.robolectric.Shadows.shadowOf;

/** A 1600 × 720 phone with a camera cutout along its top edge. */
@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 30, qualifiers = "w360dp-h800dp-xhdpi", application = Application.class,
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class ResolutionChoicesTest {
    private static SharedPreferences prefs() {
        return PreferenceManager.getDefaultSharedPreferences(RuntimeEnvironment.getApplication());
    }

    @Before
    public void phone() {
        Display display = ShadowDisplay.getDefaultDisplay();
        shadowOf(display).setDisplayCutout(new DisplayCutout(Insets.of(0, 80, 0, 0), null,
                new Rect(320, 0, 400, 80), null, null));
        Display.Mode mode = ReflectionHelpers.callConstructor(Display.Mode.class,
                ClassParameter.from(int.class, display.getMode().getModeId()), ClassParameter.from(int.class, 720),
                ClassParameter.from(int.class, 1600), ClassParameter.from(float.class, 144f));
        ShadowDisplayManager.setSupportedModes(display.getDisplayId(), mode);
    }

    @After
    public void clear() {
        prefs().edit().clear().commit();
    }

    private static ListPreference choices(String key) {
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            return (ListPreference) SettingsStructureTest.open(controller.get(), "video").findPreference(key);
        }
    }

    private static List<String> values(ListPreference list) {
        List<String> values = new ArrayList<>();
        for (CharSequence value : list.getEntryValues()) values.add(value.toString());
        return values;
    }

    @Test
    public void theScreenSizeLeadsAndLargerFixedSizesAreLeftOut() {
        ListPreference resolution = choices(PreferenceConfiguration.RESOLUTION_PREF_STRING);
        assertEquals(Arrays.asList("1600x720", "1520x720", "1280x720", "854x480", "640x360"), values(resolution));
        Application app = RuntimeEnvironment.getApplication();
        assertEquals(app.getString(R.string.resolution_screen, "1600×720"), resolution.getEntries()[0].toString());
        assertEquals(app.getString(R.string.resolution_screen_cutout, "1520×720"), resolution.getEntries()[1].toString());
        assertEquals(app.getString(R.string.resolution_720p), resolution.getEntries()[2].toString());
    }

    @Test
    public void aLargerSizeChosenEarlierStaysAvailable() {
        prefs().edit().putString(PreferenceConfiguration.RESOLUTION_PREF_STRING, "1920x1080")
                .putBoolean(PreferenceConfiguration.RESOLUTION_FOLLOWS_SCREEN_PREF_STRING, false)
                .putInt(PreferenceConfiguration.PREFS_VERSION_PREF_STRING, PreferenceConfiguration.PROFILE_NATIVE_VERSION)
                .commit();
        ListPreference resolution = choices(PreferenceConfiguration.RESOLUTION_PREF_STRING);
        assertEquals(Arrays.asList("1920x1080", "1600x720", "1520x720", "1280x720", "854x480", "640x360"), values(resolution));
        assertEquals("1920x1080", resolution.getValue());
    }

    @Test
    public void theScreensRefreshRateIsNamedAsSuch() {
        ListPreference fps = choices(PreferenceConfiguration.FPS_PREF_STRING);
        assertEquals("144", fps.getEntryValues()[fps.getEntryValues().length - 1].toString());
        assertEquals(RuntimeEnvironment.getApplication().getString(R.string.fps_screen, 144),
                fps.getEntries()[fps.getEntries().length - 1].toString());
    }

    @Test
    public void fixedSizesFitWhenNeitherSideIsLarger() {
        assertTrue(StreamSettings.SettingsFragment.fitsPanel("1920x1080", "2400x1080"));
        assertFalse(StreamSettings.SettingsFragment.fitsPanel("2560x1440", "2400x1080"));
        assertFalse(StreamSettings.SettingsFragment.fitsPanel("3840x2160", "2400x1080"));
        assertTrue(StreamSettings.SettingsFragment.fitsPanel("2560x1440", "2560x1600"));
        assertTrue(StreamSettings.SettingsFragment.fitsPanel("3840x2160", null));
    }

    @Test
    public void aFoldablesScreenSizeIsThePanelInUse() {
        // Galaxy Z Fold 7: one display lists the 2184x1968 inner and the 2520x1080 cover panel.
        int[][] fold = {{1968, 2184}, {1080, 2520}};
        assertArrayEquals(new int[] {1080, 2520}, PreferenceConfiguration.panelSize(2520, 1080, fold));
        assertArrayEquals(new int[] {1968, 2184}, PreferenceConfiguration.panelSize(2184, 1968, fold));
        assertArrayEquals(new int[] {1968, 2184}, PreferenceConfiguration.panelSize(1968, 2184, fold));
        // A panel scaled to a lower resolution keeps its own full mode.
        assertArrayEquals(new int[] {1440, 3120}, PreferenceConfiguration.panelSize(2340, 1080,
                new int[][] {{1080, 2340}, {1440, 3120}, {720, 1560}}));
        // A size in no mode's shape, such as a free-form window, keeps the largest mode.
        assertArrayEquals(new int[] {1968, 2184}, PreferenceConfiguration.panelSize(1200, 800, fold));
    }

    @Test
    public void theScreenResolutionFollowsTheCoverPanelWhenTheInnerOneIsListedToo() {
        Display display = ShadowDisplay.getDefaultDisplay();
        Display.Mode cover = ReflectionHelpers.callConstructor(Display.Mode.class,
                ClassParameter.from(int.class, display.getMode().getModeId()), ClassParameter.from(int.class, 720),
                ClassParameter.from(int.class, 1600), ClassParameter.from(float.class, 144f));
        Display.Mode inner = ReflectionHelpers.callConstructor(Display.Mode.class,
                ClassParameter.from(int.class, display.getMode().getModeId() + 1), ClassParameter.from(int.class, 1800),
                ClassParameter.from(int.class, 2000), ClassParameter.from(float.class, 120f));
        ShadowDisplayManager.setSupportedModes(display.getDisplayId(), cover, inner);
        assertEquals("1600x720", PreferenceConfiguration.nativeResolution(RuntimeEnvironment.getApplication()));
    }
}
