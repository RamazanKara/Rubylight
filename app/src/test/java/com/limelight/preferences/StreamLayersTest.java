package com.limelight.preferences;

import org.junit.Test;

import java.util.Collections;
import java.util.HashMap;
import java.util.HashSet;
import java.util.Map;
import java.util.Set;

import static com.limelight.preferences.HostStreamProfile.Field;
import static com.limelight.preferences.StreamLayers.Network;
import static com.limelight.preferences.StreamLayers.Problem;
import static com.limelight.preferences.StreamLayers.Reason;
import static com.limelight.preferences.StreamLayers.Source;
import static org.junit.Assert.*;

public class StreamLayersTest {
    private final Map<String, Object> stored = new HashMap<>();

    private void profile(String id, String name, String values) {
        Object order = stored.get(StreamProfiles.ORDER);
        stored.put(StreamProfiles.ORDER, order == null ? id : order + "," + id);
        stored.put(StreamProfiles.NAME + id, name);
        stored.put(StreamProfiles.VALUES + id, values);
    }

    private StreamLayers.Input input(Network network) {
        StreamLayers.Input input = new StreamLayers.Input();
        input.profiles = StreamProfiles.Snapshot.from(stored);
        input.network = network;
        input.mainBitrate = 20000;
        return input;
    }

    @Test
    public void nothingStoredLeavesEveryValueToTheMainSettings() {
        StreamLayers.Result result = StreamLayers.resolve(input(Network.WIFI));
        assertTrue(result.values.isEmpty());
        assertFalse(result.layered());
        assertTrue(result.problems.isEmpty());
        assertNull(result.profileId);
        assertEquals(Reason.NONE, result.reason);
        for (Field field : Field.values()) assertEquals(Source.MAIN, result.source(field));
    }

    @Test
    public void eachValueComesFromTheHighestLayerThatSetsIt() {
        profile("1", "Hotel", "v2;res=1280x720;refresh=6000;bitrate=10000;pacing=balanced");
        stored.put(StreamProfiles.ACTIVE, "1");
        StreamLayers.Input input = input(Network.OTHER);
        input.pcValues = "v2;refresh=12000;bitrate=50000;hdr=true";
        input.gameValues = "v2;bitrate=30000";
        StreamLayers.Result result = StreamLayers.resolve(input);

        assertEquals(Integer.valueOf(1280), result.values.width);
        assertEquals(Source.PROFILE, result.source(Field.RESOLUTION));
        assertEquals("balanced", result.values.pacing);
        assertEquals(Source.PROFILE, result.source(Field.PACING));
        assertEquals(Integer.valueOf(12000), result.values.refreshRateX100);
        assertEquals(Source.PC, result.source(Field.REFRESH));
        assertEquals(Boolean.TRUE, result.values.hdr);
        assertEquals(Source.PC, result.source(Field.HDR));
        assertEquals(Integer.valueOf(30000), result.values.bitrate);
        assertEquals(Source.GAME, result.source(Field.BITRATE));
        assertNull(result.values.codec);
        assertEquals(Source.MAIN, result.source(Field.CODEC));
        assertEquals("Hotel", result.profileName);
        assertEquals(Reason.IN_USE, result.reason);
        assertTrue(result.pcApplied && result.gameApplied);
    }

    @Test
    public void aGameOnlyLayerFallsThroughToThePcAndTheMainSettings() {
        StreamLayers.Input input = input(Network.WIFI);
        input.pcValues = "v2;codec=FORCE_HEVC";
        input.gameValues = "v2;yuv444=true";
        StreamLayers.Result result = StreamLayers.resolve(input);
        assertEquals(PreferenceConfiguration.FormatOption.FORCE_HEVC, result.values.codec);
        assertEquals(Boolean.TRUE, result.values.yuv444);
        assertNull(result.values.bitrate);
        assertEquals(Source.MAIN, result.source(Field.BITRATE));
    }

    @Test
    public void theNetworkPicksItsProfileOnlyWhileChoosingByNetworkIsOn() {
        profile("1", "Home", "v2;bitrate=80000");
        profile("2", "Mobile", "v2;bitrate=8000");
        profile("3", "Travel", "v2;bitrate=15000");
        stored.put(StreamProfiles.ACTIVE, "1");
        stored.put(StreamProfiles.RULE_MOBILE, "2");
        stored.put(StreamProfiles.RULE_WIFI, "3");

        StreamLayers.Result off = StreamLayers.resolve(input(Network.MOBILE));
        assertEquals("1", off.profileId);
        assertEquals(Reason.IN_USE, off.reason);

        stored.put(StreamProfiles.AUTO, true);
        StreamLayers.Result mobile = StreamLayers.resolve(input(Network.MOBILE));
        assertEquals("2", mobile.profileId);
        assertEquals(Reason.NETWORK, mobile.reason);
        assertEquals(Integer.valueOf(8000), mobile.values.bitrate);
        assertEquals("3", StreamLayers.resolve(input(Network.WIFI)).profileId);
        // No choice for home Wi-Fi or Ethernet: the profile in use.
        StreamLayers.Result home = StreamLayers.resolve(input(Network.HOME_WIFI));
        assertEquals("1", home.profileId);
        assertEquals(Reason.IN_USE, home.reason);
        assertEquals("1", StreamLayers.resolve(input(Network.OTHER)).profileId);
    }

    @Test
    public void withoutAProfileInUseAnUnmatchedNetworkUsesTheMainSettings() {
        profile("1", "Mobile", "v2;bitrate=8000");
        stored.put(StreamProfiles.AUTO, true);
        stored.put(StreamProfiles.RULE_MOBILE, "1");
        StreamLayers.Result result = StreamLayers.resolve(input(Network.WIFI));
        assertNull(result.profileId);
        assertTrue(result.problems.isEmpty());
        assertFalse(result.layered());
    }

    @Test
    public void aDeletedProfileNamedByAChoiceFallsBackToTheMainSettingsLoudly() {
        profile("1", "Home", "v2;bitrate=80000");
        stored.put(StreamProfiles.ACTIVE, "1");
        stored.put(StreamProfiles.AUTO, true);
        stored.put(StreamProfiles.RULE_MOBILE, "7");
        StreamLayers.Result result = StreamLayers.resolve(input(Network.MOBILE));
        assertEquals(Collections.singletonList(Problem.PROFILE_MISSING), result.problems);
        assertEquals(Reason.NETWORK, result.reason);
        assertNull(result.profileId);
        assertNull(result.values.bitrate);

        stored.remove(StreamProfiles.RULE_MOBILE);
        stored.put(StreamProfiles.ACTIVE, "9");
        StreamLayers.Result inUse = StreamLayers.resolve(input(Network.MOBILE));
        assertEquals(Collections.singletonList(Problem.PROFILE_MISSING), inUse.problems);
        assertEquals(Reason.IN_USE, inUse.reason);
    }

    @Test
    public void unreadableLayersAreSkippedAndReported() {
        profile("1", "Broken", "v2;bitrate=1");
        stored.put(StreamProfiles.ACTIVE, "1");
        StreamLayers.Input input = input(Network.WIFI);
        input.pcValues = "v2;hdr=maybe";
        input.gameValues = "garbage";
        StreamLayers.Result result = StreamLayers.resolve(input);
        assertTrue(result.problems.contains(Problem.PROFILE_INVALID));
        assertTrue(result.problems.contains(Problem.PC_INVALID));
        assertTrue(result.problems.contains(Problem.GAME_INVALID));
        assertEquals("Broken", result.invalidProfileName);
        assertNull(result.profileId);
        assertFalse(result.layered());

        // One bad layer leaves the others in force.
        input.pcValues = "v2;hdr=true";
        result = StreamLayers.resolve(input);
        assertEquals(Boolean.TRUE, result.values.hdr);
        assertEquals(Source.PC, result.source(Field.HDR));
    }

    @Test
    public void homeWifiWithoutItsNameCountsAsOtherWifiAndSaysSo() {
        profile("1", "Home", "v2;bitrate=80000");
        profile("2", "Away", "v2;bitrate=15000");
        stored.put(StreamProfiles.AUTO, true);
        stored.put(StreamProfiles.RULE_HOME_WIFI, "1");
        stored.put(StreamProfiles.RULE_WIFI, "2");
        stored.put(StreamProfiles.HOME_WIFI, new HashSet<>(Collections.singleton("Living room")));
        StreamLayers.Input input = input(Network.WIFI);
        input.wifiNameKnown = false;
        StreamLayers.Result result = StreamLayers.resolve(input);
        assertEquals(Collections.singletonList(Problem.HOME_WIFI_UNKNOWN), result.problems);
        assertEquals("2", result.profileId);

        // Nothing to warn about while no home Wi-Fi choice exists.
        stored.remove(StreamProfiles.RULE_HOME_WIFI);
        input = input(Network.WIFI);
        input.wifiNameKnown = false;
        assertTrue(StreamLayers.resolve(input).problems.isEmpty());
    }

    @Test
    public void classifiesNetworksWithoutNeedingTheWifiName() {
        Set<String> home = new HashSet<>(Collections.singleton("Living room"));
        assertEquals(Network.MOBILE, StreamLayers.classify(true, false, true, null, home));
        assertEquals(Network.MOBILE, StreamLayers.classify(false, true, true, "Living room", home));
        assertEquals(Network.HOME_WIFI, StreamLayers.classify(false, true, false, "Living room", home));
        assertEquals(Network.WIFI, StreamLayers.classify(false, true, false, "Cafe", home));
        assertEquals(Network.WIFI, StreamLayers.classify(false, true, false, null, home));
        assertEquals(Network.OTHER, StreamLayers.classify(false, false, false, null, home));
        assertEquals(Network.OTHER, StreamLayers.classify(false, false, true, null, home));
    }

    @Test
    public void wifiNamesLoseTheirQuotesAndUnknownNamesAreNull() {
        assertEquals("Living room", StreamLayers.cleanWifiName("\"Living room\""));
        assertEquals("raw", StreamLayers.cleanWifiName("raw"));
        assertNull(StreamLayers.cleanWifiName("<unknown ssid>"));
        assertNull(StreamLayers.cleanWifiName("\"\""));
        assertNull(StreamLayers.cleanWifiName(null));
    }

    @Test
    public void theMobileDataLimitCapsEveryLayerButOnlyOnMobileData() {
        StreamLayers.Input input = input(Network.MOBILE);
        input.mobileLimitKbps = 10000;
        input.gameValues = "v2;bitrate=50000";
        StreamLayers.Result result = StreamLayers.resolve(input);
        assertEquals(Integer.valueOf(10000), result.values.bitrate);
        assertEquals(Source.MOBILE_LIMIT, result.source(Field.BITRATE));
        assertTrue(result.bitrateLimited);

        // The main bitrate is capped too.
        input.gameValues = null;
        result = StreamLayers.resolve(input);
        assertEquals(Integer.valueOf(10000), result.values.bitrate);

        // Below the limit nothing changes.
        input.gameValues = "v2;bitrate=5000";
        result = StreamLayers.resolve(input);
        assertEquals(Integer.valueOf(5000), result.values.bitrate);
        assertEquals(Source.GAME, result.source(Field.BITRATE));
        assertFalse(result.bitrateLimited);

        input.network = Network.WIFI;
        input.gameValues = null;
        assertNull(StreamLayers.resolve(input).values.bitrate);
        input.network = Network.MOBILE;
        input.mobileLimitKbps = 0;
        assertNull(StreamLayers.resolve(input).values.bitrate);
    }

    @Test
    public void theLogDescriptionNamesNoProfileOrNetworkName() {
        profile("1", "Grandma's house", "v2;bitrate=8000");
        stored.put(StreamProfiles.AUTO, true);
        stored.put(StreamProfiles.RULE_HOME_WIFI, "1");
        String text = StreamLayers.describe(StreamLayers.resolve(input(Network.HOME_WIFI)));
        assertEquals("network home_wifi, profile #1 (for network)", text);
        assertFalse(text.contains("Grandma"));
    }
}
