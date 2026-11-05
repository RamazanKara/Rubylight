package com.limelight.console;

import static org.junit.Assert.assertArrayEquals;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;
import static org.junit.Assert.fail;

import android.app.Application;

import org.json.JSONArray;
import org.json.JSONObject;
import org.junit.After;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;

import java.io.File;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collections;
import java.util.List;

/**
 * The wire contract with libnightjar_core: each request {@link CollectionGroups} sends must be the one in
 * protocol/android/src/collections_contract.json, whose answers the Rust side's test checks too.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class CollectionGroupsTest {
    @After
    public void forget() {
        CollectionGroups.use(null);
    }

    private static JSONArray cases() throws Exception {
        for (String path : new String[] {"../protocol/android/src/collections_contract.json", "protocol/android/src/collections_contract.json"}) {
            File file = new File(path);
            if (file.isFile()) return new JSONArray(new String(Files.readAllBytes(file.toPath()), StandardCharsets.UTF_8));
        }
        throw new AssertionError("collections_contract.json not found from " + new File(".").getAbsolutePath());
    }

    private static List<List<String>> gamesOf(JSONObject request) throws Exception {
        JSONArray games = request.getJSONArray("games");
        List<List<String>> out = new ArrayList<>();
        for (int i = 0; i < games.length(); i++) {
            List<String> names = new ArrayList<>();
            for (int j = 0; j < games.getJSONArray(i).length(); j++) names.add(games.getJSONArray(i).getString(j));
            out.add(names);
        }
        return out;
    }

    @Test
    public void sendsTheContractsRequestAndReadsItsAnswer() throws Exception {
        JSONObject several = cases().getJSONObject(0);
        CollectionGroups.use(request -> {
            assertEquals(several.optJSONObject("request").toString(), request);
            return several.optJSONObject("response").toString();
        });
        List<CollectionGroups.Group> groups = CollectionGroups.of(gamesOf(several.getJSONObject("request")));
        assertEquals(3, groups.size());
        assertEquals("Couch", groups.get(0).name);
        assertArrayEquals(new int[] {0, 2}, groups.get(0).games);
        assertEquals("VR", groups.get(1).name);
        assertNull(groups.get(2).name);
        assertArrayEquals(new int[] {1}, groups.get(2).games);
    }

    @Test
    public void noCollectionsAsksNothing() throws Exception {
        CollectionGroups.use(request -> {
            fail("asked: " + request);
            return "";
        });
        assertTrue(CollectionGroups.of(gamesOf(cases().getJSONObject(1).getJSONObject("request"))).isEmpty());
    }

    @Test
    public void aRefusalOrAMissingLibraryIsNoGroups() {
        List<List<String>> games = Collections.singletonList(Arrays.asList("VR"));
        CollectionGroups.use(request -> "{\"error\":\"invalid\",\"detail\":\"x\"}");
        assertTrue(CollectionGroups.of(games).isEmpty());
        CollectionGroups.use(request -> {
            throw new UnsatisfiedLinkError("no library");
        });
        assertTrue(CollectionGroups.of(games).isEmpty());
    }
}
