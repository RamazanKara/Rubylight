package com.limelight.preferences;

import android.app.Application;
import android.content.Context;
import android.content.SharedPreferences;
import android.preference.PreferenceManager;

import org.junit.After;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;

import java.util.Arrays;
import java.util.Collections;
import java.util.HashMap;
import java.util.HashSet;
import java.util.Map;

import static org.junit.Assert.*;

@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class,
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class StreamProfilesTest {
    private final Application app = RuntimeEnvironment.getApplication();

    @After
    public void clear() {
        for (String name : new String[] {StreamProfiles.PREFERENCES, "AppStreamSettings", "HostStreamProfiles"}) {
            app.getSharedPreferences(name, Context.MODE_PRIVATE).edit().clear().commit();
        }
        PreferenceManager.getDefaultSharedPreferences(app).edit().clear().commit();
    }

    private static HostStreamProfile values(String serialized) {
        return HostStreamProfile.deserialize(serialized);
    }

    @Test
    public void profilesKeepTheirOrderNamesAndValues() {
        String home = StreamProfiles.create(app, " Home 4K120 ", values("v2;res=3840x2160;refresh=12000"));
        String hotel = StreamProfiles.create(app, "Hotel", values("v2;bitrate=10000;pacing=smoothness"));
        StreamProfiles.Snapshot profiles = StreamProfiles.load(app);
        assertEquals(Arrays.asList(home, hotel), Arrays.asList(profiles.names.keySet().toArray()));
        assertEquals("Home 4K120", profiles.name(home));
        assertEquals("smoothness", profiles.valuesOrEmpty(hotel).pacing);

        StreamProfiles.rename(app, hotel, "Travel");
        StreamProfiles.saveValues(app, hotel, values("v2;bitrate=8000"));
        profiles = StreamProfiles.load(app);
        assertEquals("Travel", profiles.name(hotel));
        assertEquals(Integer.valueOf(8000), profiles.valuesOrEmpty(hotel).bitrate);
        assertNull(profiles.valuesOrEmpty(hotel).pacing);
    }

    @Test
    public void deletingAProfileClearsEveryChoiceThatNamesIt() {
        String home = StreamProfiles.create(app, "Home", new HostStreamProfile());
        String mobile = StreamProfiles.create(app, "Mobile", values("v2;bitrate=5000"));
        StreamProfiles.setActive(app, mobile);
        StreamProfiles.setAuto(app, true);
        StreamProfiles.setRule(app, StreamLayers.Network.MOBILE, mobile);
        StreamProfiles.setRule(app, StreamLayers.Network.HOME_WIFI, home);
        StreamProfiles.delete(app, mobile);

        StreamProfiles.Snapshot profiles = StreamProfiles.load(app);
        assertFalse(profiles.exists(mobile));
        assertNull(profiles.activeId);
        assertNull(profiles.rule(StreamLayers.Network.MOBILE));
        assertEquals(home, profiles.rule(StreamLayers.Network.HOME_WIFI));
        assertEquals(Collections.singleton(home), profiles.names.keySet());
        assertNull(profiles.rawValues(mobile));
        // A new profile never reuses the deleted one's ID, so old references can't point at it.
        String next = StreamProfiles.create(app, "Next", new HostStreamProfile());
        assertNotEquals(mobile, next);
    }

    @Test
    public void namesMustBeFilledShortAndUnique() {
        StreamProfiles.create(app, "Hotel", new HostStreamProfile());
        StreamProfiles.Snapshot profiles = StreamProfiles.load(app);
        String hotel = profiles.names.keySet().iterator().next();
        assertEquals(StreamProfiles.NameProblem.EMPTY, profiles.checkName("  ", null));
        assertEquals(StreamProfiles.NameProblem.TAKEN, profiles.checkName(" hotel ", null));
        assertNull(profiles.checkName("HOTEL", hotel));
        assertEquals(StreamProfiles.NameProblem.TOO_LONG, profiles.checkName("x".repeat(StreamProfiles.MAX_NAME + 1), null));
        assertNull(profiles.checkName("x".repeat(StreamProfiles.MAX_NAME), null));
        assertEquals("Hotel 2", profiles.freeName("Hotel"));
        assertEquals("Lake", profiles.freeName("Lake"));
        String longName = "y".repeat(StreamProfiles.MAX_NAME + 5);
        assertEquals(StreamProfiles.MAX_NAME, profiles.freeName(longName).length());
    }

    @Test
    public void unreadableOrPartialStoresAreReadAsFarAsTheyGo() {
        Map<String, Object> stored = new HashMap<>();
        stored.put(StreamProfiles.ORDER, "1,2,1,3");
        stored.put(StreamProfiles.NAME + "1", "One");
        stored.put(StreamProfiles.VALUES + "1", "v2;hdr=maybe");
        stored.put(StreamProfiles.NAME + "2", "Two");
        stored.put(StreamProfiles.ACTIVE, "");
        stored.put(StreamProfiles.HOME_WIFI, new HashSet<>(Arrays.asList("Home", "Attic")));
        StreamProfiles.Snapshot profiles = StreamProfiles.Snapshot.from(stored);
        assertEquals(Arrays.asList("1", "2"), Arrays.asList(profiles.names.keySet().toArray()));
        assertTrue(profiles.valuesOrEmpty("1").isEmpty());
        assertEquals("v2", profiles.rawValues("2"));
        assertNull(profiles.activeId);
        assertFalse(profiles.auto);
        assertEquals(new HashSet<>(Arrays.asList("Home", "Attic")), profiles.homeWifi);
    }

    @Test
    public void aGameIsStoredPerPcUnderItsUuidAndId() {
        AppStreamSettings.save(app, "pc1", "game", 7, values("v2;bitrate=30000"));
        assertEquals(Integer.valueOf(30000), AppStreamSettings.load(app, "pc1", "game", 7).bitrate);
        assertEquals(Integer.valueOf(30000), AppStreamSettings.load(app, "pc1", null, 7).bitrate);
        // A known UUID decides alone, as IDs can be reused for another game.
        assertTrue(AppStreamSettings.load(app, "pc1", "other", 7).isEmpty());
        assertTrue(AppStreamSettings.load(app, "pc2", "game", 7).isEmpty());
        AppStreamSettings.save(app, "pc2", "game", 7, values("v2;hdr=true"));
        AppStreamSettings.forget(app, "pc1");
        assertTrue(AppStreamSettings.load(app, "pc1", "game", 7).isEmpty());
        assertEquals(Boolean.TRUE, AppStreamSettings.load(app, "pc2", "game", 7).hdr);
        AppStreamSettings.save(app, "pc2", "game", 7, new HostStreamProfile());
        assertNull(AppStreamSettings.raw(app, "pc2", "game", 7));
        assertTrue(app.getSharedPreferences("AppStreamSettings", Context.MODE_PRIVATE).getAll().isEmpty());
    }

    @Test
    public void anUpgradeWithPcValuesStreamsExactlyAsBeforeAndWritesNothingNew() {
        SharedPreferences main = PreferenceManager.getDefaultSharedPreferences(app);
        main.edit().putString("list_resolution", "1920x1080").putString("list_fps", "60")
                .putInt("seekbar_bitrate_kbps", 20000).putBoolean("bitrate_user_set", true)
                .putBoolean(PreferenceConfiguration.RESOLUTION_FOLLOWS_SCREEN_PREF_STRING, false).commit();
        String pcValues = "v2;res=2560x1440;refresh=12000;bitrate=45000;codec=FORCE_HEVC;hdr=true";
        app.getSharedPreferences("HostStreamProfiles", Context.MODE_PRIVATE).edit().putString("pc", pcValues).commit();
        PreferenceConfiguration.readPreferences(app);
        PreferenceConfiguration.migratePcProfilesToOverrides(app);
        Map<String, ?> before = new HashMap<>(main.getAll());

        PreferenceConfiguration expected = PreferenceConfiguration.readPreferences(app);
        HostStreamProfile.deserialize(pcValues).applyTo(expected);
        PreferenceConfiguration config = PreferenceConfiguration.readPreferences(app, "pc",
                com.limelight.nvstream.http.HostCapabilities.UNKNOWN, "game", 3, StreamLayers.Network.MOBILE);

        assertEquals(expected.width, config.width);
        assertEquals(expected.height, config.height);
        assertEquals(expected.fps, config.fps);
        assertEquals(expected.launchRefreshRateX100, config.launchRefreshRateX100);
        assertEquals(expected.bitrate, config.bitrate);
        assertEquals(expected.videoFormat, config.videoFormat);
        assertEquals(expected.enableHdr, config.enableHdr);
        assertEquals(expected.framePacing, config.framePacing);
        assertTrue(config.fromPcProfile);
        assertEquals(pcValues, config.pcSettings);
        assertTrue(config.layers.problems.isEmpty());
        assertNull(config.layers.profileId);
        assertEquals(pcValues, app.getSharedPreferences("HostStreamProfiles", Context.MODE_PRIVATE).getString("pc", null));
        assertEquals(before, main.getAll());
        assertTrue(app.getSharedPreferences(StreamProfiles.PREFERENCES, Context.MODE_PRIVATE).getAll().isEmpty());
        assertTrue(app.getSharedPreferences("AppStreamSettings", Context.MODE_PRIVATE).getAll().isEmpty());
    }

    @Test
    public void aStreamGetsTheProfileTheGameAndTheMobileLimit() {
        PreferenceConfiguration.readPreferences(app);
        PreferenceManager.getDefaultSharedPreferences(app).edit().putInt("seekbar_bitrate_kbps", 40000)
                .putBoolean("bitrate_user_set", true).putString(PreferenceConfiguration.MOBILE_BITRATE_PREF_STRING, "15000")
                .putString("frame_pacing", "latency").commit();
        String travel = StreamProfiles.create(app, "Travel", values("v2;pacing=balanced;hdr=true"));
        StreamProfiles.setAuto(app, true);
        StreamProfiles.setRule(app, StreamLayers.Network.MOBILE, travel);
        AppStreamSettings.save(app, "pc", "game", 3, values("v2;hdr=false"));

        PreferenceConfiguration mobile = PreferenceConfiguration.readPreferences(app, "pc",
                com.limelight.nvstream.http.HostCapabilities.UNKNOWN, "game", 3, StreamLayers.Network.MOBILE);
        assertEquals(15000, mobile.bitrate);
        assertEquals(PreferenceConfiguration.FRAME_PACING_BALANCED, mobile.framePacing);
        assertFalse(mobile.enableHdr);
        assertEquals("Travel", mobile.layers.profileName);

        PreferenceConfiguration wifi = PreferenceConfiguration.readPreferences(app, "pc",
                com.limelight.nvstream.http.HostCapabilities.UNKNOWN, "game", 3, StreamLayers.Network.WIFI);
        assertEquals(40000, wifi.bitrate);
        assertEquals(PreferenceConfiguration.FRAME_PACING_MIN_LATENCY, wifi.framePacing);
        assertNull(wifi.layers.profileId);
    }

    // The bitrate is automatic, and a new stream may be link-tested, only when nobody chose it:
    // not in the main settings, a profile, this PC or this game. The mobile data limit is not a choice.
    @Test
    public void theBitrateIsAutomaticUntilALayerOrTheUserSetsIt() {
        PreferenceConfiguration.readPreferences(app);
        SharedPreferences main = PreferenceManager.getDefaultSharedPreferences(app);
        main.edit().putString(PreferenceConfiguration.MOBILE_BITRATE_PREF_STRING, "15000").commit();
        assertTrue(PreferenceConfiguration.readPreferences(app).automaticBitrate);
        assertTrue(PreferenceConfiguration.readPreferences(app, "pc",
                com.limelight.nvstream.http.HostCapabilities.UNKNOWN, "game", 3, StreamLayers.Network.MOBILE).automaticBitrate);

        AppStreamSettings.save(app, "pc", "game", 3, values("v2;bitrate=30000"));
        assertFalse(PreferenceConfiguration.readPreferences(app, "pc",
                com.limelight.nvstream.http.HostCapabilities.UNKNOWN, "game", 3, StreamLayers.Network.WIFI).automaticBitrate);
        assertTrue("another game", PreferenceConfiguration.readPreferences(app, "pc",
                com.limelight.nvstream.http.HostCapabilities.UNKNOWN, "other", 4, StreamLayers.Network.WIFI).automaticBitrate);

        main.edit().putInt("seekbar_bitrate_kbps", 40000).putBoolean("bitrate_user_set", true).commit();
        assertFalse(PreferenceConfiguration.readPreferences(app).automaticBitrate);
    }

    @Test
    public void aMobileLimitOutsideTheChoicesIsReadSafely() {
        assertEquals(0, PreferenceConfiguration.mobileBitrateLimit("0"));
        assertEquals(0, PreferenceConfiguration.mobileBitrateLimit("junk"));
        assertEquals(0, PreferenceConfiguration.mobileBitrateLimit("100"));
        assertEquals(5000, PreferenceConfiguration.mobileBitrateLimit("5000"));
        assertEquals(HostStreamProfile.MAX_BITRATE_KBPS, PreferenceConfiguration.mobileBitrateLimit("99999999"));
    }
}
