package com.limelight.computers;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;
import static org.junit.Assert.fail;

import android.app.Application;
import android.content.Context;

import org.robolectric.RuntimeEnvironment;

import com.limelight.nvstream.http.ComputerDetails;

import org.json.JSONObject;
import org.junit.After;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;

import java.util.ArrayList;
import java.util.List;

/**
 * What {@link InternetPlay} asks libnightjar_core (nightjar-core internet.rs `json::call`, whose answers the Rust
 * tests check) and how it reads the answers.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class InternetPlayTest {
    private final List<JSONObject> asked = new ArrayList<>();

    @After
    public void forget() {
        InternetPlay.use(null);
    }

    private void answer(String answer) {
        InternetPlay.use(request -> {
            try {
                asked.add(new JSONObject(request));
            } catch (Exception e) {
                throw new AssertionError(e);
            }
            return answer;
        });
    }

    @Test
    public void theInternetAddressesComeAfterThePcsOwnInOrder() throws Exception {
        answer("{\"ok\":{\"ipv6\":[\"2001:db8::5\"],\"mapped\":\"203.0.113.5\",\"port\":47989}}");
        InternetPlay.Addresses addresses = InternetPlay.addresses("{\"version\":1}");
        assertEquals("addresses", asked.get(0).getString("op"));
        assertEquals("{\"version\":1}", asked.get(0).getString("reply"));
        assertEquals(new ComputerDetails.AddressTuple("2001:db8::5", 47989), addresses.ipv6.get(0));
        assertEquals(new ComputerDetails.AddressTuple("203.0.113.5", 47989), addresses.mapped);
        // A PC that never gave a reply has no internet addresses, and nothing is asked.
        asked.clear();
        assertTrue(InternetPlay.addresses(null).ipv6.isEmpty());
        assertTrue(asked.isEmpty());
    }

    @Test
    public void connectGivesTheTunnelsLoopbackEndOrTheWordsForEveryPath() throws Exception {
        InternetPlay.Tried tried = new InternetPlay.Tried();
        tried.lan = true;
        tried.mapped = true;
        answer("{\"ok\":{\"address\":\"127.78.10.20\",\"port\":47989}}");
        ComputerDetails.AddressTuple tunnel = InternetPlay.connect("uuid", "{}", tried);
        assertEquals("127.78.10.20", tunnel.address);
        assertTrue(InternetPlay.isTunnelAddress(tunnel.address));
        JSONObject request = asked.get(0);
        assertEquals("connect", request.getString("op"));
        assertEquals("uuid", request.getString("host"));
        assertTrue(request.getJSONObject("tried").getBoolean("lan"));
        assertFalse(request.getJSONObject("tried").getBoolean("ipv6"));
        answer("{\"error\":\"The PC could not be reached. Tried the local network: no answer.\"}");
        try {
            InternetPlay.connect("uuid", "{}", tried);
            fail("no tunnel came up");
        } catch (InternetPlay.Unreachable e) {
            assertTrue(e.getMessage().startsWith("The PC could not be reached."));
        }
    }

    @Test
    public void thePathInUseIsNamedAndTheLocalNetworkNeedsNoWords() throws Exception {
        Context context = RuntimeEnvironment.getApplication();
        ComputerDetails details = new ComputerDetails();
        details.localAddress = new ComputerDetails.AddressTuple("192.168.1.10", 47989);
        details.remoteAddress = new ComputerDetails.AddressTuple("203.0.113.5", 47989);
        details.activeAddress = details.remoteAddress;
        // Without a reply naming it, the remote address is still a forwarded port.
        answer("{\"ok\":\"lan\"}");
        assertEquals("mapped", InternetPlay.path(details, null));
        assertEquals("192.168.1.10", asked.get(0).getJSONArray("lan").getString(0));
        assertEquals("Internet (forwarded port)", InternetPlay.pathLabel(context, "mapped"));
        assertEquals("Internet (direct path)", InternetPlay.pathLabel(context, "punched"));
        assertNull(InternetPlay.pathLabel(context, "lan"));
        assertFalse(InternetPlay.isTunnelAddress("127.0.0.1"));
    }

    @Test
    public void theKeptReplyIsPerPcAndGoesWithIt() {
        Context context = RuntimeEnvironment.getApplication();
        answer("{\"ok\":null}");
        InternetPlay.save(context, "a", "{\"version\":1}");
        assertEquals("{\"version\":1}", InternetPlay.reply(context, "a"));
        assertNull(InternetPlay.reply(context, "b"));
        InternetPlay.forget(context, "a");
        assertNull(InternetPlay.reply(context, "a"));
        assertTrue(InternetPlay.due(null, 0));
        assertFalse(InternetPlay.due(0L, InternetPlay.READ_INTERVAL_MS - 1));
        assertTrue(InternetPlay.due(0L, InternetPlay.READ_INTERVAL_MS));
    }
}
