package com.limelight.preferences;

import android.app.Application;
import android.content.Context;
import android.content.Intent;
import android.os.Looper;
import android.preference.Preference;
import android.preference.PreferenceCategory;
import android.preference.PreferenceFragment;
import android.preference.PreferenceScreen;

import com.google.android.material.appbar.MaterialToolbar;
import com.limelight.R;
import com.limelight.computers.HostCapabilityCache;
import com.limelight.nvstream.http.TestHosts;

import org.junit.After;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.Shadows;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;

import java.util.ArrayList;
import java.util.List;

import static org.junit.Assert.*;

/** The profile list, a profile's values and one game's values, each saying where a value comes from. */
@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class,
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class ProfileScreensTest {
    private final Application app = RuntimeEnvironment.getApplication();

    @After
    public void clear() {
        for (String name : new String[] {StreamProfiles.PREFERENCES, "AppStreamSettings", "HostStreamProfiles"}) {
            app.getSharedPreferences(name, Context.MODE_PRIVATE).edit().clear().commit();
        }
    }

    private static String summary(PreferenceFragment fragment, String key) {
        Preference row = fragment.findPreference(key);
        assertNotNull(key, row);
        return row.getSummary() == null ? "" : row.getSummary().toString();
    }

    private static List<String> groups(PreferenceFragment fragment) {
        PreferenceScreen root = fragment.getPreferenceScreen();
        List<String> titles = new ArrayList<>();
        for (int i = 0; i < root.getPreferenceCount(); i++) {
            if (root.getPreference(i) instanceof PreferenceCategory) titles.add(root.getPreference(i).getTitle().toString());
        }
        return titles;
    }

    private static PreferenceFragment settings(HostStreamSettings screen) {
        return (PreferenceFragment) screen.getFragmentManager().findFragmentById(R.id.host_profile_settings);
    }

    @Test
    public void aGamesValuesSayWhichLayerEachValueComesFrom() {
        HostCapabilityCache.save(app, "pc", TestHosts.rubylight());
        String travel = StreamProfiles.create(app, "Travel", HostStreamProfile.deserialize("v2;hdr=true;pacing=balanced"));
        StreamProfiles.setActive(app, travel);
        HostStreamProfile.deserialize("v2;bitrate=45000").save(app, "pc");
        AppStreamSettings.save(app, "pc", "game", 4, HostStreamProfile.deserialize("v2;refresh=12000"));
        Intent intent = new Intent(app, HostStreamSettings.class).putExtra(HostStreamSettings.EXTRA_MODE, "GAME")
                .putExtra("host_uuid", "pc").putExtra("host_name", "Den PC")
                .putExtra(HostStreamSettings.EXTRA_APP_UUID, "game").putExtra(HostStreamSettings.EXTRA_APP_ID, 4)
                .putExtra(HostStreamSettings.EXTRA_APP_NAME, "Racer");
        ActivityController<HostStreamSettings> controller = Robolectric.buildActivity(HostStreamSettings.class, intent).setup();
        try {
            HostStreamSettings screen = controller.get();
            PreferenceFragment fragment = settings(screen);
            MaterialToolbar toolbar = screen.findViewById(R.id.host_profile_toolbar);
            assertEquals("Racer", toolbar.getTitle().toString());
            assertEquals(app.getString(R.string.game_settings_subtitle, "Den PC"), toolbar.getSubtitle().toString());
            assertEquals("120 " + app.getString(R.string.fps_suffix_fps), summary(fragment, "refresh"));
            assertTrue(summary(fragment, "bitrate").endsWith(app.getString(R.string.layers_from_pc)));
            assertTrue(summary(fragment, "hdr").endsWith(app.getString(R.string.layers_from_profile, "Travel")));
            assertTrue(summary(fragment, "pacing").startsWith(app.getString(R.string.pacing_balanced)));
            assertTrue(summary(fragment, "codec").endsWith(app.getString(R.string.layers_from_main)));
            // Only stream values: no connection or host rows for a game.
            assertFalse(groups(fragment).contains(app.getString(R.string.pc_settings_connection)));
            assertNull(fragment.findPreference("host_display"));

            screen.findViewById(R.id.host_profile_save).performClick();
            assertEquals(Integer.valueOf(12000), AppStreamSettings.load(app, "pc", "game", 4).refreshRateX100);
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void aProfileEditsOnlyItsOwnValuesOverTheMainSettings() {
        String hotel = StreamProfiles.create(app, "Hotel", HostStreamProfile.deserialize("v2;bitrate=8000"));
        // The PC's values sit above a profile, so they don't show on the profile's screen.
        HostStreamProfile.deserialize("v2;hdr=true").save(app, "pc");
        Intent intent = new Intent(app, HostStreamSettings.class).putExtra(HostStreamSettings.EXTRA_MODE, "PROFILE")
                .putExtra(HostStreamSettings.EXTRA_PROFILE_ID, hotel);
        ActivityController<HostStreamSettings> controller = Robolectric.buildActivity(HostStreamSettings.class, intent).setup();
        try {
            HostStreamSettings screen = controller.get();
            PreferenceFragment fragment = settings(screen);
            assertEquals("Hotel", ((MaterialToolbar) screen.findViewById(R.id.host_profile_toolbar)).getTitle().toString());
            assertEquals("8 Mbps", summary(fragment, "bitrate"));
            assertTrue(summary(fragment, "hdr").startsWith(app.getString(R.string.host_profile_off)));
            assertTrue(summary(fragment, "hdr").endsWith(app.getString(R.string.layers_from_main)));
            assertNull(fragment.findPreference("virtual"));
            assertFalse(groups(fragment).contains(app.getString(R.string.pc_settings_host)));
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void aDeletedProfileClosesItsScreen() {
        Intent intent = new Intent(app, HostStreamSettings.class).putExtra(HostStreamSettings.EXTRA_MODE, "PROFILE")
                .putExtra(HostStreamSettings.EXTRA_PROFILE_ID, "42");
        ActivityController<HostStreamSettings> controller = Robolectric.buildActivity(HostStreamSettings.class, intent).create();
        assertTrue(controller.get().isFinishing());
    }

    private void mobileData(boolean has) {
        Shadows.shadowOf(app.getPackageManager()).setSystemFeature(android.content.pm.PackageManager.FEATURE_TELEPHONY, has);
    }

    @Test
    public void withoutMobileDataTheMobileRuleShowsOnlyOnceSet() {
        // A TV or a Wi-Fi tablet streams on Wi-Fi; a mobile data rule set earlier still acts on a metered one.
        mobileData(false);
        String hotel = StreamProfiles.create(app, "Hotel", new HostStreamProfile());
        StreamProfiles.setAuto(app, true);
        ActivityController<StreamProfilesActivity> controller = Robolectric.buildActivity(StreamProfilesActivity.class).setup();
        try {
            Shadows.shadowOf(Looper.getMainLooper()).idle();
            PreferenceFragment fragment = (PreferenceFragment) controller.get().getFragmentManager()
                    .findFragmentById(R.id.page_content);
            assertNull(fragment.findPreference("rule:MOBILE"));
            assertNotNull(fragment.findPreference("rule:HOME_WIFI"));
            assertNotNull(fragment.findPreference("rule:WIFI"));

            StreamProfiles.setRule(app, StreamLayers.Network.MOBILE, hotel);
            controller.pause().resume();
            assertTrue(summary(fragment, "rule:MOBILE").startsWith("Hotel"));
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void theProfileListShowsEachProfileAndWhatAStreamUses() {
        mobileData(true);
        String home = StreamProfiles.create(app, "Home", HostStreamProfile.deserialize("v2;bitrate=80000;hdr=true"));
        StreamProfiles.create(app, "Hotel", new HostStreamProfile());
        StreamProfiles.setActive(app, home);
        ActivityController<StreamProfilesActivity> controller = Robolectric.buildActivity(StreamProfilesActivity.class).setup();
        try {
            Shadows.shadowOf(Looper.getMainLooper()).idle();
            PreferenceFragment fragment = (PreferenceFragment) controller.get().getFragmentManager()
                    .findFragmentById(R.id.page_content);
            String homeRow = summary(fragment, "profile:" + home);
            assertTrue(homeRow, homeRow.startsWith(app.getResources().getQuantityString(R.plurals.profiles_values_count, 2, 2)));
            assertTrue(homeRow.contains(app.getString(R.string.profiles_marker_in_use)));
            assertEquals("Home", summary(fragment, "in_use"));
            // Network choices wait until choosing by network is on.
            assertFalse(fragment.findPreference("rule:MOBILE").isEnabled());
            assertTrue(summary(fragment, "now").endsWith(app.getString(R.string.layers_part_profile, "Home")));
            assertTrue(summary(fragment, "order").startsWith(app.getString(R.string.profiles_order)));

            StreamProfiles.setAuto(app, true);
            controller.pause().resume();
            assertTrue(fragment.findPreference("rule:MOBILE").isEnabled());
            assertTrue(summary(fragment, "rule:MOBILE").startsWith(app.getString(R.string.profiles_rule_default)));
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void theQualityPresetIsReadFromThePcOrMainBitrateAndNotFromTheNetworkProfile() {
        HostCapabilityCache.save(app, "pc", TestHosts.rubylight());
        PreferenceConfiguration.readPreferences(app); // first read migrates the stored defaults
        android.preference.PreferenceManager.getDefaultSharedPreferences(app).edit()
                .putInt(PreferenceConfiguration.BITRATE_PREF_STRING, 20000)
                .putBoolean(PreferenceConfiguration.BITRATE_USER_SET_PREF_STRING, true).commit();
        String travel = StreamProfiles.create(app, "Travel", HostStreamProfile.deserialize("v2;bitrate=90000"));
        StreamProfiles.setActive(app, travel);
        AppStreamSettings.save(app, "pc", "game", 4, HostStreamProfile.deserialize("v2;bitrate=30000"));
        Intent game = new Intent(app, HostStreamSettings.class).putExtra(HostStreamSettings.EXTRA_MODE, "GAME")
                .putExtra("host_uuid", "pc").putExtra("host_name", "Den PC")
                .putExtra(HostStreamSettings.EXTRA_APP_UUID, "game").putExtra(HostStreamSettings.EXTRA_APP_ID, 4)
                .putExtra(HostStreamSettings.EXTRA_APP_NAME, "Racer");
        assertEquals("The profile in use sets 90 Mbps; Quality still reads from the main 20 Mbps",
                app.getString(R.string.game_preset_quality), presetTitle(game));
        StreamProfiles.saveValues(app, travel, HostStreamProfile.deserialize("v2;bitrate=5000"));
        assertEquals("Another network profile value changes nothing",
                app.getString(R.string.game_preset_quality), presetTitle(game));
        HostStreamProfile pc = new HostStreamProfile();
        pc.bitrate = 40000;
        pc.save(app, "pc");
        assertEquals("The PC's own bitrate is the base once it has one",
                app.getString(R.string.game_preset_custom), presetTitle(game));
        AppStreamSettings.save(app, "pc", "game", 4, HostStreamProfile.deserialize("v2;bitrate=60000"));
        assertEquals(app.getString(R.string.game_preset_quality), presetTitle(game));
    }

    private String presetTitle(Intent game) {
        ActivityController<HostStreamSettings> controller = Robolectric.buildActivity(HostStreamSettings.class, game).setup();
        try {
            String summary = summary(settings(controller.get()), "preset");
            return summary.substring(0, summary.indexOf(10));
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void onlyAGameHasPresetsAndItNamesTheOneItsValuesMatch() {
        HostCapabilityCache.save(app, "pc", TestHosts.rubylight());
        AppStreamSettings.save(app, "pc", "game", 4, HostStreamProfile.deserialize("v2;pacing=smoothness;vrr=false"));
        Intent game = new Intent(app, HostStreamSettings.class).putExtra(HostStreamSettings.EXTRA_MODE, "GAME")
                .putExtra("host_uuid", "pc").putExtra("host_name", "Den PC")
                .putExtra(HostStreamSettings.EXTRA_APP_UUID, "game").putExtra(HostStreamSettings.EXTRA_APP_ID, 4)
                .putExtra(HostStreamSettings.EXTRA_APP_NAME, "Racer");
        ActivityController<HostStreamSettings> controller = Robolectric.buildActivity(HostStreamSettings.class, game).setup();
        try {
            assertTrue(summary(settings(controller.get()), "preset").startsWith(app.getString(R.string.game_preset_smoothness)));
        } finally {
            controller.pause().stop().destroy();
        }
        AppStreamSettings.save(app, "pc", "game", 4, HostStreamProfile.deserialize("v2;refresh=12000"));
        controller = Robolectric.buildActivity(HostStreamSettings.class, game).setup();
        try {
            assertTrue(summary(settings(controller.get()), "preset").startsWith(app.getString(R.string.game_preset_custom)));
        } finally {
            controller.pause().stop().destroy();
        }
        String id = StreamProfiles.create(app, "Travel", new HostStreamProfile());
        Intent profile = new Intent(app, HostStreamSettings.class).putExtra(HostStreamSettings.EXTRA_MODE, "PROFILE")
                .putExtra(HostStreamSettings.EXTRA_PROFILE_ID, id);
        controller = Robolectric.buildActivity(HostStreamSettings.class, profile).setup();
        try {
            assertNull(settings(controller.get()).findPreference("preset"));
        } finally {
            controller.pause().stop().destroy();
        }
    }
}
