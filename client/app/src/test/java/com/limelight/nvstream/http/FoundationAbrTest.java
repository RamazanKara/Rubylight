package com.limelight.nvstream.http;

import android.app.Application;

import org.json.JSONObject;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;

import java.io.IOException;

import static org.junit.Assert.*;

// Foundation Sunshine's /api/abr shapes (src/nvhttp/abr_api.cpp), as Moonlight V+ sends them;
// the same as protocol/client/src/http/foundation.rs on desktop.
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class FoundationAbrTest {
    @Test
    public void requestsAreVPlusBodies() throws Exception {
        JSONObject on = FoundationAbr.enableRequest(40000);
        assertEquals(4, on.length());
        assertTrue(on.getBoolean("enabled"));
        assertEquals("balanced", on.getString("mode"));
        assertEquals(0, on.getInt("minBitrate"));
        assertEquals(40000, on.getInt("maxBitrate"));
        JSONObject off = FoundationAbr.disableRequest();
        assertEquals(1, off.length());
        assertFalse(off.getBoolean("enabled"));
    }

    @Test
    public void answersReadAsFoundationWritesThem() throws Exception {
        assertTrue(FoundationAbr.supported("{\"supported\":true,\"version\":1,\"features\":[\"llm_ai\"],\"llmEnabled\":false,\"hostMaxBitrate\":80000}"));
        assertFalse(FoundationAbr.supported("{\"supported\":false}"));
        assertEquals(35000, FoundationAbr.initialBitrate("{\"success\":true,\"enabled\":true,\"mode\":\"balanced\",\"minBitrate\":0," +
                "\"maxBitrate\":40000,\"initialBitrate\":35000,\"bitrateApplied\":true}"));
        IOException refused = assertThrows(IOException.class, () ->
                FoundationAbr.initialBitrate("{\"success\":false,\"error\":\"No active streaming session for this client\"}"));
        assertEquals("No active streaming session for this client", refused.getMessage());
        assertThrows(IOException.class, () -> FoundationAbr.supported("not json"));
    }

    // Foundation's network_probe.cpp capabilities, validated as V+ Android does.
    @Test
    public void theBandwidthTestSizeFollowsThePcsCapabilities() throws Exception {
        assertEquals(4 * 1024 * 1024, NvHTTP.foundationProbeBytes("{\"version\":1,\"features\":[\"bandwidth-probe-v1\"]," +
                "\"bandwidthProbe\":{\"version\":1,\"endpoint\":\"/api/network/probe\",\"minBytes\":262144,\"maxBytes\":8388608,\"cooldownMs\":10000}}"));
        assertEquals(2000, NvHTTP.foundationProbeBytes("{\"version\":1,\"bandwidthProbe\":{\"version\":1," +
                "\"endpoint\":\"/api/network/probe\",\"minBytes\":1000,\"maxBytes\":2000}}"));
        for (String bad : new String[] {
                "{\"version\":1}",
                "{\"version\":0,\"bandwidthProbe\":{\"version\":1,\"endpoint\":\"/api/network/probe\",\"minBytes\":1,\"maxBytes\":2}}",
                "{\"version\":1,\"bandwidthProbe\":{\"version\":1,\"endpoint\":\"/elsewhere\",\"minBytes\":1,\"maxBytes\":2}}",
                "{\"version\":1,\"bandwidthProbe\":{\"version\":1,\"endpoint\":\"/api/network/probe\",\"minBytes\":3,\"maxBytes\":2}}",
                "{\"version\":1,\"bandwidthProbe\":{\"version\":1,\"endpoint\":\"/api/network/probe\",\"minBytes\":8388608,\"maxBytes\":9000000}}",
        }) {
            assertThrows(bad, IOException.class, () -> NvHTTP.foundationProbeBytes(bad));
        }
    }

    @Test
    public void reportsGoEveryThreeSecondsOneAtATime() throws Exception {
        FoundationAbr abr = new FoundationAbr(35000, 50000, 1000);
        assertEquals(35000, abr.currentKbps());
        assertEquals(50000, new FoundationAbr(0, 50000, 0).currentKbps());
        for (int i = 0; i < 3; i++) abr.add(2.5, 117);
        assertNull("not before three seconds", abr.due(3999, 8));
        JSONObject report = abr.due(4000, 8);
        assertEquals(5, report.length());
        assertEquals(2.5, report.getDouble("packetLoss"), 1e-9);
        assertEquals(8.0, report.getDouble("rttMs"), 1e-9);
        assertEquals(117.0, report.getDouble("decodeFps"), 1e-9);
        assertEquals(9, report.getInt("droppedFrames"));
        assertEquals(35000, report.getInt("currentBitrate"));
        abr.add(0, 120);
        assertNull("the last report is not answered yet", abr.due(8000, 0));
        assertEquals(28000, abr.answered(new JSONObject("{\"newBitrate\":28000,\"bitrateApplied\":true,\"reason\":\"loss\"}")));
        JSONObject next = abr.due(8000, 0);
        assertEquals(28000, next.getInt("currentBitrate"));
        assertEquals(0.0, next.getDouble("packetLoss"), 1e-9);
        assertEquals(0, abr.answered(new JSONObject("{\"bitrateApplied\":true,\"reason\":\"stable\"}")));
        assertEquals("a failed report changes nothing", 0, abr.answered(null));
        assertEquals(28000, abr.currentKbps());
    }
}
