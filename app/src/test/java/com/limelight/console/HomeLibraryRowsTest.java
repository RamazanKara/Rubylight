package com.limelight.console;

import static com.limelight.nativeapi.NativeApiFixtures.classicHost;
import static com.limelight.nativeapi.NativeApiFixtures.library;
import static com.limelight.nativeapi.NativeApiFixtures.nativeHost;
import static com.limelight.nativeapi.NativeApiFixtures.ok;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

import android.app.Application;

import com.limelight.nativeapi.NativeApiFixtures;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvApp;

import org.junit.After;
import org.junit.Before;
import org.junit.Rule;
import org.junit.Test;
import org.junit.rules.TemporaryFolder;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;

import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collections;
import java.util.HashSet;
import java.util.List;

/** TV Home's library rows: one per store, Other apps, Recently played and not added, each only when the host can tell. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class HomeLibraryRowsTest {
    @Rule public TemporaryFolder temporary = new TemporaryFolder();

    private ConsoleRig rig;

    @Before
    public void setUp() throws Exception {
        rig = new ConsoleRig(temporary.newFolder());
    }

    @After
    public void tearDown() {
        rig.close();
    }

    /** A game JSON; store null is an app no store owns, installed null the host not saying. */
    static String game(int appId, String title, String store, Boolean installed, Long lastPlayed) {
        return "{\"install_id\":\"i" + appId + "\",\"app_id\":" + appId + ",\"title\":\"" + title + "\","
                + "\"game_id\":\"g" + appId + "\",\"store\":"
                + (store == null ? "null" : "{\"source\":\"" + store + "\"}") + ",\"installed\":" + installed
                + (lastPlayed == null ? "" : ",\"last_played\":" + lastPlayed) + ",\"running\":false}";
    }

    static String found(String title, String store) {
        return "{\"install_id\":null,\"title\":\"" + title + "\",\"game_id\":\"found:" + title + "\","
                + "\"store\":{\"source\":\"" + store + "\"},\"installed\":true,\"added\":false}";
    }

    private void tower(List<String> features, String... games) {
        StringBuilder list = new StringBuilder("[\"library\"");
        for (String feature : features) list.append(",\"").append(feature).append('"');
        rig.transport.reply("tower", "capabilities", ok("{\"api_version\":1,\"features\":" + list + "],"
                + "\"permissions\":{\"list\":true,\"view\":true,\"launch\":true}}"));
        rig.transport.reply("tower", "library", ok(library("t", games)));
        rig.transport.reply("tower", "session", ok(NativeApiFixtures.IDLE));
    }

    private HomeModel model() throws Exception {
        List<ComputerDetails> hosts = Collections.singletonList(nativeHost("tower", "Tower", true));
        return HomeModel.of(rig.repository.refresh(hosts), new HashSet<>(Collections.singletonList("tower")));
    }

    private static List<String> keys(HomeModel model) {
        List<String> keys = new ArrayList<>();
        for (HomeModel.Row row : model.rows) keys.add(row.key);
        return keys;
    }

    private static List<String> titles(List<HomeModel.GameCard> cards) {
        List<String> titles = new ArrayList<>();
        for (HomeModel.GameCard card : cards) titles.add(card.title);
        return titles;
    }

    @Test
    public void eachStoreIsARowLargestFirstAndAppsOfNoStoreFollow() throws Exception {
        tower(Collections.emptyList(),
                game(1, "Desktop", null, true, null), game(2, "Portal", "steam", true, null),
                game(3, "Hades", "epic", true, null), game(4, "Celeste", "steam", true, null),
                game(5, "Witcher", "gog", true, null), game(6, "Alan Wake", "epic", true, null),
                game(7, "Doom", "steam", true, null));
        HomeModel model = model();
        // 3 Steam, 2 Epic, 1 GOG, then the one app of no store.
        assertEquals(Arrays.asList("shelf:store:steam", "shelf:store:epic", "shelf:store:gog", "shelf:other"),
                keys(model));
        assertEquals(Arrays.asList("Celeste", "Doom", "Portal"), titles(model.row("shelf:store:steam").cards));
        assertEquals("Steam", model.row("shelf:store:steam").storeName);
        assertEquals("Epic Games", model.row("shelf:store:epic").storeName);
        assertEquals("store:steam", model.row("shelf:store:steam").filter);
        assertEquals(Arrays.asList("Desktop"), titles(model.row("shelf:other").cards));
        assertEquals(HomeModel.RowKind.OTHER, model.row("shelf:other").kind);
        assertEquals("game:game:g2", model.row("shelf:store:steam").cards.get(2).key);
    }

    @Test
    public void aLibraryWithNoStoresIsOnePlainRowAsBefore() {
        NvApp app = new NvApp("Desktop", 1, false);
        rig.classicApps.put("old", Collections.singletonList(app));
        ComputerDetails old = classicHost("old", "Old PC");
        HomeModel model;
        try {
            model = HomeModel.of(rig.repository.refresh(Collections.singletonList(old)),
                    new HashSet<>(Collections.singletonList("old")));
        } catch (Exception e) {
            throw new AssertionError(e);
        }
        assertEquals(Collections.singletonList("shelf:library"), keys(model));
        assertEquals("all", model.row("shelf:library").filter);
    }

    @Test
    public void aRowStopsAtTwentyAndTellsHowManyMoreAnAllTileOpens() throws Exception {
        List<String> games = new ArrayList<>();
        for (int i = 1; i <= 23; i++) games.add(game(i, String.format("Game %02d", i), "steam", true, null));
        tower(Collections.emptyList(), games.toArray(new String[0]));
        HomeModel.Row steam = model().row("shelf:store:steam");
        assertEquals(HomeModel.ROW_CAP, steam.cards.size());
        assertEquals(23, steam.total);
        assertTrue(steam.hasMore());
        assertEquals("Game 20", steam.cards.get(19).title);
    }

    @Test
    public void aGameNotInstalledStaysInItsStoresRowAfterTheInstalledOnes() throws Exception {
        tower(Collections.emptyList(), game(1, "Alan Wake", "epic", false, null), game(2, "Hades", "epic", true, null),
                game(3, "Control", "epic", null, null));
        HomeModel model = model();
        HomeModel.Row epic = model.row("shelf:store:epic");
        assertEquals(Arrays.asList("Control", "Hades", "Alan Wake"), titles(epic.cards));
        assertTrue(epic.cards.get(2).notInstalled());
        assertFalse(epic.cards.get(1).notInstalled());
        assertFalse("the host not saying is not 'not installed'", epic.cards.get(0).notInstalled());
        // The phone's grid still leaves it out.
        assertEquals(Arrays.asList("Control", "Hades"), titles(model.library));
    }

    @Test
    public void recentlyPlayedNeedsTheHostsLastPlayedAndIsNewestFirst() throws Exception {
        tower(Collections.emptyList(), game(1, "Portal", "steam", true, null), game(2, "Hades", "epic", true, null));
        assertNull(model().row("shelf:recent"));

        rig.close();
        rig = new ConsoleRig(temporary.newFolder());
        tower(Collections.emptyList(), game(1, "Portal", "steam", true, 1_700_000_000L),
                game(2, "Hades", "epic", true, 1_760_000_000L), game(3, "Doom", "steam", true, null));
        HomeModel model = model();
        assertEquals("shelf:recent", model.rows.get(0).key);
        assertEquals(Arrays.asList("Hades", "Portal"), titles(model.row("shelf:recent").cards));
        assertFalse(model.row("shelf:recent").hasMore());
        assertEquals("recent:game:g2", model.row("shelf:recent").cards.get(0).key);
    }

    @Test
    public void gamesNotAddedAreARowOnlyWhenTheHostListsThem() throws Exception {
        tower(Collections.emptyList(), game(1, "Portal", "steam", true, null), found("Celeste", "gog"));
        HomeModel without = model();
        assertNull(without.row("shelf:unadded"));
        assertFalse(titles(without.allGames).contains("Celeste"));
        assertEquals("", rig.transport.arguments.get(rig.transport.calls.indexOf("tower|library")));

        rig.close();
        rig = new ConsoleRig(temporary.newFolder());
        tower(Collections.singletonList("library.unadded"), game(1, "Portal", "steam", true, null),
                found("Celeste", "gog"));
        HomeModel with = model();
        assertEquals(Arrays.asList("shelf:store:steam", "shelf:unadded"), keys(with));
        HomeModel.GameCard celeste = with.row("shelf:unadded").cards.get(0);
        assertTrue(celeste.notAdded());
        assertEquals("unadded:game:found:Celeste", celeste.key);
        assertEquals("gog", celeste.store());
        assertNull("not a store row of its own", with.row("shelf:store:gog"));
        assertEquals("the PC counts what it has added", 1, with.hosts.get(0).gameCount);
    }

    @Test
    public void theFiltersFollowWhatTheLibraryHas() throws Exception {
        tower(Collections.singletonList("library.unadded"), game(1, "Desktop", null, true, null),
                game(2, "Portal", "steam", true, null), game(3, "Doom", "steam", false, null),
                game(4, "Hades", "epic", true, null), found("Celeste", "gog"));
        HomeModel model = model();
        List<String> filters = new ArrayList<>();
        for (HomeModel.Filter filter : model.filters) filters.add(filter.key + "=" + filter.count);
        assertEquals(Arrays.asList("all=4", "installed=3", "store:steam=2", "store:epic=1", "other=1", "unadded=1"),
                filters);
        List<String> steam = new ArrayList<>();
        for (HomeModel.GameCard card : model.allGames) {
            if (HomeModel.matches("store:steam", card)) steam.add(card.title);
        }
        assertEquals(Arrays.asList("Doom", "Portal"), steam);
        int installed = 0;
        for (HomeModel.GameCard card : model.allGames) if (HomeModel.matches("installed", card)) installed++;
        assertEquals(3, installed);
        assertNotNull(model.filters.get(2).storeName);
    }

    @Test
    public void everythingInstalledNeedsNoInstalledChip() throws Exception {
        tower(Collections.emptyList(), game(1, "Portal", "steam", true, null), game(2, "Hades", "epic", true, null));
        List<String> filters = new ArrayList<>();
        for (HomeModel.Filter filter : model().filters) filters.add(filter.key);
        assertEquals(Arrays.asList("all", "store:epic", "store:steam"), filters);
    }
}
