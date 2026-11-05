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

    private static String in(String game, String... collections) {
        return game.substring(0, game.length() - 1) + ",\"collections\":[\"" + String.join("\",\"", collections) + "\"]}";
    }

    @After
    public void forgetTheGroups() {
        CollectionGroups.use(null);
    }

    @Test
    public void rowsByCollectionShowAGameInEachAndTheRestLast() throws Exception {
        tower(Collections.emptyList(),
                in(game(1, "Alpha", "steam", true, null), "VR", "Couch"),
                game(2, "Beta", "steam", true, null),
                in(game(3, "Gamma", "steam", true, null), "couch"));
        CollectionGroups.use(request -> {
            assertEquals("{\"games\":[[\"VR\",\"Couch\"],[],[\"couch\"]]}", request);
            return "{\"ok\":{\"groups\":[{\"name\":\"Couch\",\"games\":[0,2]},{\"name\":\"VR\",\"games\":[0]},"
                    + "{\"name\":null,\"games\":[1]}]}}";
        });
        HomeModel model = model();
        assertTrue(model.hasCollections());
        List<HomeModel.Row> rows = model.rows(HomeView.Group.COLLECTION);
        assertEquals(3, rows.size());
        assertEquals(HomeModel.RowKind.COLLECTION, rows.get(0).kind);
        assertEquals("Couch", rows.get(0).storeName);
        assertEquals("Alpha", rows.get(0).cards.get(0).title);
        assertEquals("Gamma", rows.get(0).cards.get(1).title);
        assertEquals("Alpha", rows.get(1).cards.get(0).title);
        assertEquals(HomeModel.RowKind.NO_COLLECTION, rows.get(2).kind);
        assertEquals("Beta", rows.get(2).cards.get(0).title);
        assertTrue("every row its own key for a game in several",
                !rows.get(0).cards.get(0).key.equals(rows.get(1).cards.get(0).key));
        // Each row is exactly its chip.
        for (HomeModel.Row row : rows) {
            assertNotNull(row.filter, AllGamesActivity.filterOf(model, row.filter));
            assertEquals(row.total, AllGamesActivity.cardsOf(model, row.filter).size());
        }
    }

    @Test
    public void withNoCollectionsGroupingByThemShowsStoresAndIsNotOffered() throws Exception {
        tower(Collections.emptyList(), game(1, "Alpha", "steam", true, null));
        CollectionGroups.use(request -> {
            throw new AssertionError("asked: " + request);
        });
        HomeModel model = model();
        assertFalse(model.hasCollections());
        assertEquals(model.rows(HomeView.Group.STORE), model.rows(HomeView.Group.COLLECTION));
    }

    @Test
    public void aHiddenGameLeavesHomeAndComesBackFromItsChip() throws Exception {
        tower(Collections.emptyList(), game(1, "Alpha", "steam", true, 100L), game(2, "Beta", "steam", true, null));
        HomeModel before = model();
        HomeModel.GameCard alpha = null;
        for (HomeModel.GameCard card : before.library) {
            if (card.title.equals("Alpha")) alpha = card;
        }
        assertNotNull(alpha);
        assertNull(AllGamesActivity.filterOf(before, HomeModel.FILTER_HIDDEN));
        rig.repository.setHidden(before.copiesOf(alpha), true);

        HomeModel after = model();
        assertEquals(1, after.library.size());
        assertEquals("Beta", after.library.get(0).title);
        for (HomeModel.Row row : after.rows) {
            for (HomeModel.GameCard card : row.cards) assertFalse(row.key, card.title.equals("Alpha"));
        }
        assertEquals(1, AllGamesActivity.cardsOf(after, HomeModel.FILTER_ALL).size());
        HomeModel.Filter chip = AllGamesActivity.filterOf(after, HomeModel.FILTER_HIDDEN);
        assertNotNull(chip);
        assertEquals(1, chip.count);
        List<HomeModel.GameCard> hidden = AllGamesActivity.cardsOf(after, HomeModel.FILTER_HIDDEN);
        assertEquals("Alpha", hidden.get(0).title);
        assertTrue(hidden.get(0).hidden());

        rig.repository.setHidden(after.copiesOf(hidden.get(0)), false);
        HomeModel shown = model();
        assertEquals(2, shown.library.size());
        assertNull(AllGamesActivity.filterOf(shown, HomeModel.FILTER_HIDDEN));
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
                game(2, "Hades", "epic", true, 1_760_000_000L), game(3, "Doom", "steam", true, null),
                game(4, "Quake", "steam", true, 0L));
        HomeModel model = model();
        assertEquals("shelf:recent", model.rows.get(0).key);
        assertEquals("never played (0) is not recent", Arrays.asList("Hades", "Portal"), titles(model.row("shelf:recent").cards));
        assertFalse(model.row("shelf:recent").hasMore());
        assertEquals("recent:game:g2", model.row("shelf:recent").cards.get(0).key);
    }

    @Test
    public void aLibraryCardTakesTheNewestLastPlayedAcrossItsPcs() throws Exception {
        tower(Collections.emptyList(), game(1, "Portal", "steam", true, 1_700_000_000L), game(2, "Quake", "steam", true, 0L),
                game(3, "Doom", "steam", true, null));
        rig.transport.reply("den", "capabilities", ok("{\"api_version\":1,\"features\":[\"library\"],"
                + "\"permissions\":{\"list\":true,\"view\":true,\"launch\":true}}"));
        rig.transport.reply("den", "library", ok(library("d", game(1, "Portal", "steam", true, 1_760_000_000L),
                game(2, "Quake", "steam", true, null))));
        rig.transport.reply("den", "session", ok(NativeApiFixtures.IDLE));
        List<ComputerDetails> hosts = java.util.Arrays.asList(nativeHost("tower", "Tower", true), nativeHost("den", "Den", true));
        HomeModel model = HomeModel.of(rig.repository.refresh(hosts), new HashSet<>(java.util.Arrays.asList("tower", "den")));
        HomeModel.GameCard portal = null, quake = null, doom = null;
        for (HomeModel.GameCard card : model.allGames) {
            if (card.title.equals("Portal")) portal = card;
            if (card.title.equals("Quake")) quake = card;
            if (card.title.equals("Doom")) doom = card;
        }
        assertEquals(2, portal.hostCount);
        assertEquals("the newer of the two PCs", Long.valueOf(1_760_000_000L), portal.lastPlayed);
        assertEquals("one PC says never, the other nothing", Long.valueOf(0L), quake.lastPlayed);
        assertNull("no PC says", doom.lastPlayed);

        // Only when no PC says: when this device last streamed it, as on desktop.
        rig.recents.add(new com.limelight.nativeapi.RecentsStore.Recent("tower", "Tower", doom.appId, "Doom", 1_780_000_000_000L));
        rig.recents.add(new com.limelight.nativeapi.RecentsStore.Recent("tower", "Tower", portal.appId, "Portal", 1_790_000_000_000L));
        model = HomeModel.of(rig.repository.refresh(hosts), new HashSet<>(java.util.Arrays.asList("tower", "den")));
        for (HomeModel.GameCard card : model.allGames) {
            if (card.title.equals("Portal")) portal = card;
            if (card.title.equals("Doom")) doom = card;
        }
        assertEquals("this device's recents", Long.valueOf(1_780_000_000L), doom.lastPlayed);
        assertEquals("the PCs say, so they win", Long.valueOf(1_760_000_000L), portal.lastPlayed);
        assertEquals("the Recently played row stays the PCs' own", Arrays.asList("Portal"), titles(model.row("shelf:recent").cards));
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
        assertEquals(Arrays.asList("all=4", "installed=3", "games=3", "store:steam=2", "store:epic=1", "other=1", "unadded=1"),
                filters);
        List<String> steam = new ArrayList<>();
        for (HomeModel.GameCard card : model.allGames) {
            if (HomeModel.matches("store:steam", card)) steam.add(card.title);
        }
        assertEquals(Arrays.asList("Doom", "Portal"), steam);
        int installed = 0;
        for (HomeModel.GameCard card : model.allGames) if (HomeModel.matches("installed", card)) installed++;
        assertEquals(3, installed);
        assertNotNull(model.filters.get(3).storeName);
        // Games is every store's games, without Desktop.
        int games = 0;
        for (HomeModel.GameCard card : model.allGames) if (HomeModel.matches(HomeModel.FILTER_GAMES, card)) games++;
        assertEquals(3, games);
    }

    @Test
    public void everythingInstalledNeedsNoInstalledChip() throws Exception {
        tower(Collections.emptyList(), game(1, "Portal", "steam", true, null), game(2, "Hades", "epic", true, null));
        List<String> filters = new ArrayList<>();
        for (HomeModel.Filter filter : model().filters) filters.add(filter.key);
        assertEquals(Arrays.asList("all", "store:epic", "store:steam"), filters);
    }

    @Test
    public void ungroupedIsOneLibraryRowOfTheInstalledGames() throws Exception {
        tower(Collections.emptyList(), game(1, "Portal", "steam", true, null), game(2, "Hades", "epic", true, null),
                game(3, "Doom", "steam", false, null));
        HomeModel model = model();
        // Grouped stays the rows per store.
        assertEquals(model.rows, model.rows(HomeView.Group.STORE));
        List<HomeModel.Row> plain = model.rows(HomeView.Group.NONE);
        assertEquals(1, plain.size());
        assertEquals(HomeModel.LIBRARY_ROW, plain.get(0).key);
        assertEquals(titles(model.library), titles(plain.get(0).cards));
        assertFalse(titles(plain.get(0).cards).contains("Doom"));
    }

    @Test
    public void ungroupedWithNothingInstalledHasNoRow() throws Exception {
        tower(Collections.emptyList(), game(1, "Portal", "steam", false, null));
        assertTrue(model().rows(HomeView.Group.NONE).isEmpty());
    }

    @Test
    public void byTypeTheGamesOfEveryStoreAreOneRowAndTheAppsOfNoStoreTheirOwn() throws Exception {
        tower(Collections.emptyList(), game(1, "Desktop", null, true, null), game(2, "Portal", "steam", true, null),
                game(3, "Hades", "epic", true, null), game(4, "Playnite", null, true, null),
                game(5, "Doom", "steam", false, null));
        HomeModel model = model();
        List<HomeModel.Row> parts = model.rows(HomeView.Group.TYPE);
        assertEquals(2, parts.size());
        assertEquals("shelf:games", parts.get(0).key);
        assertEquals(HomeModel.RowKind.GAMES, parts.get(0).kind);
        // As in the store rows: the installed games by name, then the one not installed.
        assertEquals(Arrays.asList("Hades", "Portal", "Doom"), titles(parts.get(0).cards));
        // Each row is exactly what its All N tile's chip shows.
        assertEquals(HomeModel.FILTER_GAMES, parts.get(0).filter);
        for (HomeModel.Row part : parts) {
            assertEquals(part.key, new HashSet<>(titles(part.cards)),
                    new HashSet<>(titles(AllGamesActivity.cardsOf(model, part.filter))));
            HomeModel.Filter chip = AllGamesActivity.filterOf(model, part.filter);
            assertNotNull(part.key + " has its chip", chip);
            assertEquals(part.total, chip.count);
        }
        assertEquals("shelf:other", parts.get(1).key);
        assertEquals(HomeModel.RowKind.OTHER, parts.get(1).kind);
        assertEquals(HomeModel.FILTER_OTHER, parts.get(1).filter);
        assertEquals(Arrays.asList("Desktop", "Playnite"), titles(parts.get(1).cards));
    }

    @Test
    public void byTypeAPartPastTheCapIsWholeInTheGrid() throws Exception {
        List<String> games = new ArrayList<>();
        for (int i = 1; i <= 23; i++) games.add(game(i, String.format("Game %02d", i), i % 2 == 0 ? "steam" : "epic", true, null));
        games.add(game(30, "Desktop", null, true, null));
        tower(Collections.emptyList(), games.toArray(new String[0]));
        HomeModel model = model();
        HomeModel.Row part = model.rows(HomeView.Group.TYPE).get(0);
        assertTrue(part.hasMore());
        List<HomeModel.GameCard> whole = model.whole(part);
        assertEquals(23, whole.size());
        assertFalse("an app is not a game", titles(whole).contains("Desktop"));
    }

    @Test
    public void byTypeALibraryOfNoGamesFromStoresIsOneLibraryRow() throws Exception {
        tower(Collections.emptyList(), game(1, "Desktop", null, true, null), game(2, "Steam", null, true, null));
        List<HomeModel.Row> parts = model().rows(HomeView.Group.TYPE);
        assertEquals(1, parts.size());
        assertEquals(HomeModel.LIBRARY_ROW, parts.get(0).key);

        rig.close();
        rig = new ConsoleRig(temporary.newFolder());
        tower(Collections.emptyList(), game(1, "Portal", "steam", true, null));
        parts = model().rows(HomeView.Group.TYPE);
        assertEquals(Collections.singletonList("shelf:games"), Arrays.asList(parts.get(0).key));
        assertEquals(1, parts.size());
    }

    @Test
    public void aWholeRowHasEveryGamePastTheCap() throws Exception {
        List<String> games = new ArrayList<>();
        for (int i = 1; i <= 23; i++) games.add(game(i, String.format("Game %02d", i), "steam", true, null));
        games.add(game(30, "Hades", "epic", true, null));
        tower(Collections.emptyList(), games.toArray(new String[0]));
        HomeModel model = model();
        List<HomeModel.GameCard> steam = model.whole(model.row("shelf:store:steam"));
        assertEquals(23, steam.size());
        assertEquals("Game 23", steam.get(22).title);
        assertFalse(titles(steam).contains("Hades"));
        // A row that already holds all its games is itself.
        HomeModel.Row epic = model.row("shelf:store:epic");
        assertEquals(epic.cards, model.whole(epic));
    }

    private static HomeModel.GameCard titled(List<HomeModel.GameCard> cards, String title) {
        for (HomeModel.GameCard card : cards) {
            if (card.title.equals(title)) return card;
        }
        throw new AssertionError(title + " not in " + titles(cards));
    }

    /** The same game ("i1") under another app id, as a host lists it after renumbering. */
    private static String renumbered(int appId, String title) {
        return game(appId, title, "steam", true, null).replace("\"i" + appId + "\"", "\"i1\"").replace("\"g" + appId + "\"", "\"g1\"");
    }

    @Test
    public void aHiddenGameStaysHiddenWhenTheHostRenumbersItAndAnotherGetsItsOldId() throws Exception {
        tower(Collections.emptyList(), game(1, "Alpha", "steam", true, null), game(2, "Beta", "steam", true, null));
        HomeModel before = model();
        rig.repository.setHidden(before.copiesOf(titled(before.library, "Alpha")), true);

        rig.transport.reply("tower", "library", ok(library("t2", renumbered(7, "Alpha"),
                game(2, "Beta", "steam", true, null), game(1, "Newcomer", "steam", true, null).replace("\"i1\"", "\"i9\"").replace("\"g1\"", "\"g9\""))));
        HomeModel after = model();

        assertEquals(Arrays.asList("Beta", "Newcomer"), titles(after.library));
        List<HomeModel.GameCard> hidden = AllGamesActivity.cardsOf(after, HomeModel.FILTER_HIDDEN);
        assertEquals(Collections.singletonList("Alpha"), titles(hidden));
        assertEquals(7, hidden.get(0).appId);

        // Shown again under its new id, it stays shown through the next renumber.
        rig.repository.setHidden(after.copiesOf(hidden.get(0)), false);
        rig.transport.reply("tower", "library", ok(library("t3", renumbered(8, "Alpha"), game(2, "Beta", "steam", true, null))));
        assertEquals(Arrays.asList("Alpha", "Beta"), titles(model().library));
    }

    @Test
    public void aCollectionRowsCardStillKnowsItsLibraryCardForWhichPc() throws Exception {
        tower(Collections.emptyList(), in(game(1, "Alpha", "steam", true, null), "VR"));
        CollectionGroups.use(request -> "{\"ok\":{\"groups\":[{\"name\":\"VR\",\"games\":[0]}]}}");
        HomeModel model = model();
        HomeModel.GameCard card = model.rows(HomeView.Group.COLLECTION).get(0).cards.get(0);
        assertFalse(card.key.startsWith("game:"));
        assertEquals(titled(model.library, "Alpha").key, card.libraryKey);
    }

    @Test
    public void aCollectionsChipHoldsTheCardsItsGroupPlacesOnItNotAComparisonOfNames() throws Exception {
        // The shared code groups these two; Java's trim and ignore-case would not.
        tower(Collections.emptyList(),
                in(game(1, "Alpha", "steam", true, null), "Straße"),
                in(game(2, "Beta", "steam", true, null), "STRASSE"),
                game(3, "Gamma", "steam", true, null));
        CollectionGroups.use(request -> "{\"ok\":{\"groups\":[{\"name\":\"Straße\",\"games\":[0,1]},"
                + "{\"name\":null,\"games\":[2]}]}}");
        HomeModel model = model();
        assertEquals(Arrays.asList("Alpha", "Beta"),
                titles(AllGamesActivity.cardsOf(model, HomeModel.COLLECTION_FILTER + "Straße")));
        assertEquals(Collections.singletonList("Gamma"),
                titles(AllGamesActivity.cardsOf(model, HomeModel.FILTER_NO_COLLECTION)));
        assertEquals(Collections.emptyList(), AllGamesActivity.cardsOf(model, HomeModel.COLLECTION_FILTER + "Gone"));
    }

    @Test
    public void aRunningHiddenGameStaysUnderContinuePlayingAndItsCardShowsItAgain() throws Exception {
        tower(Collections.emptyList(), game(1, "Alpha", "steam", true, null), game(2, "Beta", "steam", true, null));
        HomeModel before = model();
        rig.repository.setHidden(before.copiesOf(titled(before.library, "Alpha")), true);
        rig.transport.reply("tower", "session", ok(NativeApiFixtures.session("streaming", "other_device",
                "{\"install_id\":\"i1\",\"app_id\":1,\"title\":\"Alpha\"}", "[]")));

        HomeModel model = model();
        HomeModel.GameCard running = titled(model.continuePlaying, "Alpha");
        assertTrue(running.sourceHidden());
        assertEquals(Collections.singletonList("Beta"), titles(model.library));

        // Its menu offers Show: the copies are the hidden ones, and that brings it back.
        assertEquals(1, model.copiesOf(running).size());
        assertTrue(model.copiesOf(running).get(0).hidden);
        rig.repository.setHidden(model.copiesOf(running), false);
        HomeModel shown = model();
        assertFalse(titled(shown.continuePlaying, "Alpha").sourceHidden());
        assertEquals(Arrays.asList("Alpha", "Beta"), titles(shown.library));
    }

    @Test
    public void aHiddenGameTakesNoPlaceAmongTheRecentOnes() throws Exception {
        List<String> games = new ArrayList<>();
        for (int id = 1; id <= 11; id++) {
            games.add(game(id, "Game " + id, "steam", true, null));
            rig.recents.add(new com.limelight.nativeapi.RecentsStore.Recent("tower", "Tower", id, "Game " + id, 1000 - id));
        }
        tower(Collections.emptyList(), games.toArray(new String[0]));
        HomeModel before = model();
        rig.repository.setHidden(before.copiesOf(titled(before.library, "Game 1")), true);

        List<String> recent = titles(model().continuePlaying);
        assertEquals(recent.toString(), 10, recent.size());
        assertFalse(recent.contains("Game 1"));
        assertTrue("the place goes to the next one", recent.contains("Game 11"));
    }
}
