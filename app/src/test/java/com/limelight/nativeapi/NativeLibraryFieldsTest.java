package com.limelight.nativeapi;

import static com.limelight.nativeapi.NativeApiFixtures.ok;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

import android.app.Application;

import com.limelight.nvstream.http.ComputerDetails;

import org.json.JSONObject;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;

/** The library and capability fields past the first API version: each optional, unknown ones ignored. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class NativeLibraryFieldsTest {
    private static final String BARE = "{\"install_id\":\"i1\",\"app_id\":5,\"title\":\"Portal\"}";
    private static final String FULL = "{\"install_id\":\"i2\",\"app_id\":6,\"title\":\"Hades\",\"game_id\":\"epic:h\","
            + "\"store\":{\"source\":\"epic\",\"id\":\"h\",\"name\":\"Epic Games\",\"region\":\"eu\"},"
            + "\"installed\":false,\"running\":false,\"added\":true,\"last_played\":1761300000,"
            + "\"playtime_minutes\":125,\"launchable\":false,\"future\":{\"x\":1},"
            + "\"artwork\":{\"cover\":\"/c/i2\",\"version\":\"v3\",\"hero\":\"/h/i2\",\"logo\":\"/l/i2\","
            + "\"landscape\":\"/w/i2\",\"icon\":\"/i/i2\"}}";
    private static final String UNADDED = "{\"install_id\":null,\"title\":\"Celeste\",\"game_id\":\"gog:c\","
            + "\"store\":{\"source\":\"gog\",\"id\":\"c\",\"name\":\"GOG\"},\"installed\":true,\"added\":false}";

    private final NativeApiFixtures.FakeTransport transport = new NativeApiFixtures.FakeTransport();

    private NativeApiClient client() {
        ComputerDetails host = NativeApiFixtures.nativeHost("pc", "PC", true);
        return new NativeApiClient(transport, HostEndpoint.of(host, "unique", NativeApiFixtures.crypto()));
    }

    private static NativeLibrary.Game parse(String json) throws Exception {
        return NativeLibrary.Game.parse(new JSONObject(json));
    }

    @Test
    public void aGameWithoutTheNewFieldsReadsAsAddedAndUntold() throws Exception {
        NativeLibrary.Game game = parse(BARE);
        assertEquals("i1", game.installId);
        assertEquals(5, game.appId);
        assertTrue(game.added);
        assertNull(game.lastPlayed);
        assertNull(game.playtimeMinutes);
        assertNull(game.launchable);
        assertNull(game.storeSource);
        assertNull(game.storeId);
        assertNull(game.storeName);
        assertNull(game.installed);
        assertNull(game.coverPath);
        assertNull(game.heroPath);
        assertNull(game.logoPath);
        assertNull(game.landscapePath);
        assertEquals("", game.coverVersion);
    }

    @Test
    public void everyNewFieldIsReadAndUnknownOnesAreIgnored() throws Exception {
        NativeLibrary.Game game = parse(FULL);
        assertEquals("epic", game.storeSource);
        assertEquals("h", game.storeId);
        assertEquals("Epic Games", game.storeName);
        assertEquals(Boolean.FALSE, game.installed);
        assertTrue(game.added);
        assertEquals(Long.valueOf(1761300000L), game.lastPlayed);
        assertEquals(Integer.valueOf(125), game.playtimeMinutes);
        assertEquals(Boolean.FALSE, game.launchable);
        assertEquals("/c/i2", game.coverPath);
        assertEquals("v3", game.coverVersion);
        assertEquals("/h/i2", game.heroPath);
        assertEquals("/l/i2", game.logoPath);
        assertEquals("/w/i2", game.landscapePath);
    }

    @Test
    public void aGameTheHostHasNotAddedNeedsNoIds() throws Exception {
        NativeLibrary.Game game = parse(UNADDED);
        assertFalse(game.added);
        assertNull(game.installId);
        assertEquals(0, game.appId);
        assertEquals("gog:c", game.gameId);
        assertEquals("gog", game.storeSource);
    }

    @Test(expected = org.json.JSONException.class)
    public void anAddedGameStillNeedsItsInstallId() throws Exception {
        parse("{\"app_id\":5,\"title\":\"Portal\"}");
    }

    @Test
    public void nullsAndNonsenseReadAsUntold() throws Exception {
        NativeLibrary.Game game = parse("{\"install_id\":\"i1\",\"app_id\":5,\"title\":\"Portal\",\"added\":null,"
                + "\"last_played\":null,\"playtime_minutes\":null,\"launchable\":null,\"store\":null,"
                + "\"artwork\":{\"cover\":null,\"hero\":\"\"}}");
        assertTrue(game.added);
        assertNull(game.lastPlayed);
        assertNull(game.playtimeMinutes);
        assertNull(game.launchable);
        assertNull(game.storeSource);
        assertNull(game.coverPath);
        assertNull(game.heroPath);
        assertNull(parse("{\"install_id\":\"i1\",\"app_id\":5,\"title\":\"P\",\"last_played\":\"soon\"}").lastPlayed);
        assertNull(parse("{\"install_id\":\"i1\",\"app_id\":5,\"title\":\"P\",\"last_played\":0}").lastPlayed);
    }

    @Test
    public void theCacheRoundTripKeepsEveryField() throws Exception {
        NativeLibrary library = NativeLibrary.parse(new JSONObject(
                "{\"revision\":\"r1\",\"games\":[" + BARE + "," + FULL + "," + UNADDED + "]}"));
        NativeLibrary again = NativeLibrary.parse(new JSONObject(library.toJson().toString()));
        assertEquals(library.toJson().toString(), again.toJson().toString());
        NativeLibrary.Game full = again.games.get(1);
        assertEquals("Epic Games", full.storeName);
        assertEquals(Long.valueOf(1761300000L), full.lastPlayed);
        assertEquals(Integer.valueOf(125), full.playtimeMinutes);
        assertEquals(Boolean.FALSE, full.launchable);
        assertEquals("/w/i2", full.landscapePath);
        NativeLibrary.Game unadded = again.games.get(2);
        assertFalse(unadded.added);
        assertNull(unadded.installId);
        assertTrue(again.games.get(0).added);
        assertNull(again.games.get(0).storeSource);
    }

    @Test
    public void theUnaddedGamesAreAskedForOnlyWhenWanted() throws Exception {
        transport.reply("pc", "library", ok("{\"revision\":\"r\",\"games\":[" + UNADDED + "]}"));
        client().library();
        client().library(true);
        assertEquals("", transport.arguments.get(0));
        assertEquals("include=unadded", transport.arguments.get(1));
    }

    @Test
    public void addingAGameSendsItsIdAndReadsTheLibraryBack() throws Exception {
        transport.reply("pc", "library_add", ok("{\"revision\":\"r2\",\"games\":[" + BARE + "]}"));
        NativeLibrary library = client().addToLibrary("gog:c");
        assertEquals("pc|library_add", transport.calls.get(0));
        assertEquals("gog:c", new JSONObject(transport.arguments.get(0)).getString("game_id"));
        assertEquals("r2", library.revision);
    }

    @Test
    public void capabilitiesWithoutFeaturesHaveNone() throws Exception {
        NativeCapabilities caps = NativeCapabilities.parse(new JSONObject("{\"api_version\":1,\"permissions\":{}}"));
        assertTrue(caps.features.isEmpty());
        assertFalse(caps.has(NativeCapabilities.LIBRARY_UNADDED));
        assertFalse(caps.has(NativeCapabilities.LIBRARY_ADD));
    }

    @Test
    public void theLibraryFeaturesAreReadAndUnreadableEntriesSkipped() throws Exception {
        NativeCapabilities caps = NativeCapabilities.parse(new JSONObject("{\"api_version\":1,\"permissions\":{},"
                + "\"features\":[\"library\",7,null,{\"x\":1},\"library.unadded\",\"library.add\",\"later\"]}"));
        assertTrue(caps.has(NativeCapabilities.LIBRARY_UNADDED));
        assertTrue(caps.has(NativeCapabilities.LIBRARY_ADD));
        assertTrue(caps.has("later"));
        assertEquals(4, caps.features.size());
    }
}
