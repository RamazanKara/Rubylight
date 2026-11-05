package com.limelight.nativeapi;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

import android.app.Application;

import org.json.JSONObject;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;

/** What GET /session says about Continue here, and which hosts offer it. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class NativeStreamMoveTest {
    private static String capabilities(String features, boolean view, boolean launch) {
        return "{\"api_version\":1,\"features\":[" + features + "],\"permissions\":{\"view\":" + view
                + ",\"launch\":" + launch + "}}";
    }

    @Test
    public void onlyAHostThatListsItWithViewAndLaunchOffersIt() {
        String listed = "\"session.continue_here\"";
        assertTrue(NativeCapabilities.offersContinueHere(capabilities(listed, true, true)));
        assertFalse(NativeCapabilities.offersContinueHere(capabilities(listed, true, false)));
        assertFalse(NativeCapabilities.offersContinueHere(capabilities(listed, false, true)));
        assertFalse(NativeCapabilities.offersContinueHere(capabilities("\"session\"", true, true)));
        // Capabilities not read in this process, as on an older host: today's start.
        assertFalse(NativeCapabilities.offersContinueHere(null));
        assertFalse(NativeCapabilities.offersContinueHere("not json"));
    }

    @Test
    public void anOldHostSaysNothing() throws Exception {
        StreamMove move = StreamMove.parse(new JSONObject("{\"revision\":1}"));
        assertNull(move.playingOn);
        assertNull(move.movedTo);
        // The transport's own JSON spells absent as null.
        move = StreamMove.parse(new JSONObject("{\"playing_on\":null,\"moved_to\":null}"));
        assertNull(move.playingOn);
        assertNull(move.movedTo);
    }

    @Test
    public void bothPartsAreRead() throws Exception {
        StreamMove move = StreamMove.parse(new JSONObject("{\"playing_on\":{\"device\":\"Phone\",\"app\":"
                + "{\"install_id\":\"u\",\"app_id\":4000000000,\"title\":\"Game\"}},"
                + "\"moved_to\":{\"device\":\"TV\",\"since_unix_ms\":5}}"));
        assertEquals("Phone", move.playingOn);
        assertEquals("Game", move.playingTitle);
        assertEquals("u", move.playingAppUuid);
        // Host ids are unsigned; NvApp keeps them in an int.
        assertEquals((int) 4000000000L, move.playingAppId);
        assertEquals("TV", move.movedTo);
        assertFalse(move.ownedHere);
        assertTrue(StreamMove.parse(new JSONObject("{\"owns_game\":true}")).ownedHere);
        assertFalse(move.pressFailed);
        assertTrue(StreamMove.parse(new JSONObject("{\"press_failed\":true}")).pressFailed);
    }
}
