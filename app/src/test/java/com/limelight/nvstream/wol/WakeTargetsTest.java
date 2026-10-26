package com.limelight.nvstream.wol;

import static com.limelight.nativeapi.NativeApiFixtures.error;
import static com.limelight.nativeapi.NativeApiFixtures.ok;
import static org.junit.Assert.assertArrayEquals;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertSame;

import android.app.Application;

import com.limelight.nativeapi.HostEndpoint;
import com.limelight.nativeapi.NativeApiClient;
import com.limelight.nativeapi.NativeApiFixtures;

import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;

import java.util.Arrays;
import java.util.Collections;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class WakeTargetsTest {
    /** The host contract's example (docs/native-api.md), with a second, Wi-Fi adapter. */
    static final String WAKE = "{\"revision\":1,\"primary\":\"02:7C:16:B4:13:4F\",\"targets\":["
            + "{\"mac\":\"02:7c:16:b4:13:4f\",\"kind\":\"ethernet\",\"connected\":true,\"route\":true,"
            + "\"broadcasts\":[\"192.168.4.255\"],\"state\":\"ready\",\"checks\":[{\"id\":\"wake_enabled\",\"state\":\"ok\"}]},"
            + "{\"mac\":\"02-e0-4c-68-00-01\",\"kind\":\"wifi\",\"connected\":true,\"route\":false,"
            + "\"broadcasts\":[\"10.0.0.255\",\"192.168.4.255\"],\"state\":\"experimental\",\"checks\":[],\"future\":1}],"
            + "\"system\":{\"sleep\":\"s3\"},\"idle_sleep\":{\"state\":\"off\",\"minutes\":0,\"reason\":null}}";

    private final NativeApiFixtures.FakeTransport transport = new NativeApiFixtures.FakeTransport();
    private WakeTargets targets;

    @Before
    public void setUp() {
        targets = new WakeTargets(RuntimeEnvironment.getApplication());
    }

    private NativeApiClient client() {
        return new NativeApiClient(transport, HostEndpoint.of(NativeApiFixtures.nativeHost("pc", "PC", true), "unique",
                NativeApiFixtures.crypto()));
    }

    private static String capabilities(String... features) {
        StringBuilder list = new StringBuilder();
        for (String feature : features) {
            list.append(list.length() == 0 ? "" : ",").append('"').append(feature).append('"');
        }
        return ok("{\"api_version\":1,\"features\":[" + list + "],"
                + "\"permissions\":{\"list\":true,\"view\":true,\"launch\":true,\"host_control\":false}}");
    }

    @Test
    public void everyAdapterAndBroadcastIsKeptOnceAndSurvivesARestart() throws Exception {
        transport.reply("pc", "wake", ok(WAKE));
        targets.store("pc", client().wake());

        WakeTargets.Targets stored = new WakeTargets(RuntimeEnvironment.getApplication()).load("pc");
        assertEquals(Arrays.asList("02:7c:16:b4:13:4f", "02:e0:4c:68:00:01"), stored.macs);
        assertEquals(Arrays.asList("192.168.4.255", "10.0.0.255"), stored.broadcasts);
        assertSame(WakeTargets.Targets.NONE, targets.load("other"));
        assertSame(WakeTargets.Targets.NONE, targets.load(null));
    }

    @Test
    public void malformedZeroAndNamedTargetsAreLeftOut() throws Exception {
        transport.reply("pc", "wake", ok("{\"revision\":1,\"primary\":null,\"targets\":["
                + "{\"mac\":\"00:00:00:00:00:00\",\"broadcasts\":[\"pc.example\",\"192.168.4.256\",\"1.2.3\"]},"
                + "{\"mac\":\"02:7c:16:b4:13\",\"broadcasts\":[\"192.168.004.255\"]},"
                + "{\"mac\":7,\"broadcasts\":[\"10.1.1.255\"]},"
                + "\"not a target\","
                + "{\"mac\":\"02:7c:16:b4:13:4f\",\"broadcasts\":[5,\"10.0.0.255\"]}]}"));
        targets.store("pc", client().wake());

        WakeTargets.Targets stored = targets.load("pc");
        assertEquals(Collections.singletonList("02:7c:16:b4:13:4f"), stored.macs);
        // A leading zero still reads as a number; names would need DNS and are dropped.
        assertEquals(Arrays.asList("192.168.4.255", "10.0.0.255"), stored.broadcasts);
        assertNull(WakeTargets.ipv4("pc.example"));
        assertArrayEquals(new byte[] {(byte) 192, (byte) 168, 4, (byte) 255}, WakeTargets.ipv4("192.168.4.255"));
    }

    @Test
    public void aHostListsAtMostAPcsWorthOfTargets() throws Exception {
        StringBuilder many = new StringBuilder();
        for (int i = 0; i < 40; i++) {
            many.append(i == 0 ? "" : ",").append(String.format("{\"mac\":\"02:00:00:00:00:%02x\",\"broadcasts\":"
                    + "[\"10.0.%d.255\"]}", i, i));
        }
        transport.reply("pc", "wake", ok("{\"revision\":1,\"targets\":[" + many + "]}"));
        targets.store("pc", client().wake());

        assertEquals(WakeTargets.MAX_MACS, targets.load("pc").macs.size());
        assertEquals(WakeTargets.MAX_BROADCASTS, targets.load("pc").broadcasts.size());
    }

    @Test
    public void aRefreshReadsTheTargetsOnlyFromAHostThatListsWake() throws Exception {
        transport.reply("pc", "capabilities", capabilities("wake"));
        transport.reply("pc", "wake", ok(WAKE));
        NativeApiClient client = client();
        targets.refresh("pc", client.capabilities(), client);
        assertEquals(2, targets.load("pc").macs.size());

        // The host lost the feature (an older host build): its old targets go, serverinfo's MAC is all that is left.
        transport.calls.clear();
        transport.reply("pc", "capabilities", capabilities("session.couch_menu"));
        targets.refresh("pc", client.capabilities(), client);
        assertSame(WakeTargets.Targets.NONE, targets.load("pc"));
        assertEquals(Collections.singletonList("pc|capabilities"), transport.calls);
    }

    @Test
    public void aFailedReadKeepsTheStoredTargets() throws Exception {
        transport.reply("pc", "wake", ok(WAKE));
        targets.store("pc", client().wake());

        transport.reply("pc", "capabilities", capabilities("wake"));
        transport.reply("pc", "wake", error("network", "down"));
        NativeApiClient client = client();
        targets.refresh("pc", client.capabilities(), client);
        assertEquals(2, targets.load("pc").macs.size());

        targets.forget("pc");
        assertSame(WakeTargets.Targets.NONE, targets.load("pc"));
    }
}
