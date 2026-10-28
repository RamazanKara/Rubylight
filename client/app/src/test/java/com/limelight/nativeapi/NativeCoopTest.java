package com.limelight.nativeapi;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertSame;
import static org.junit.Assert.assertTrue;
import static org.junit.Assert.fail;

import com.limelight.nvstream.http.ComputerDetails;

import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;

import java.util.Arrays;

/** GET /coop and POST /coop/join as native_api.rs envelopes them from coop.rs, and their problems. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = android.app.Application.class)
public class NativeCoopTest {
    private final NativeApiFixtures.FakeTransport transport = new NativeApiFixtures.FakeTransport();

    /** One session in slot order: Retroid is this device, slot 2 is free, and the host can take more. */
    public static final String LISTING = "{\"available\":true,\"reason\":null,\"games\":[{\"app_id\":2147483647,"
            + "\"title\":\"Co-op (Nucleus): Portal 2\",\"game\":\"Portal 2\",\"max_players\":4}],"
            + "\"sessions\":[{\"id\":3000000000,\"app_id\":2147483647,\"title\":\"Portal 2\",\"game\":\"Portal 2\","
            + "\"phase\":\"running\",\"end\":null,\"players\":[{\"slot\":0,\"name\":\"Laptop\",\"you\":false,"
            + "\"away\":false},{\"slot\":1,\"name\":\"Retroid\",\"you\":true,\"away\":true},{\"slot\":2,"
            + "\"name\":null,\"you\":false,\"away\":false}],\"free_slots\":1,\"joinable\":true,\"refusal\":null}]}";

    private NativeApiClient client() {
        ComputerDetails host = NativeApiFixtures.nativeHost("pc", "PC", true);
        return new NativeApiClient(transport, HostEndpoint.of(host, "unique", NativeApiFixtures.crypto()));
    }

    private static NativeApiException error(String code, int status, String message) {
        return new NativeApiException(NativeApiException.Kind.API, message, status, code, null, 0);
    }

    @Before
    public void forget() {
        KnownCapabilities.clear();
    }

    @Test
    public void theListingCarriesSessionsPlayersAndTheSeat() throws Exception {
        transport.reply("pc", "coop", NativeApiFixtures.ok(LISTING));
        NativeCoop coop = client().coop();
        assertTrue(coop.available);
        assertNull(coop.reason);
        assertEquals(1, coop.sessions.size());
        NativeCoop.Session session = coop.sessions.get(0);
        // Session ids are u32 on the wire; the bits survive the int.
        assertEquals(3000000000L, Integer.toUnsignedLong(session.id));
        assertEquals(Integer.MAX_VALUE, session.appId);
        assertEquals("Portal 2", session.title);
        assertSame(NativeCoop.Phase.RUNNING, session.phase);
        assertNull(session.endReason);
        assertEquals(3, session.players.size());
        assertEquals("Retroid", session.seat().name);
        assertTrue(session.seat().away);
        assertTrue(session.players.get(2).isFree());
        assertFalse(session.players.get(0).isFree());
        assertEquals(1, session.freeSlots);
        assertTrue(session.joinable);
        assertNull(session.refusal);
        assertEquals("Co-op (Nucleus): Portal 2", coop.entryTitle(Integer.MAX_VALUE));
        assertNull(coop.entryTitle(1));
        assertEquals(Arrays.asList("pc|coop"), transport.calls);
    }

    @Test
    public void anEndedSessionAndNewerValuesReadTolerantly() throws Exception {
        transport.reply("pc", "coop", NativeApiFixtures.ok("{\"available\":true,\"games\":[],\"sessions\":[{\"id\":1,"
                + "\"app_id\":5,\"title\":\"Game\",\"phase\":\"paused\",\"end\":{\"reason\":\"crashed\",\"detail\":"
                + "\"Instance 2 exited\"},\"players\":[],\"free_slots\":0,\"joinable\":false,\"refusal\":"
                + "\"The session has ended.\"},{\"id\":2,\"app_id\":5,\"title\":\"Game\",\"phase\":\"ended\","
                + "\"end\":{\"reason\":\"vanished\"},\"free_slots\":0,\"joinable\":false}]}"));
        NativeCoop coop = client().coop();
        NativeCoop.Session crashed = coop.sessions.get(0);
        assertSame(NativeCoop.Phase.UNKNOWN, crashed.phase);
        assertSame(NativeCoop.EndReason.CRASHED, crashed.endReason);
        assertEquals("Instance 2 exited", crashed.endDetail);
        assertEquals("The session has ended.", crashed.refusal);
        assertNull(crashed.seat());
        NativeCoop.Session other = coop.sessions.get(1);
        assertSame(NativeCoop.EndReason.UNKNOWN, other.endReason);
        assertNull(other.endDetail);
        assertTrue(other.players.isEmpty());
    }

    @Test
    public void anUnavailableHostSaysWhy() throws Exception {
        transport.reply("pc", "coop", NativeApiFixtures.ok("{\"available\":false,\"reason\":"
                + "\"Nucleus Co-op is not installed.\",\"games\":[],\"sessions\":[]}"));
        NativeCoop coop = client().coop();
        assertFalse(coop.available);
        assertEquals("Nucleus Co-op is not installed.", coop.reason);
    }

    @Test
    public void aListingWithoutAvailableIsMalformed() {
        transport.reply("pc", "coop", NativeApiFixtures.ok("{\"sessions\":[]}"));
        try {
            client().coop();
            fail("A listing without available must not read as one");
        } catch (NativeApiException e) {
            assertSame(NativeApiException.Kind.MALFORMED, e.getKind());
        }
    }

    @Test
    public void joinSendsTheUnsignedSessionIdAndReadsTheSlot() throws Exception {
        transport.reply("pc", "coop.join", NativeApiFixtures.ok(
                "{\"session\":3000000000,\"slot\":2,\"app_id\":2147483647}"));
        NativeCoop.Joined joined = client().joinCoop((int) 3000000000L);
        assertEquals(Arrays.asList("3000000000"), transport.arguments);
        assertEquals(3000000000L, Integer.toUnsignedLong(joined.session));
        assertEquals(2, joined.slot);
        assertEquals(Integer.MAX_VALUE, joined.appId);
    }

    @Test
    public void joinErrorsCarryTheHostsWordsAndStatus() {
        // coop_refused is newer than the codes the bridge pins: it arrives as unknown with its 409.
        transport.reply("pc", "coop.join", "{\"error\":{\"kind\":\"api\",\"message\":\"The session is full.\","
                + "\"http_status\":409,\"code\":\"unknown\"}}");
        try {
            client().joinCoop(1);
            fail("A refused join must throw");
        } catch (NativeApiException e) {
            assertEquals("The session is full.", e.getMessage());
            assertEquals(409, e.getHttpStatus());
            assertSame(NativeCoop.Problem.REFUSED, NativeCoop.Problem.of(e));
        }
    }

    @Test
    public void eachProblemReadsAsCoopRsMapsIt() {
        assertSame(NativeCoop.Problem.PERMISSION, NativeCoop.Problem.of(error("permission_required", 403, "off")));
        assertSame(NativeCoop.Problem.PERMISSION, NativeCoop.Problem.of(error("unknown", 403, "off")));
        assertSame(NativeCoop.Problem.GONE, NativeCoop.Problem.of(error("no_session", 409, "gone")));
        assertSame(NativeCoop.Problem.BUSY, NativeCoop.Problem.of(error("session_busy", 409, "busy")));
        assertSame(NativeCoop.Problem.UNAVAILABLE,
                NativeCoop.Problem.of(error("temporarily_unavailable", 503, "later")));
        assertSame(NativeCoop.Problem.UNAVAILABLE, NativeCoop.Problem.of(error("unknown", 503, "later")));
        assertSame(NativeCoop.Problem.REFUSED, NativeCoop.Problem.of(error("coop_refused", 409, "full")));
        assertSame(NativeCoop.Problem.REFUSED, NativeCoop.Problem.of(error("unknown", 409, "full")));
        assertSame(NativeCoop.Problem.OTHER, NativeCoop.Problem.of(error("unknown", 500, "broke")));
        assertSame(NativeCoop.Problem.UNREACHABLE, NativeCoop.Problem.of(
                new NativeApiException(NativeApiException.Kind.NETWORK, "connection refused")));
    }

    @Test
    public void aCapabilitiesReadIsKnownForTheMenu() throws Exception {
        assertNull(KnownCapabilities.of("pc"));
        String json = "{\"api_version\":1,\"host\":{\"name\":\"PC\",\"version\":\"3.0.0\",\"unique_id\":\"U\"},"
                + "\"features\":[\"coop\"],\"permissions\":{\"list\":true,\"view\":true,\"launch\":true,"
                + "\"host_control\":false}}";
        transport.reply("pc", "capabilities", NativeApiFixtures.ok(json));
        assertTrue(client().capabilities().has(NativeCapabilities.COOP));
        assertTrue(KnownCapabilities.of("pc").contains("\"coop\""));
        assertNull(KnownCapabilities.of("other"));
    }

    @Test
    public void aFailedCapabilitiesReadRecordsNothing() {
        transport.unreachable("pc");
        try {
            client().capabilities();
            fail("An unreachable host must throw");
        } catch (NativeApiException e) {
            assertTrue(e.isUnreachable());
        }
        assertNull(KnownCapabilities.of("pc"));
    }
}
