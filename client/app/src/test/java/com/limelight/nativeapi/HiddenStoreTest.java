package com.limelight.nativeapi;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import android.app.Application;
import android.content.SharedPreferences;

import com.limelight.AppView;

import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;

import java.util.Arrays;
import java.util.Collections;
import java.util.HashMap;
import java.util.HashSet;
import java.util.Map;
import java.util.Set;

/** A game stays hidden through a host's renumbering, with the list a PC's app grid reads still plain app ids. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class HiddenStoreTest {
    private SharedPreferences prefs;
    private HiddenStore store;

    @Before
    public void setUp() {
        Application app = RuntimeEnvironment.getApplication();
        prefs = app.getSharedPreferences(AppView.HIDDEN_APPS_PREF_FILENAME, 0);
        store = new HiddenStore.Preferences(app);
    }

    private static Map<String, Integer> games(Object... pairs) {
        Map<String, Integer> games = new HashMap<>();
        for (int at = 0; at < pairs.length; at += 2) games.put((String) pairs[at], (Integer) pairs[at + 1]);
        return games;
    }

    private static Set<Integer> ids(Integer... ids) {
        return new HashSet<>(Arrays.asList(ids));
    }

    private Set<String> gridList() {
        return prefs.getStringSet("host", Collections.emptySet());
    }

    @Test
    public void aRenumberMovesTheGridsListToo() {
        store.set("host", 1, "alpha", true);
        assertEquals(ids(7), store.following("host", games("alpha", 7, "beta", 1)));
        assertEquals(Collections.singleton("7"), gridList());
        assertEquals(ids(7), store.of("host"));
    }

    @Test
    public void hostsSwappingTwoIdsLoseNeitherChoice() {
        store.set("host", 1, "alpha", true);
        assertEquals(ids(2), store.following("host", games("alpha", 2, "beta", 1)));
        assertEquals(games("alpha", 2), store.ids("host"));
        store.set("host", 1, "beta", true);
        assertEquals(ids(1, 2), store.following("host", games("alpha", 1, "beta", 2)));
    }

    @Test
    public void whatWasHiddenBeforeStableIdsWereKeptIsLearnedAndThenFollowed() {
        prefs.edit().putStringSet("host", Collections.singleton("5")).commit();
        assertEquals(ids(5), store.following("host", games("alpha", 5, "beta", 6)));
        assertEquals(games("alpha", 5), store.ids("host"));
        assertEquals(ids(9), store.following("host", games("alpha", 9, "beta", 6)));
    }

    @Test
    public void whatThePcsGridShowsAgainStaysShownThroughARenumber() {
        store.set("host", 1, "alpha", true);
        store.following("host", games("alpha", 1));
        // AppView writes only the plain list.
        prefs.edit().putStringSet("host", Collections.emptySet()).commit();
        assertEquals(ids(), store.following("host", games("alpha", 1)));
        assertEquals(ids(), store.following("host", games("alpha", 4)));
        assertTrue(store.ids("host").isEmpty());
    }

    @Test
    public void aGridThatRenumberedFirstIsNotUndone() {
        store.set("host", 1, "alpha", true);
        prefs.edit().putStringSet("host", Collections.singleton("4")).commit();
        assertEquals(ids(4), store.following("host", games("alpha", 4)));
        assertEquals(games("alpha", 4), store.ids("host"));
    }

    @Test
    public void aGameNotListedIsKeptForWhenItComesBack() {
        store.set("host", 1, "alpha", true);
        assertEquals(ids(1), store.following("host", games("beta", 2)));
        assertEquals(ids(3), store.following("host", games("alpha", 3, "beta", 2)));
    }

    @Test
    public void aGameWithNoStableIdGoesByItsAppIdAndShowingOneForgetsIt() {
        store.set("host", 4, "", true);
        assertEquals(ids(4), store.of("host"));
        assertTrue(store.ids("host").isEmpty());
        store.set("host", 4, "", false);
        assertEquals(ids(), store.of("host"));

        store.set("host", 1, "a:b", true);
        assertEquals(games("a:b", 1), store.ids("host"));
        store.set("host", 1, "a:b", false);
        assertTrue(store.ids("host").isEmpty());
        assertFalse(store.following("host", games("a:b", 2)).contains(2));
    }

    @Test
    public void hostsDoNotShareTheirIdsAndTheInMemoryOneDoesTheSame() {
        HiddenStore memory = new HiddenStore.InMemory();
        for (HiddenStore each : Arrays.asList(store, memory)) {
            each.set("host", 1, "alpha", true);
            assertEquals(ids(), each.following("other", games("alpha", 3)));
            assertEquals(ids(3), each.following("host", games("alpha", 3)));
        }
    }

    @Test
    public void showingFromAMenuOlderThanTheRenumberStillShowsIt() {
        store.set("host", 1, "alpha", true);
        store.following("host", games("alpha", 7));
        store.set("host", 1, "alpha", false);
        assertEquals(ids(), store.of("host"));
        assertTrue(store.ids("host").isEmpty());
    }

    @Test
    public void showingFromAStaleMenuLeavesTheGameThatHasItsOldIdNow() {
        store.set("host", 1, "alpha", true);
        store.set("host", 2, "beta", true);
        store.following("host", games("alpha", 7, "beta", 1));
        store.set("host", 1, "alpha", false);
        assertEquals(ids(1), store.of("host"));
        assertEquals(games("beta", 1), store.ids("host"));
    }

    @Test
    public void showingAGameTheGridMovedFirstTakesTheMenusIdToo() {
        store.set("host", 1, "alpha", true);
        prefs.edit().putStringSet("host", Collections.singleton("7")).commit();
        store.set("host", 7, "alpha", false);
        assertEquals(ids(), store.of("host"));
    }

    @Test
    public void anIdListedTwiceFollowsNeitherApp() {
        store.set("host", 1, "alpha", true);
        HiddenStore.Listing listed = new HiddenStore.Listing();
        listed.add("alpha", 5);
        listed.add("alpha", 6);
        listed.add("alpha", 7);
        listed.add("", 8);
        listed.add(null, 9);
        listed.add("beta", 2);
        assertEquals(games("beta", 2), listed.games());
        assertEquals(ids(1), store.following("host", listed.games()));
    }

    @Test
    public void theGridsChangeGoesOnTheListAsItIsNow() {
        HiddenStore.Preferences grid = new HiddenStore.Preferences(RuntimeEnvironment.getApplication());
        store.set("host", 1, "alpha", true);
        store.following("host", games("alpha", 7));
        assertEquals(ids(7, 3), grid.changed("host", ids(9), ids(3)));
        assertEquals(new HashSet<>(Arrays.asList("7", "3")), gridList());
        assertEquals("its stable ids stay", games("alpha", 7), store.ids("host"));
    }

    /** Each screen has its own store: without one lock for all, one's write loses another's. */
    @Test
    public void storesOfDifferentScreensLoseNoWrite() throws Exception {
        int writers = 8;
        int each = 50;
        java.util.concurrent.ExecutorService pool = java.util.concurrent.Executors.newFixedThreadPool(writers);
        try {
            java.util.List<java.util.concurrent.Future<?>> done = new java.util.ArrayList<>();
            for (int writer = 0; writer < writers; writer++) {
                int base = writer * each;
                done.add(pool.submit(() -> {
                    HiddenStore own = new HiddenStore.Preferences(RuntimeEnvironment.getApplication());
                    for (int id = base; id < base + each; id++) own.set("host", id, "game" + id, true);
                }));
            }
            for (java.util.concurrent.Future<?> one : done) one.get();
        } finally {
            pool.shutdownNow();
        }
        assertEquals(writers * each, store.of("host").size());
        assertEquals(writers * each, store.ids("host").size());
    }

    @Test
    public void anUnreadableEntryIsSkippedNotThrown() {
        prefs.edit().putStringSet("host", new HashSet<>(Arrays.asList("x", "3")))
                .putStringSet(HiddenStore.Preferences.idsKey("host"), new HashSet<>(Arrays.asList("nocolon", "z:a", "3:alpha"))).commit();
        assertEquals(ids(3), store.of("host"));
        assertEquals(games("alpha", 3), store.ids("host"));
    }
}
