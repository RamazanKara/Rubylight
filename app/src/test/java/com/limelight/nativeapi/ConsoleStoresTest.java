package com.limelight.nativeapi;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

import android.app.Application;
import android.content.Context;


import org.junit.Rule;
import org.junit.Test;
import org.junit.rules.TemporaryFolder;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;

import java.io.File;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.util.List;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class ConsoleStoresTest {
    @Rule public TemporaryFolder temporary = new TemporaryFolder();

    private static RecentsStore.Recent recent(String host, int appId, long at) {
        return new RecentsStore.Recent(host, host + " PC", appId, "Game " + appId, at);
    }

    @Test
    public void theLibraryCacheKeepsOneFilePerHostAndSurvivesAReopen() throws Exception {
        File dir = new File(temporary.getRoot(), "libraries");
        new FileLibraryCache(dir).store("a/b", new LibraryCache.Entry("{\"revision\":\"r\",\"games\":[]}", 42));
        LibraryCache.Entry entry = new FileLibraryCache(dir).load("a/b");
        assertNotNull(entry);
        assertEquals(42, entry.fetchedAtMs);
        assertEquals("r", new org.json.JSONObject(entry.libraryJson).getString("revision"));
        assertNull(new FileLibraryCache(dir).load("other"));
        assertEquals(1, dir.list().length);
    }

    @Test
    public void aBrokenCacheFileReadsAsNothingAndTheNextStoreRepairsIt() throws Exception {
        File dir = temporary.newFolder();
        FileLibraryCache cache = new FileLibraryCache(dir);
        cache.store("h", new LibraryCache.Entry("{\"games\":[]}", 1));
        Files.write(new File(dir, "h.json").toPath(), "{not json".getBytes(StandardCharsets.UTF_8));
        assertNull(cache.load("h"));
        cache.store("h", new LibraryCache.Entry("{\"games\":[]}", 2));
        assertEquals(2, cache.load("h").fetchedAtMs);
    }

    @Test
    public void forgettingAHostRemovesItsCacheFile() throws Exception {
        FileLibraryCache cache = new FileLibraryCache(temporary.newFolder());
        cache.store("h", new LibraryCache.Entry("{\"games\":[]}", 1));
        cache.forget("h");
        assertNull(cache.load("h"));
        cache.forget("never-there");
    }

    @Test
    public void recentsAreNewestFirstWithoutDuplicatesAndCappedAtTwenty() {
        Context context = RuntimeEnvironment.getApplication();
        PreferencesRecentsStore store = new PreferencesRecentsStore(context);
        assertTrue(store.load().isEmpty());
        for (int i = 0; i < 25; i++) {
            store.record(recent("h", i, 100 + i));
        }
        store.record(recent("h", 20, 500));
        store.record(recent("other", 20, 501));
        List<RecentsStore.Recent> recents = store.load();
        assertEquals(PreferencesRecentsStore.MAX, recents.size());
        assertEquals("other", recents.get(0).hostUuid);
        assertEquals(500, recents.get(1).playedAtMs);
        assertEquals(20, recents.get(1).appId);
        // Reopened, it reads the same list.
        assertEquals(recents.size(), new PreferencesRecentsStore(context).load().size());
        int sameApp = 0;
        for (RecentsStore.Recent recent : recents) {
            if (recent.hostUuid.equals("h") && recent.appId == 20) {
                sameApp++;
            }
        }
        assertEquals(1, sameApp);
    }

    @Test
    public void forgettingAHostDropsOnlyItsRecents() {
        PreferencesRecentsStore store = new PreferencesRecentsStore(RuntimeEnvironment.getApplication());
        store.record(recent("a", 1, 1));
        store.record(recent("b", 1, 2));
        store.forgetHost("a");
        List<RecentsStore.Recent> recents = store.load();
        assertEquals(1, recents.size());
        assertEquals("b", recents.get(0).hostUuid);
    }

    private static FavouritesStore.Favourite star(String host, int appId) {
        return new FavouritesStore.Favourite(host, host + " PC", appId, "Game " + appId);
    }

    @Test
    public void favouritesAreNewestStarFirstPerHostAndGameAndSurviveAReopen() {
        Context context = RuntimeEnvironment.getApplication();
        PreferencesFavouritesStore store = new PreferencesFavouritesStore(context);
        assertTrue(store.load().isEmpty());
        store.set(star("a", 1), true);
        store.set(star("b", 1), true);
        store.set(star("a", 2), true);
        // Starring again changes nothing but the stored names.
        store.set(new FavouritesStore.Favourite("a", "Renamed", 1, "Renamed game"), true);

        List<FavouritesStore.Favourite> favourites = new PreferencesFavouritesStore(context).load();
        assertEquals(3, favourites.size());
        assertEquals("a", favourites.get(0).hostUuid);
        assertEquals(2, favourites.get(0).appId);
        assertEquals("Renamed game", favourites.get(2).title);
        assertTrue(store.isFavourite("b", 1));
        assertFalse(store.isFavourite("b", 2));

        store.set(star("b", 1), false);
        store.set(star("never", 7), false);
        assertFalse(store.isFavourite("b", 1));
        assertTrue(store.isFavourite("a", 1));
        assertEquals(2, store.load().size());
    }

    @Test
    public void forgettingAHostDropsOnlyItsFavouritesAndABrokenFileReadsAsNone() {
        Context context = RuntimeEnvironment.getApplication();
        PreferencesFavouritesStore store = new PreferencesFavouritesStore(context);
        store.set(star("a", 1), true);
        store.set(star("b", 1), true);
        store.forgetHost("a");
        assertEquals(1, store.load().size());
        assertEquals("b", store.load().get(0).hostUuid);

        context.getSharedPreferences(PreferencesFavouritesStore.PREFS, Context.MODE_PRIVATE).edit()
                .putString(PreferencesFavouritesStore.KEY, "{not json").commit();
        assertTrue(store.load().isEmpty());
        store.set(star("c", 3), true);
        assertEquals(1, store.load().size());
    }
}
