package com.limelight.console;

import static com.limelight.nativeapi.NativeApiFixtures.game;
import static com.limelight.nativeapi.NativeApiFixtures.library;
import static com.limelight.nativeapi.NativeApiFixtures.nativeHost;
import static com.limelight.nativeapi.NativeApiFixtures.ok;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

import android.app.Application;
import android.view.ContextThemeWrapper;
import android.widget.FrameLayout;

import androidx.recyclerview.widget.RecyclerView;

import com.limelight.R;
import com.limelight.nativeapi.NativeApiFixtures;

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
import java.util.Collections;
import java.util.HashSet;
import java.util.List;

/** The Favourites row of Home and the menu on a long press (a held D-pad centre is a long press too). */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class, qualifiers = "w960dp-h540dp-television-xhdpi")
public class FavouritesShelvesTest {
    @Rule public TemporaryFolder temporary = new TemporaryFolder();

    private ConsoleRig rig;
    private ContextThemeWrapper context;

    @Before
    public void setUp() throws Exception {
        rig = new ConsoleRig(temporary.newFolder());
        context = new ContextThemeWrapper(RuntimeEnvironment.getApplication(), R.style.AppTheme);
        rig.transport.reply("tower", "library", ok(library("t1",
                game("t-portal", 11, "Portal", "steam:400", false), game("t-zelda", 12, "Zelda", null, false))));
        rig.transport.reply("tower", "session", ok(NativeApiFixtures.IDLE));
    }

    @After
    public void tearDown() {
        rig.close();
    }

    private HomeModel model() throws Exception {
        return HomeModel.of(rig.repository.refresh(Collections.singletonList(nativeHost("tower", "Tower", true))),
                new HashSet<>(Collections.singletonList("tower")));
    }

    /** The card at {@code card} on shelf {@code row}, made and bound the way the shelves do. */
    private ShelfAdapter.CardHolder card(ConsoleHomeAdapter adapter, int row, int card) {
        FrameLayout parent = new FrameLayout(context);
        ConsoleHomeAdapter.ShelfHolder shelf = adapter.onCreateViewHolder(parent, 0);
        adapter.onBindViewHolder(shelf, row);
        ShelfAdapter.CardHolder holder = shelf.adapter.onCreateViewHolder(parent, shelf.adapter.getItemViewType(card));
        shelf.adapter.onBindViewHolder(holder, card);
        return holder;
    }

    @Test
    public void theShelvesRunContinuePlayingThenFavouritesThenTheLibraryThenThePcs() throws Exception {
        rig.recents.add(new com.limelight.nativeapi.RecentsStore.Recent("tower", "Tower", 11, "Portal", 10));
        rig.repository.setFavourite("tower", "Tower", 12, "Zelda", true);
        List<RecordedMenu> menus = new ArrayList<>();
        ConsoleHomeAdapter adapter = new ConsoleHomeAdapter(context, null, uuid -> null, recorder(menus));
        adapter.submit(model());

        assertEquals(4, adapter.rowCount());
        assertEquals("shelf:continue", adapter.rowKey(0));
        assertEquals("shelf:favourites", adapter.rowKey(1));
        assertEquals("shelf:library", adapter.rowKey(2));
        assertEquals("shelf:hosts", adapter.rowKey(3));
        FrameLayout parent = new FrameLayout(context);
        ConsoleHomeAdapter.ShelfHolder second = adapter.onCreateViewHolder(parent, 0);
        adapter.onBindViewHolder(second, 1);
        assertEquals(context.getString(R.string.console_section_favourites), second.title.getText().toString());
        assertEquals(new ConsoleHomeAdapter.Cursor(0, 0), adapter.firstCard());
    }

    @Test
    public void aShelfWithNoCardsIsLeftOutAndAGameIsAlwaysACover() throws Exception {
        ConsoleHomeAdapter adapter = new ConsoleHomeAdapter(context, null, uuid -> null, recorder(new ArrayList<>()));
        adapter.submit(model());
        // No recents and no stars: the library and the PCs.
        assertEquals(2, adapter.rowCount());
        assertEquals("shelf:library", adapter.rowKey(0));
        assertTrue(card(adapter, 0, 0).cover);
        assertFalse(card(adapter, 1, 0).cover);
    }

    @Test
    public void aLongPressOnAGameCardOpensItsMenuAndAHostCardOpensTheHostOptions() throws Exception {
        rig.repository.setFavourite("tower", "Tower", 12, "Zelda", true);
        List<RecordedMenu> menus = new ArrayList<>();
        ConsoleHomeAdapter adapter = new ConsoleHomeAdapter(context, null, uuid -> null, recorder(menus));
        HomeModel model = model();
        adapter.submit(model);
        FrameLayout parent = new FrameLayout(context);

        RecyclerView.ViewHolder card = card(adapter, 0, 0);
        assertTrue(card.itemView.isFocusable());
        assertTrue(card.itemView.performLongClick());
        assertEquals(1, menus.size());
        assertEquals("Zelda", menus.get(0).title);
        assertTrue(menus.get(0).favourite);
        assertTrue(card.itemView.getContentDescription().toString()
                .endsWith(context.getString(R.string.console_favourite_description)));

        // A host card has no game menu; its long press opens the PC's options instead.
        RecyclerView.ViewHolder host = card(adapter, adapter.rowCount() - 1, 0);
        assertTrue(host.itemView.performLongClick());
        assertEquals(1, menus.size());
        assertEquals(1, hostOptions.size());
    }

    @Test
    public void xOnAGameCardOpensItsMenuLikeALongPress() throws Exception {
        rig.repository.setFavourite("tower", "Tower", 12, "Zelda", true);
        List<RecordedMenu> menus = new ArrayList<>();
        ConsoleHomeAdapter adapter = new ConsoleHomeAdapter(context, null, uuid -> null, recorder(menus));
        adapter.submit(model());
        RecyclerView.ViewHolder card = card(adapter, 0, 0);
        focus(new FrameLayout(context), card.itemView);

        card.itemView.dispatchKeyEvent(new android.view.KeyEvent(android.view.KeyEvent.ACTION_DOWN,
                android.view.KeyEvent.KEYCODE_BUTTON_X));
        assertTrue("the menu waits for the key to come up", menus.isEmpty());
        card.itemView.dispatchKeyEvent(new android.view.KeyEvent(android.view.KeyEvent.ACTION_UP,
                android.view.KeyEvent.KEYCODE_BUTTON_X));
        assertEquals(1, menus.size());
        assertEquals("Zelda", menus.get(0).title);
        assertTrue("and stars nothing", toggles.isEmpty());
    }

    @Test
    public void menuOnAGameCardOpensItsOptionsAndStarsNothing() throws Exception {
        rig.repository.setFavourite("tower", "Tower", 12, "Zelda", true);
        List<RecordedMenu> menus = new ArrayList<>();
        ConsoleHomeAdapter adapter = new ConsoleHomeAdapter(context, null, uuid -> null, recorder(menus));
        adapter.submit(model());
        RecyclerView.ViewHolder card = card(adapter, 0, 0);
        focus(new FrameLayout(context), card.itemView);

        card.itemView.dispatchKeyEvent(new android.view.KeyEvent(android.view.KeyEvent.ACTION_DOWN,
                android.view.KeyEvent.KEYCODE_MENU));
        card.itemView.dispatchKeyEvent(new android.view.KeyEvent(android.view.KeyEvent.ACTION_UP,
                android.view.KeyEvent.KEYCODE_MENU));
        assertEquals("the options open, as on a PC card and in the library", 1, menus.size());
        assertEquals("Zelda", menus.get(0).title);
        assertTrue("favourite stays in the options", toggles.isEmpty());
    }

    @Test
    public void aKeyThatWasCancelledOpensNoMenu() throws Exception {
        rig.repository.setFavourite("tower", "Tower", 12, "Zelda", true);
        List<RecordedMenu> menus = new ArrayList<>();
        ConsoleHomeAdapter adapter = new ConsoleHomeAdapter(context, null, uuid -> null, recorder(menus));
        adapter.submit(model());
        RecyclerView.ViewHolder card = card(adapter, 0, 0);
        focus(new FrameLayout(context), card.itemView);

        for (int key : new int[]{android.view.KeyEvent.KEYCODE_BUTTON_X, android.view.KeyEvent.KEYCODE_MENU}) {
            card.itemView.dispatchKeyEvent(new android.view.KeyEvent(android.view.KeyEvent.ACTION_DOWN, key));
            // The window took the key away (a dialog came up, focus moved): the up arrives flagged as cancelled.
            card.itemView.dispatchKeyEvent(new android.view.KeyEvent(0, 0, android.view.KeyEvent.ACTION_UP, key, 0, 0, 0, 0,
                    android.view.KeyEvent.FLAG_CANCELED));
        }
        assertTrue(menus.isEmpty());

        RecyclerView.ViewHolder host = card(adapter, adapter.rowCount() - 1, 0);
        focus(new FrameLayout(context), host.itemView);
        host.itemView.dispatchKeyEvent(new android.view.KeyEvent(android.view.KeyEvent.ACTION_DOWN,
                android.view.KeyEvent.KEYCODE_MENU));
        host.itemView.dispatchKeyEvent(new android.view.KeyEvent(0, 0, android.view.KeyEvent.ACTION_UP,
                android.view.KeyEvent.KEYCODE_MENU, 0, 0, 0, 0, android.view.KeyEvent.FLAG_CANCELED));
        assertTrue("nor does a PC card", hostOptions.isEmpty());
    }

    @Test
    public void theHintNamesTheKeysOfTheLastDeviceAndSaysOpenForAGame() {
        String remote = HomeAmbience.hints(context, false, true);
        assertEquals(context.getString(R.string.console_hint_line, context.getString(R.string.console_hint_key_ok),
                context.getString(R.string.console_hint_open), context.getString(R.string.console_hint_key_menu),
                context.getString(R.string.console_hint_options)), remote);
        assertTrue(remote, remote.contains("OK") && remote.contains("Open") && remote.contains("Menu"));
        assertFalse("a game's A opens its page; it does not play", remote.contains("Play"));

        String pad = HomeAmbience.hints(context, true, true);
        assertTrue(pad, pad.startsWith("A  Open") && pad.contains("X  "));
        assertFalse(pad, pad.contains("OK") || pad.contains("Menu"));

        String host = HomeAmbience.hints(context, true, false);
        assertTrue("a PC opens its library", host.contains(context.getString(R.string.library_open)));
    }

    @Test
    public void theHintFollowsTheKeyThatWasPressedLast() throws Exception {
        android.widget.LinearLayout hero = new android.widget.LinearLayout(context);
        android.widget.TextView hints = new android.widget.TextView(context);
        hints.setId(R.id.home_hero_hints);
        hero.addView(hints);
        HomeAmbience ambience = new HomeAmbience(context, null, hero, uuid -> null, () -> null, () -> true);
        ConsoleHomeAdapter adapter = new ConsoleHomeAdapter(context, null, uuid -> null, recorder(new ArrayList<>()));
        adapter.submit(model());
        ambience.refresh(adapter.itemAt(new ConsoleHomeAdapter.Cursor(0, 0)));
        org.robolectric.shadows.ShadowLooper.idleMainLooper(100, java.util.concurrent.TimeUnit.MILLISECONDS);
        assertTrue(hints.getText().toString().contains("OK"));

        ambience.setGamepad(ConsoleHomeActivity.fromGamepad(new android.view.KeyEvent(0, 0, android.view.KeyEvent.ACTION_DOWN,
                android.view.KeyEvent.KEYCODE_BUTTON_A, 0, 0, 0, 0, 0, android.view.InputDevice.SOURCE_GAMEPAD)));
        assertTrue(hints.getText().toString(), hints.getText().toString().startsWith("A  "));
        ambience.setGamepad(ConsoleHomeActivity.fromGamepad(new android.view.KeyEvent(android.view.KeyEvent.ACTION_DOWN,
                android.view.KeyEvent.KEYCODE_DPAD_CENTER)));
        assertTrue(hints.getText().toString().contains("OK"));
    }

    private static final class RecordedMenu {
        final String title;
        final boolean favourite;

        RecordedMenu(String title, boolean favourite) {
            this.title = title;
            this.favourite = favourite;
        }
    }

    private ConsoleHomeAdapter.Listener recorder(List<RecordedMenu> menus) {
        return new ConsoleHomeAdapter.Listener() {
            @Override public void onGame(HomeModel.GameCard card) {
            }

            @Override public void onGameMenu(HomeModel.GameCard card) {
                menus.add(new RecordedMenu(card.title, card.favourite));
            }

            @Override public void onGameToggleFavourite(HomeModel.GameCard card) {
                toggles.add(card.title);
            }

            @Override public void onHost(HomeModel.HostCard card) {
            }

            @Override public void onHostOptions(HomeModel.HostCard card) {
                hostOptions.add(card.uuid);
            }
        };
    }

    private final List<String> toggles = new ArrayList<>();
    private final List<String> hostOptions = new ArrayList<>();

    /** A view group hands key events to its own listener only while it has focus, so give the card its focus. */
    private static void focus(FrameLayout parent, android.view.View card) {
        parent.addView(card);
        parent.measure(android.view.View.MeasureSpec.makeMeasureSpec(600, android.view.View.MeasureSpec.EXACTLY),
                android.view.View.MeasureSpec.makeMeasureSpec(600, android.view.View.MeasureSpec.EXACTLY));
        parent.layout(0, 0, 600, 600);
        card.setFocusable(true);
        assertTrue(card.requestFocus());
    }

    /** A held key moves focus card to card with no tween: the new card is at its focus scale and the old at rest at once. */
    @Test
    public void aHeldKeyPutsTheNewHomeCardStraightIntoFocusAndTheOldOneAtRest() throws Exception {
        ConsoleHomeAdapter adapter = new ConsoleHomeAdapter(context, null, uuid -> null, recorder(new ArrayList<>()));
        adapter.submit(model());
        assertTrue(adapter.cardCount(0) >= 2);
        android.widget.LinearLayout row = new android.widget.LinearLayout(context);
        RecyclerView.ViewHolder first = card(adapter, 0, 0);
        RecyclerView.ViewHolder second = card(adapter, 0, 1);
        first.itemView.setFocusable(true);
        second.itemView.setFocusable(true);
        row.addView(first.itemView, new android.widget.LinearLayout.LayoutParams(300, 400));
        row.addView(second.itemView, new android.widget.LinearLayout.LayoutParams(300, 400));
        row.measure(android.view.View.MeasureSpec.makeMeasureSpec(1200, android.view.View.MeasureSpec.EXACTLY),
                android.view.View.MeasureSpec.makeMeasureSpec(800, android.view.View.MeasureSpec.EXACTLY));
        row.layout(0, 0, 1200, 800);

        // A single press tweens: the card has not reached its scale when focus arrives.
        assertTrue(first.itemView.requestFocus());
        assertTrue("a press animates", first.itemView.getScaleX() < 1.08f);
        first.itemView.getStateListAnimator().jumpToCurrentState();
        assertEquals(1.08f, first.itemView.getScaleX(), 0.001f);

        // A held key: the key listener sees the repeat, then focus moves to the next card.
        first.itemView.dispatchKeyEvent(new android.view.KeyEvent(0, 0, android.view.KeyEvent.ACTION_DOWN,
                android.view.KeyEvent.KEYCODE_DPAD_RIGHT, 1));
        assertTrue(second.itemView.requestFocus());
        assertEquals("the new card is focused at once", 1.08f, second.itemView.getScaleX(), 0.001f);
        assertEquals("the old card is at rest at once", 1f, first.itemView.getScaleX(), 0.001f);
    }

    @Test
    public void initialFocusGoesToTheRunningGameWhileFavouritesStayFirst() throws Exception {
        rig.transport.reply("tower", "session", ok(NativeApiFixtures.session("streaming", "you",
                "{\"install_id\":\"t-portal\",\"app_id\":11,\"title\":\"Portal\"}", "[]")));
        rig.transport.reply("tower", "library", ok(library("t1",
                game("t-portal", 11, "Portal", "steam:400", true), game("t-zelda", 12, "Zelda", null, false))));
        rig.repository.setFavourite("tower", "Tower", "t-zelda", 12, "Zelda", true);
        ConsoleHomeAdapter adapter = new ConsoleHomeAdapter(context, null, uuid -> null, recorder(new ArrayList<>()));
        adapter.submit(model());

        // Continue playing first, with the running Portal; the favourites after it.
        assertEquals("shelf:continue", adapter.rowKey(0));
        assertEquals(new ConsoleHomeAdapter.Cursor(0, 0), adapter.firstCard());
        assertEquals("favourite:tower:t-zelda", adapter.keyAt(new ConsoleHomeAdapter.Cursor(1, 0)));
        ConsoleHomeAdapter.Cursor focus = adapter.initialFocus();
        assertEquals(0, focus.row);
        assertTrue(adapter.keyAt(focus).startsWith("continue:tower:11"));
    }

    @Test
    public void aRunningGameInTheLibraryTakesFocusWhenNothingIsInContinuePlaying() throws Exception {
        // The shelf of games to continue is empty (no recents) but a game runs: the library's copy has the focus.
        rig.transport.reply("tower", "session", ok(NativeApiFixtures.session("streaming", "you",
                "{\"install_id\":\"t-zelda\",\"app_id\":12,\"title\":\"Zelda\"}", "[]")));
        rig.transport.reply("tower", "library", ok(library("t1",
                game("t-portal", 11, "Portal", "steam:400", false), game("t-zelda", 12, "Zelda", null, true))));
        ConsoleHomeAdapter adapter = new ConsoleHomeAdapter(context, null, uuid -> null, recorder(new ArrayList<>()));
        HomeModel model = model();
        adapter.submit(model);
        ConsoleHomeAdapter.Cursor focus = adapter.initialFocus();
        assertNotNull(focus);
        assertEquals("Zelda", adapter.itemAt(focus).game.title);
    }

    @Test
    public void aCardIsFoundAgainByItsStableIdAndAMissingOneHasANeighbour() throws Exception {
        rig.repository.setFavourite("tower", "Tower", 12, "Zelda", true);
        ConsoleHomeAdapter adapter = new ConsoleHomeAdapter(context, null, uuid -> null, recorder(new ArrayList<>()));
        adapter.submit(model());
        ConsoleHomeAdapter.Cursor library = adapter.positionOfKey("game:host:tower:12");
        assertNotNull(library);
        assertEquals("shelf:library", adapter.rowKey(library.row));
        assertEquals("game:host:tower:12", adapter.keyAt(library));
        assertNull(adapter.positionOfKey("favourite:tower:gone"));
        assertNull(adapter.keyAt(new ConsoleHomeAdapter.Cursor(0, 99)));
        // Where a removed card was, the nearest card of its shelf takes over; past the last shelf, the last.
        ConsoleHomeAdapter.Cursor near = adapter.nearestCard(new ConsoleHomeAdapter.Cursor(0, 99));
        assertEquals(new ConsoleHomeAdapter.Cursor(0, adapter.cardCount(0) - 1), near);
        assertNotNull(adapter.keyAt(adapter.nearestCard(new ConsoleHomeAdapter.Cursor(9, 0))));
    }

    @Test
    public void aHostThatRenumbersItsAppsKeepsTheStarOnTheSameGame() throws Exception {
        rig.repository.setFavourite("tower", "Tower", "t-zelda", 12, "Zelda", true);
        assertEquals(java.util.Collections.singletonList("Zelda"), titles(model().favourites));

        // The host now numbers Zelda 40, and another title took the id 12.
        rig.transport.reply("tower", "library", ok(library("t2",
                game("t-portal", 11, "Portal", "steam:400", false), game("t-zelda", 40, "Zelda", null, false),
                game("t-other", 12, "Other", null, false))));
        HomeModel renumbered = model();
        assertEquals(java.util.Collections.singletonList("Zelda"), titles(renumbered.favourites));
        assertEquals(40, renumbered.favourites.get(0).appId);
        for (HomeModel.GameCard card : renumbered.library) {
            assertEquals(card.title, "Zelda".equals(card.title), card.favourite);
        }
        assertTrue(rig.repository.isFavourite("tower", "t-zelda", 40));
        assertFalse(rig.repository.isFavourite("tower", "t-other", 12));
    }

    private static List<String> titles(List<HomeModel.GameCard> cards) {
        List<String> titles = new ArrayList<>();
        for (HomeModel.GameCard card : cards) {
            titles.add(card.title);
        }
        return titles;
    }
}
