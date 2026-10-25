package com.limelight.console;

import static com.limelight.console.HomeLibraryRowsTest.found;
import static com.limelight.console.HomeLibraryRowsTest.game;
import static com.limelight.nativeapi.NativeApiFixtures.library;
import static com.limelight.nativeapi.NativeApiFixtures.nativeHost;
import static com.limelight.nativeapi.NativeApiFixtures.ok;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

import android.app.Application;
import android.content.Context;
import android.view.View;
import android.view.ContextThemeWrapper;
import android.widget.FrameLayout;

import com.limelight.R;
import com.limelight.nativeapi.NativeApiFixtures;
import com.limelight.nativeapi.NativeLibrary;
import com.limelight.nvstream.http.ComputerDetails;

import org.junit.After;
import org.junit.Before;
import org.junit.Rule;
import org.junit.Test;
import org.junit.rules.TemporaryFolder;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;

import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collections;
import java.util.HashSet;
import java.util.List;

/** The TV library's cards: chips, the grey of a game not installed, the "All N" tile and the All games filters. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class, qualifiers = "w960dp-h540dp-television-xhdpi")
public class LibraryCardsTest {
    @Rule public TemporaryFolder temporary = new TemporaryFolder();

    private ConsoleRig rig;
    private Context context;
    private final List<String> opened = new ArrayList<>();
    private final List<String> games = new ArrayList<>();

    @Before
    public void setUp() throws Exception {
        rig = new ConsoleRig(temporary.newFolder());
        context = new ContextThemeWrapper(RuntimeEnvironment.getApplication(), R.style.AppTheme);
    }

    @After
    public void tearDown() {
        rig.close();
    }

    private void tower(List<String> features, String... list) {
        StringBuilder names = new StringBuilder("[\"library\"");
        for (String feature : features) names.append(",\"").append(feature).append('"');
        rig.transport.reply("tower", "capabilities", ok("{\"api_version\":1,\"features\":" + names + "],"
                + "\"permissions\":{\"list\":true,\"view\":true,\"launch\":true}}"));
        rig.transport.reply("tower", "library", ok(library("t", list)));
        rig.transport.reply("tower", "session", ok(NativeApiFixtures.IDLE));
    }

    private HomeModel model() throws Exception {
        return HomeModel.of(rig.repository.refresh(Collections.singletonList(nativeHost("tower", "Tower", true))),
                new HashSet<>(Collections.singletonList("tower")));
    }

    private ConsoleHomeAdapter adapter(HomeModel model) {
        ConsoleHomeAdapter adapter = new ConsoleHomeAdapter(context, null, uuid -> null, new ConsoleHomeAdapter.Listener() {
            @Override public void onGame(HomeModel.GameCard card) { games.add(card.title); }
            @Override public void onGameMenu(HomeModel.GameCard card) { }
            @Override public void onGameToggleFavourite(HomeModel.GameCard card) { }
            @Override public void onHost(HomeModel.HostCard card) { }
            @Override public void onHostOptions(HomeModel.HostCard card) { }
            @Override public void onAllGames(String filter) { opened.add(filter); }
        });
        adapter.setTelevision(true);
        adapter.submit(model);
        return adapter;
    }

    private ShelfAdapter.CardHolder card(ConsoleHomeAdapter adapter, String row, int card) {
        int at = -1;
        for (int i = 0; i < adapter.rowCount(); i++) {
            if (row.equals(adapter.rowKey(i))) at = i;
        }
        assertTrue(row + " is a shelf", at >= 0);
        FrameLayout parent = new FrameLayout(context);
        ConsoleHomeAdapter.ShelfHolder shelf = adapter.onCreateViewHolder(parent, 0);
        adapter.onBindViewHolder(shelf, at);
        ShelfAdapter.CardHolder holder = shelf.adapter.onCreateViewHolder(parent, shelf.adapter.getItemViewType(card));
        shelf.adapter.onBindViewHolder(holder, card);
        return holder;
    }

    private static int count(ConsoleHomeAdapter adapter, String row) {
        for (int i = 0; i < adapter.rowCount(); i++) {
            if (row.equals(adapter.rowKey(i))) return adapter.cardCount(i);
        }
        return -1;
    }

    @Test
    public void aGameNotInstalledIsGreyFromTheBindAndSaysSoInItsChip() throws Exception {
        tower(Collections.emptyList(), game(1, "Portal", "steam", false, null), game(2, "Doom", "steam", true, null));
        ConsoleHomeAdapter adapter = adapter(model());
        ShelfAdapter.CardHolder doom = card(adapter, "shelf:store:steam", 0);
        ShelfAdapter.CardHolder portal = card(adapter, "shelf:store:steam", 1);
        assertNull("an installed poster keeps its colours", doom.image.getColorFilter());
        assertNotNull("a poster not installed is grey", portal.image.getColorFilter());
        assertEquals(View.VISIBLE, portal.store.getVisibility());
        assertEquals(context.getString(R.string.console_chip_not_installed), portal.store.getText().toString());
        // Steam has a glyph: an installed game shows it alone.
        assertEquals(View.VISIBLE, doom.store.getVisibility());
        assertEquals("", doom.store.getText().toString());
        assertNotNull(doom.store.getCompoundDrawablesRelative()[0]);
        String description = portal.itemView.getContentDescription().toString();
        assertTrue(description, description.startsWith("Portal, Steam, "));
        assertTrue(description, description.contains(context.getString(R.string.console_chip_not_installed)));
        assertTrue(description, description.contains(context.getString(R.string.console_card_on_host, "Tower")));
        // A card bound again for an installed game loses the grey.
        assertNull(card(adapter, "shelf:store:steam", 0).image.getColorFilter());
    }

    @Test
    public void aStoreWithoutAGlyphShowsItsNameAndAnAppOfNoStoreShowsNoChip() throws Exception {
        tower(Collections.emptyList(),
                "{\"install_id\":\"i1\",\"app_id\":1,\"title\":\"Tetris\",\"game_id\":\"g1\","
                        + "\"store\":{\"source\":\"itch\",\"name\":\"itch.io\"},\"installed\":true,\"running\":false}",
                game(2, "Desktop", null, true, null));
        ConsoleHomeAdapter adapter = adapter(model());
        ShelfAdapter.CardHolder tetris = card(adapter, "shelf:store:itch", 0);
        assertEquals("itch.io", tetris.store.getText().toString());
        assertNull(tetris.store.getCompoundDrawablesRelative()[0]);
        assertEquals(View.GONE, card(adapter, "shelf:other", 0).store.getVisibility());
    }

    @Test
    public void aRunningGameHasTheLiveDotAndAStarredOneItsStar() throws Exception {
        tower(Collections.emptyList(), NativeApiFixtures.game("t-1", 1, "Portal", "steam:1", true));
        rig.repository.setFavourite("tower", "Tower", "t-1", 1, "Portal", true);
        ConsoleHomeAdapter adapter = adapter(model());
        ShelfAdapter.CardHolder portal = card(adapter, "shelf:store:steam", 0);
        assertEquals(View.VISIBLE, portal.badge.getVisibility());
        assertEquals(context.getString(R.string.console_badge_running), portal.badge.getText().toString());
        assertNotNull("Running has the green dot", portal.badge.getCompoundDrawablesRelative()[0]);
        assertEquals(View.VISIBLE, portal.star.getVisibility());
    }

    @Test
    public void aRowPastTwentyEndsInAnAllTileThatOpensAllGamesUnderItsFilter() throws Exception {
        String[] list = new String[23];
        for (int i = 0; i < list.length; i++) list[i] = game(i + 1, String.format("Game %02d", i), "steam", true, null);
        tower(Collections.emptyList(), list);
        ConsoleHomeAdapter adapter = adapter(model());
        assertEquals("twenty posters and the tile", HomeModel.ROW_CAP + 1, count(adapter, "shelf:store:steam"));
        ShelfAdapter.CardHolder tile = card(adapter, "shelf:store:steam", HomeModel.ROW_CAP);
        assertTrue(tile.tile);
        assertEquals("23", tile.allCount.getText().toString());
        assertEquals("Steam", tile.allName.getText().toString());
        assertEquals(View.VISIBLE, tile.allGlyph.getVisibility());
        assertEquals(context.getResources().getQuantityString(R.plurals.console_all_tile_description, 23, "Steam", 23),
                tile.itemView.getContentDescription().toString());
        tile.itemView.performClick();
        assertEquals(Collections.singletonList("store:steam"), opened);
    }

    @Test
    public void aRowOfTwentyHasNoTile() throws Exception {
        String[] list = new String[20];
        for (int i = 0; i < list.length; i++) list[i] = game(i + 1, "Game " + i, "steam", true, null);
        tower(Collections.emptyList(), list);
        assertEquals(20, count(adapter(model()), "shelf:store:steam"));
    }

    @Test
    public void aGameNotAddedSaysWhatAddingTakes() throws Exception {
        tower(Collections.singletonList("library.unadded"), game(1, "Portal", "steam", true, null), found("Hades", "epic"));
        HomeModel model = model();
        ShelfAdapter.CardHolder hades = card(adapter(model), "shelf:unadded", 0);
        assertFalse(model.row("shelf:unadded").cards.get(0).addable);
        assertEquals(context.getString(R.string.console_add_on_pc), hades.store.getText().toString());
        assertEquals("Hades", hades.imageText.getText().toString());
        assertTrue(hades.itemView.getContentDescription().toString()
                .contains(context.getString(R.string.console_add_on_pc)));
    }

    @Test
    public void aGameNotAddedOnAHostThatAddsSaysNotAdded() throws Exception {
        tower(Arrays.asList("library.unadded", "library.add"), game(1, "Portal", "steam", true, null), found("Hades", "epic"));
        HomeModel model = model();
        assertTrue(model.row("shelf:unadded").cards.get(0).addable);
        assertEquals(context.getString(R.string.console_chip_not_added),
                card(adapter(model), "shelf:unadded", 0).store.getText().toString());
    }

    @Test
    public void addingAGameAsksTheHostAndReturnsItWithItsAppId() throws Exception {
        tower(Arrays.asList("library.unadded", "library.add"), found("Hades", "epic"));
        model();
        rig.transport.reply("tower", "library_add", ok(library("t2",
                "{\"install_id\":\"i9\",\"app_id\":9,\"title\":\"Hades\",\"game_id\":\"found:Hades\","
                        + "\"store\":{\"source\":\"epic\"},\"installed\":true,\"added\":true,\"running\":false}")));
        ComputerDetails tower = nativeHost("tower", "Tower", true);
        NativeLibrary.Game added = rig.repository.addToLibrary(tower, "found:Hades");
        assertNotNull(added);
        assertEquals(9, added.appId);
        assertEquals("tower|library_add", rig.transport.calls.get(rig.transport.calls.size() - 1));
        assertTrue(rig.transport.arguments.get(rig.transport.arguments.size() - 1).contains("found:Hades"));
        // The answer is the library now: what is saved shows the game added, before any refresh.
        HomeModel saved = HomeModel.of(rig.repository.cached(Collections.singletonList(tower)),
                new HashSet<>(Collections.singletonList("tower")));
        assertNull(saved.row("shelf:unadded"));
        assertEquals(9, saved.row("shelf:store:epic").cards.get(0).appId);
    }

    @Test
    public void allGamesHasAChipPerSourceAndEachShowsItsGames() throws Exception {
        tower(Collections.singletonList("library.unadded"),
                game(1, "Portal", "steam", true, null), game(2, "Doom", "steam", false, null),
                game(3, "Hades", "epic", true, null), game(4, "Desktop", null, true, null), found("Celeste", "gog"));
        HomeModel model = model();
        List<String> chips = new ArrayList<>();
        for (HomeModel.Filter f : model.filters) chips.add(f.key + "=" + f.count);
        assertEquals(Arrays.asList("all=4", "installed=3", "store:steam=2", "store:epic=1", "other=1", "unadded=1"), chips);
        assertEquals(Arrays.asList("Doom", "Portal"), titles(AllGamesActivity.cardsOf(model, "store:steam")));
        assertEquals(Arrays.asList("Desktop", "Hades", "Portal"), titles(AllGamesActivity.cardsOf(model, "installed")));
        assertEquals(Arrays.asList("Celeste"), titles(AllGamesActivity.cardsOf(model, "unadded")));
        assertEquals("Steam", AllGamesActivity.labelOf(context, AllGamesActivity.filterOf(model, "store:steam")).toString());
        assertEquals(context.getString(R.string.console_row_other),
                AllGamesActivity.labelOf(context, AllGamesActivity.filterOf(model, "other")).toString());
        assertNull(AllGamesActivity.filterOf(model, "store:gog"));
    }

    @Test
    public void allGamesFitsSixPostersAcrossATvAndFewerOnANarrowScreen() {
        assertEquals(6, AllGamesActivity.columnsFor(864, 144));
        assertEquals(6, AllGamesActivity.columnsFor(1800, 144));
        assertEquals(2, AllGamesActivity.columnsFor(300, 144));
        assertEquals(4, AllGamesActivity.columnsFor(600, 144));
    }

    private static List<String> titles(List<HomeModel.GameCard> cards) {
        List<String> titles = new ArrayList<>();
        for (HomeModel.GameCard card : cards) titles.add(card.title);
        return titles;
    }
}
