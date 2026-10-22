package com.limelight.nativeapi;

import static com.limelight.nativeapi.NativeApiFixtures.error;
import static com.limelight.nativeapi.NativeApiFixtures.game;
import static com.limelight.nativeapi.NativeApiFixtures.library;
import static com.limelight.nativeapi.NativeApiFixtures.ok;
import static com.limelight.nativeapi.NativeApiFixtures.session;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;
import static org.junit.Assert.fail;

import android.app.Application;

import com.limelight.nvstream.http.ComputerDetails;

import org.json.JSONObject;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;

import java.util.Arrays;
import java.util.Collections;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class NativeApiClientTest {
    private final NativeApiFixtures.FakeTransport transport = new NativeApiFixtures.FakeTransport();

    private NativeApiClient client() {
        ComputerDetails host = NativeApiFixtures.nativeHost("pc", "PC", true);
        return new NativeApiClient(transport, HostEndpoint.of(host, "unique", NativeApiFixtures.crypto()));
    }

    @Test
    public void theEndpointCarriesTheHostTheIdentityAndAPemCertificate() {
        ComputerDetails host = NativeApiFixtures.nativeHost("pc", "PC", true);
        HostEndpoint endpoint = HostEndpoint.of(host, "unique", NativeApiFixtures.crypto());
        assertEquals(host.activeAddress.address, endpoint.address);
        assertEquals(47989, endpoint.httpPort);
        assertEquals(47984, endpoint.httpsPort);
        assertEquals("unique", endpoint.uniqueId);
        assertTrue(endpoint.serverCertificatePem.startsWith("-----BEGIN CERTIFICATE-----\n"));
        assertTrue(endpoint.serverCertificatePem.endsWith("\n-----END CERTIFICATE-----\n"));
        for (String line : endpoint.serverCertificatePem.split("\n")) {
            assertTrue(line, line.length() <= 64 || line.startsWith("-----"));
        }
        assertTrue(endpoint.clientCertificateDer.length > 0);
        assertTrue(endpoint.clientPrivateKeyPkcs8.length > 0);
        host.serverCert = null;
        assertNull(HostEndpoint.of(host, "unique", NativeApiFixtures.crypto()));
    }

    @Test
    public void aLibraryKeepsTheGameIdentityAndCoverAndSurvivesTheCacheRoundTrip() throws Exception {
        transport.reply("pc", "library", ok(library("r5",
                game("steam-1", 11, "Portal", "steam:400", true), game("local-2", 12, "Notepad", null, false))));
        NativeLibrary library = client().library();
        assertEquals("r5", library.revision);
        NativeLibrary.Game portal = library.games.get(0);
        assertEquals("steam-1", portal.installId);
        assertEquals(11, portal.appId);
        assertEquals("steam:400", portal.gameId);
        assertEquals("steam", portal.storeSource);
        assertEquals(Boolean.TRUE, portal.installed);
        assertTrue(portal.running);
        assertEquals("/api/native/v1/cover/steam-1", portal.coverPath);
        assertEquals("v1", portal.coverVersion);
        assertNull(library.games.get(1).gameId);
        assertFalse(library.games.get(1).running);

        NativeLibrary again = NativeLibrary.parse(new JSONObject(library.toJson().toString()));
        assertEquals(library.toJson().toString(), again.toJson().toString());
        assertEquals("steam:400", again.games.get(0).gameId);
        assertNull(again.games.get(1).gameId);
    }

    @Test
    public void aSessionListsWhatThisDeviceMayDo() throws Exception {
        transport.reply("pc", "session", ok(session("suspended", "you",
                "{\"install_id\":\"steam-1\",\"app_id\":11,\"title\":\"Portal\"}",
                "[{\"id\":\"resume\",\"allowed\":true},{\"id\":\"end\",\"allowed\":true},"
                        + "{\"id\":\"close_game\",\"allowed\":false,\"reason\":\"not_session_owner\"},"
                        + "{\"id\":\"teleport\",\"allowed\":true}]")));
        NativeSession session = client().session();
        assertEquals(NativeSession.State.SUSPENDED, session.state);
        assertEquals(NativeSession.Owner.YOU, session.owner);
        assertEquals("Portal", session.app.title);
        assertTrue(session.isActive());
        assertTrue(session.allows(NativeSession.ActionId.RESUME));
        assertTrue(session.allows(NativeSession.ActionId.END));
        assertFalse(session.allows(NativeSession.ActionId.CLOSE_GAME));
        assertEquals("not_session_owner", session.action(NativeSession.ActionId.CLOSE_GAME).reason);
        // Disconnect is not listed, and an action this client does not know is kept apart from the known ones.
        assertNull(session.action(NativeSession.ActionId.DISCONNECT));
        assertEquals(NativeSession.ActionId.UNKNOWN, session.actions.get(3).id);
    }

    @Test
    public void anIdleSessionAndAnUnknownStateAreNotActive() throws Exception {
        transport.reply("pc", "session", ok(NativeApiFixtures.IDLE));
        assertFalse(client().session().isActive());
        transport.reply("pc", "session", ok(session("holographic", "somebody", "null", "[]")));
        NativeSession session = client().session();
        assertEquals(NativeSession.State.UNKNOWN, session.state);
        assertEquals(NativeSession.Owner.UNKNOWN, session.owner);
        assertFalse(session.isActive());
    }

    @Test
    public void sessionActionsGoOutByTheirWireName() throws Exception {
        transport.reply("pc", "session_action", ok(NativeApiFixtures.IDLE));
        NativeApiClient client = client();
        client.sessionAction(NativeSession.ActionId.DISCONNECT);
        client.sessionAction(NativeSession.ActionId.END);
        client.sessionAction(NativeSession.ActionId.CLOSE_GAME);
        assertEquals(Arrays.asList("disconnect", "end", "close_game"), transport.arguments);
    }

    @Test
    public void resumeIsTheLaunchPathAndNotASessionAction() {
        for (NativeSession.ActionId id : new NativeSession.ActionId[] {NativeSession.ActionId.RESUME,
                NativeSession.ActionId.UNKNOWN}) {
            try {
                client().sessionAction(id);
                fail(id.name());
            } catch (NativeApiException e) {
                assertEquals(NativeApiException.Kind.INVALID_ARGUMENT, e.getKind());
            }
        }
        assertTrue(transport.calls.isEmpty());
    }

    @Test
    public void anApiErrorKeepsItsCodeSettingAndRetryDelay() {
        transport.reply("pc", "radeon_write", "{\"error\":{\"kind\":\"api\",\"message\":\"Turn on Host control.\","
                + "\"http_status\":403,\"code\":\"host_control_required\",\"setting\":\"chill.enabled\","
                + "\"retry_after_ms\":250}}");
        try {
            client().writeRadeon(1, Collections.singletonList(new NativeRadeon.Change("chill.enabled", true)));
            fail();
        } catch (NativeApiException e) {
            assertEquals(NativeApiException.Kind.API, e.getKind());
            assertEquals("Turn on Host control.", e.getMessage());
            assertEquals(403, e.getHttpStatus());
            assertEquals("host_control_required", e.getCode());
            assertEquals("chill.enabled", e.getSetting());
            assertEquals(250, e.getRetryAfterMs());
            assertFalse(e.isUnreachable());
        }
    }

    @Test
    public void transportAndMalformedFailuresAreTold() {
        transport.reply("pc", "library", error("certificate_mismatch", "pinned certificate changed"));
        try {
            client().library();
            fail();
        } catch (NativeApiException e) {
            assertEquals(NativeApiException.Kind.CERTIFICATE_MISMATCH, e.getKind());
        }
        transport.reply("pc", "library", error("network", "timed out"));
        try {
            client().library();
            fail();
        } catch (NativeApiException e) {
            assertTrue(e.isUnreachable());
        }
        for (String garbage : new String[] {"not json", "{}", "{\"ok\":{\"revision\":\"x\"}}"}) {
            transport.reply("pc", "library", garbage);
            try {
                client().library();
                fail(garbage);
            } catch (NativeApiException e) {
                assertEquals(garbage, NativeApiException.Kind.MALFORMED, e.getKind());
            }
        }
    }

    @Test
    public void radeonStateParsesTheFiveControlStatesAndKeepsValuesTyped() throws Exception {
        transport.reply("pc", "radeon", ok("{\"revision\":3,\"state\":\"configurable\",\"host_control\":true,"
                + "\"writable_now\":true,\"gpus\":[{\"id\":9,\"name\":\"RX 7900 XT\",\"driver\":\"24.1\",\"settings\":["
                + "{\"id\":\"chill.enabled\",\"group\":\"chill\",\"kind\":\"toggle\",\"state\":\"configurable\","
                + "\"value\":false,\"restore\":{\"original\":false,\"written\":true}},"
                + "{\"id\":\"chill.max\",\"group\":\"chill\",\"kind\":\"integer\",\"state\":\"read_only\",\"value\":144,"
                + "\"range\":{\"min\":30,\"max\":240,\"step\":1},\"unit\":\"fps\",\"reason\":\"driver\"},"
                + "{\"id\":\"scaling\",\"group\":\"display\",\"kind\":\"choice\",\"state\":\"needs_external_setup\","
                + "\"value\":\"off\",\"options\":[{\"value\":\"off\",\"label\":\"Off\"}],\"message\":\"Use Adrenalin.\"},"
                + "{\"id\":\"a\",\"group\":\"g\",\"kind\":\"toggle\",\"state\":\"unsupported\"},"
                + "{\"id\":\"b\",\"group\":\"g\",\"kind\":\"toggle\",\"state\":\"temporarily_unavailable\","
                + "\"value\":{\"weird\":1}},"
                + "{\"id\":\"c\",\"group\":\"g\",\"kind\":\"toggle\",\"state\":\"from_the_future\"}]}]}"));
        NativeRadeon radeon = client().radeon();
        assertEquals(NativeRadeon.ControlState.CONFIGURABLE, radeon.state);
        assertTrue(radeon.hostControl);
        assertTrue(radeon.writableNow);
        NativeRadeon.Gpu gpu = radeon.gpus.get(0);
        assertEquals(9, gpu.id);
        NativeRadeon.Setting chill = gpu.settings.get(0);
        assertTrue(chill.isConfigurable());
        assertEquals(Boolean.FALSE, chill.value);
        assertEquals(Boolean.TRUE, chill.restore.written);
        NativeRadeon.Setting max = gpu.settings.get(1);
        assertEquals(NativeRadeon.ControlState.READ_ONLY, max.state);
        assertEquals(Long.valueOf(144), max.value);
        assertEquals(240, max.range.max);
        assertEquals("fps", max.unit);
        NativeRadeon.Setting scaling = gpu.settings.get(2);
        assertEquals(NativeRadeon.ControlState.NEEDS_EXTERNAL_SETUP, scaling.state);
        assertEquals("off", scaling.value);
        assertEquals("Off", scaling.options.get(0).label);
        assertEquals("Use Adrenalin.", scaling.message);
        assertEquals(NativeRadeon.ControlState.UNSUPPORTED, gpu.settings.get(3).state);
        // A value of a shape this client does not know is left out, as the host's own types do.
        assertEquals(NativeRadeon.ControlState.TEMPORARILY_UNAVAILABLE, gpu.settings.get(4).state);
        assertNull(gpu.settings.get(4).value);
        assertEquals(NativeRadeon.ControlState.UNKNOWN, gpu.settings.get(5).state);
    }

    @Test
    public void aRadeonWriteIsOneGpuAndTypedChanges() throws Exception {
        transport.reply("pc", "radeon_write", ok("{\"revision\":4,\"state\":\"configurable\",\"host_control\":true,"
                + "\"writable_now\":true,\"gpus\":[]}"));
        transport.reply("pc", "radeon_restore", ok("{\"revision\":5,\"state\":\"configurable\",\"host_control\":true,"
                + "\"writable_now\":true,\"gpus\":[]}"));
        NativeApiClient client = client();
        assertEquals(4, client.writeRadeon(9, Arrays.asList(new NativeRadeon.Change("chill.enabled", true),
                new NativeRadeon.Change("chill.max", 90L), new NativeRadeon.Change("scaling", "fsr"))).revision);
        JSONObject sent = new JSONObject(transport.arguments.get(0));
        assertEquals(9, sent.getLong("gpu"));
        assertEquals(true, sent.getJSONArray("changes").getJSONObject(0).get("value"));
        assertEquals(90, sent.getJSONArray("changes").getJSONObject(1).getLong("value"));
        assertEquals("fsr", sent.getJSONArray("changes").getJSONObject(2).get("value"));
        assertEquals(5, client.restoreRadeon().revision);
        assertEquals(Arrays.asList("pc|radeon_write", "pc|radeon_restore"), transport.calls);
    }
}
