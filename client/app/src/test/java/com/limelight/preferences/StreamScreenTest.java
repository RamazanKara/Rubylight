package com.limelight.preferences;

import org.junit.Test;

import java.util.HashMap;
import java.util.LinkedHashMap;
import java.util.Map;

import static com.limelight.preferences.StreamLayers.Network;
import static com.limelight.preferences.StreamLayers.Reason;
import static org.junit.Assert.*;

/**
 * Per-display profile recall (nightjar#163): profiles stored by 6.4 pick as before, and a profile kept for a screen
 * is picked on that screen by nightjar_policy::display_profile, through the real native library.
 */
public class StreamScreenTest {
    private final Map<String, Object> stored = new HashMap<>();

    private void profile(String id, String name, String values) {
        Object order = stored.get(StreamProfiles.ORDER);
        stored.put(StreamProfiles.ORDER, order == null ? id : order + "," + id);
        stored.put(StreamProfiles.NAME + id, name);
        stored.put(StreamProfiles.VALUES + id, values);
    }

    private StreamLayers.Result resolve(Network network, StreamScreen screen) {
        StreamLayers.Input input = new StreamLayers.Input();
        input.profiles = StreamProfiles.Snapshot.from(stored);
        input.network = network;
        input.mainBitrate = 20000;
        input.screen = screen;
        return StreamLayers.resolve(input);
    }

    /** What 6.4 stored: two profiles, one in use, one chosen for mobile data. */
    private void profilesFrom64() {
        profile("1", "Home", "v2;res=2560x1440;refresh=12000;hdr=true");
        profile("2", "Hotel", "v2;bitrate=8000");
        stored.put(StreamProfiles.ACTIVE, "1");
        stored.put(StreamProfiles.AUTO, true);
        stored.put(StreamProfiles.RULE_MOBILE, "2");
        stored.put(StreamProfiles.NEXT_ID, 3);
    }

    @Test
    public void profilesFrom64PickAsBeforeOnAnyScreen() {
        profilesFrom64();
        assertTrue(StreamProfiles.Snapshot.from(stored).screens.isEmpty());
        for (StreamScreen screen : new StreamScreen[]{null, StreamScreen.builtIn(), StreamScreen.external("HDMI Screen")}) {
            StreamLayers.Result mobile = resolve(Network.MOBILE, screen);
            assertEquals("2", mobile.profileId);
            assertEquals(Reason.NETWORK, mobile.reason);
            StreamLayers.Result wifi = resolve(Network.WIFI, screen);
            assertEquals("1", wifi.profileId);
            assertEquals(Reason.IN_USE, wifi.reason);
        }
    }

    @Test
    public void aScreenCallsUpTheProfileKeptForItAheadOfTheNetwork() {
        profilesFrom64();
        profile("3", "TV", "v2;res=3840x2160;refresh=6000;hdr=true");
        stored.put(StreamProfiles.SCREEN + "3", "external:HDMI Screen");
        stored.put(StreamProfiles.SCREEN_NAME + "3", "HDMI Screen");
        profile("4", "Handheld", "v2;res=1920x1080;refresh=6000;hdr=false");
        stored.put(StreamProfiles.SCREEN + "4", StreamScreen.BUILT_IN);

        StreamLayers.Result tv = resolve(Network.MOBILE, StreamScreen.external("hdmi screen"));
        assertEquals("3", tv.profileId);
        assertEquals(Reason.SCREEN, tv.reason);
        assertEquals(Integer.valueOf(3840), tv.values.width);
        assertEquals("network mobile, profile #3 (for screen)", StreamLayers.describe(tv));
        // The stream keeps the screen in its intent with the network, and a restart resolves with both.
        assertEquals("external:hdmi screen", tv.screen.id);
        assertEquals("3", resolve(Network.MOBILE, tv.screen).profileId);

        StreamLayers.Result own = resolve(Network.WIFI, StreamScreen.builtIn());
        assertEquals("4", own.profileId);
        assertEquals(Boolean.FALSE, own.values.hdr);

        // Another connected display, or a screen Android can't name: the network choice and the profile in use.
        assertEquals("2", resolve(Network.MOBILE, StreamScreen.external("LG TV")).profileId);
        assertEquals("1", resolve(Network.WIFI, null).profileId);
    }

    @Test
    public void aProfileKeptForAScreenOfAMissingProfileIsNotRead() {
        profilesFrom64();
        stored.put(StreamProfiles.SCREEN + "9", StreamScreen.BUILT_IN);
        assertTrue(StreamProfiles.Snapshot.from(stored).screens.isEmpty());
        assertEquals("1", resolve(Network.WIFI, StreamScreen.builtIn()).profileId);
    }

    @Test
    public void theRequestIsTheOneTheNativeSideReads() {
        Map<String, StreamScreen> screens = new LinkedHashMap<>();
        screens.put("2", StreamScreen.builtIn());
        screens.put("5", StreamScreen.external("HDMI Screen"));
        // The same request protocol/android/src/display_profile.rs answers in its test.
        assertEquals("{\"rules\":[{\"profile\":\"2\",\"display\":{\"id\":\"builtin\",\"name\":\"\"}},"
                        + "{\"profile\":\"5\",\"display\":{\"id\":\"external:HDMI Screen\",\"name\":\"HDMI Screen\"}}],"
                        + "\"screen\":{\"id\":\"external:HDMI Screen\",\"name\":\"HDMI Screen\"}}",
                StreamLayers.screenRequest(screens, StreamScreen.external("HDMI Screen")));
        assertEquals("\"a\\\"b\\\\c\\u0001\"", StreamLayers.quote("a\"b\\c\u0001"));
        assertEquals("5", StreamLayers.profileOf("{\"profile\":\"5\"}"));
        assertNull(StreamLayers.profileOf("{\"profile\":null}"));
        assertNull(StreamLayers.profileOf("{\"error\":\"invalid\",\"detail\":\"x\"}"));
        assertNull(StreamLayers.profileOf(null));
        assertNull(StreamScreen.external("  "));
    }
}
