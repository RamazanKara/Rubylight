package com.limelight.preferences;

import android.app.Application;
import android.content.Context;
import android.content.pm.PackageManager;
import android.preference.PreferenceGroup;
import android.preference.PreferenceManager;
import android.preference.PreferenceScreen;

import org.junit.After;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;

import java.util.ArrayList;
import java.util.List;

import static com.limelight.preferences.SettingsStructureTest.keys;
import static com.limelight.preferences.SettingsStructureTest.open;
import static org.robolectric.Shadows.shadowOf;

/**
 * How many rows Settings shows at the defaults with every More row open, per section the list of sections shows: the
 * number the settings-simplification work counts (134 before rows that can do nothing were left out). A touch-screen device
 * with nothing attached that does not fold, on a chip Chipset tuning does not tune. Set NIGHTJAR_ROW_COUNTS to print them
 * by section.
 */
@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 28, qualifiers = "en", application = Application.class,
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class SettingsRowCountTest {
    private static Context context() {
        return RuntimeEnvironment.getApplication();
    }

    @After
    public void clear() {
        PreferenceManager.getDefaultSharedPreferences(context()).edit().clear().commit();
        context().getSharedPreferences("SettingsScreenState", Context.MODE_PRIVATE).edit().clear().commit();
    }

    /** Settings' rows at the defaults, every More row open, including Show debug settings, NJ#187 controller mouse sensitivity, NJ#191 compact overlay device info, NJ#190 Picture position, Pinch to zoom, and Controls' Controller row that opens the controller editor. */
    static final int ROWS_AT_DEFAULTS = 117;

    @Test
    public void rowCountsAtTheDefaults() {
        boolean print = System.getenv("NIGHTJAR_ROW_COUNTS") != null;
        shadowOf(context().getPackageManager()).setSystemFeature(PackageManager.FEATURE_TOUCHSCREEN, true);
        ActivityController<StreamSettings> built = Robolectric.buildActivity(StreamSettings.class);
        built.get().devices = SettingsHideTest.NOTHING_ATTACHED;
        try (ActivityController<StreamSettings> controller = built.setup()) {
            StreamSettings screen = controller.get();
            int total = 0;
            for (String section : StreamSettings.RAIL_SECTIONS) {
                if (!listed(screen, section)) {
                    if (print) System.out.println("ROWS " + section + " not listed");
                    continue;
                }
                StreamSettings.SettingsFragment fragment = open(screen, section);
                List<String> rows = new ArrayList<>();
                PreferenceScreen list = fragment.getPreferenceScreen();
                for (int i = 0; i < list.getPreferenceCount(); i++) {
                    for (String key : keys((PreferenceGroup) list.getPreference(i))) {
                        if (!key.startsWith(StreamSettings.MORE_PREFIX)) rows.add(key);
                    }
                }
                total += rows.size();
                if (print) System.out.println("ROWS " + section + " " + rows.size() + " " + rows);
            }
            if (print) System.out.println("ROWS total " + total);
            org.junit.Assert.assertEquals("rows shown at the defaults", ROWS_AT_DEFAULTS, total);
        }
    }

    /** Whether the list of sections shows it: Debug only on request. */
    private static boolean listed(StreamSettings screen, String section) {
        return !"debug".equals(section) || screen.debugListed();
    }
}
