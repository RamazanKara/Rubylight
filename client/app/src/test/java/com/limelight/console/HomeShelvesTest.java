package com.limelight.console;

import static com.limelight.nativeapi.NativeApiFixtures.game;
import static com.limelight.nativeapi.NativeApiFixtures.library;
import static com.limelight.nativeapi.NativeApiFixtures.nativeHost;
import static com.limelight.nativeapi.NativeApiFixtures.ok;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertSame;
import static org.junit.Assert.assertTrue;

import android.app.Activity;
import android.app.Application;
import android.content.Context;
import android.preference.PreferenceManager;
import android.view.View;
import android.view.ViewGroup;
import android.widget.FrameLayout;
import android.widget.LinearLayout;
import android.widget.TextView;

import androidx.recyclerview.widget.LinearLayoutManager;
import androidx.recyclerview.widget.RecyclerView;

import com.limelight.R;
import com.limelight.nativeapi.NativeApiFixtures;
import com.limelight.ui.Motion;

import org.junit.After;
import org.junit.Before;
import org.junit.Rule;
import org.junit.Test;
import org.junit.rules.TemporaryFolder;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowLooper;

import java.util.Collections;
import java.util.HashSet;
import java.util.concurrent.TimeUnit;

/** Home's shelves: where a key lands, what a shelf remembers, the keyline of a row and what the hero is told. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class, qualifiers = "w960dp-h540dp-television-xhdpi")
public class HomeShelvesTest {
    @Rule public TemporaryFolder temporary = new TemporaryFolder();

    private ConsoleRig rig;
    private Activity activity;
    private ConsoleHomeAdapter adapter;
    private RecyclerView list;
    private View bar;
    private HomeModel model;

    @Before
    public void setUp() throws Exception {
        rig = new ConsoleRig(temporary.newFolder());
        // Attached to a window: the layout managers answer a focus search only once they are.
        ActivityController<Activity> controller = Robolectric.buildActivity(Activity.class);
        activity = controller.get();
        activity.setTheme(R.style.AppTheme);
        controller.create().start().resume();
        // Instant scrolls: a test lays the list out once and reads where it is.
        PreferenceManager.getDefaultSharedPreferences(activity).edit().putString(Motion.REDUCE_MOTION, Motion.REDUCE).commit();
        String[] cards = new String[12];
        for (int i = 0; i < cards.length; i++) {
            cards[i] = game("t-" + i, 100 + i, "Game " + i, null, false);
        }
        rig.transport.reply("tower", "library", ok(library("t1", cards)));
        rig.transport.reply("tower", "session", ok(NativeApiFixtures.IDLE));
        // Eight stars: the favourites shelf has eight cards above the library's twelve.
        for (int i = 0; i < 8; i++) rig.repository.setFavourite("tower", "Tower", "t-" + i, 100 + i, "Game " + i, true);
        adapter = new ConsoleHomeAdapter(activity, null, uuid -> null, new NoListener());
        adapter.setTelevision(true);
        list = new RecyclerView(activity);
        ShelfLayoutManager rows = new ShelfLayoutManager(activity, LinearLayoutManager.VERTICAL,
                () -> adapter.isFastTravel() || Motion.reduced(activity));
        rows.setRowFocus(adapter);
        list.setLayoutManager(rows);
        list.setAdapter(adapter);
        // A button above the shelves stands for the shell bar.
        LinearLayout root = new LinearLayout(activity);
        root.setOrientation(LinearLayout.VERTICAL);
        bar = new android.widget.Button(activity);
        bar.setFocusable(true);
        root.addView(bar, new LinearLayout.LayoutParams(200, 100));
        root.addView(list, new LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, 0, 1f));
        activity.setContentView(root);
        controller.visible();
        model = HomeModel.of(rig.repository.refresh(Collections.singletonList(nativeHost("tower", "Tower", true))),
                new HashSet<>(Collections.singletonList("tower")));
        adapter.submit(model);
        layout();
    }

    @After
    public void tearDown() {
        rig.close();
    }

    private void layout() {
        ViewGroup root = (ViewGroup) list.getParent();
        root.measure(View.MeasureSpec.makeMeasureSpec(1920, View.MeasureSpec.EXACTLY),
                View.MeasureSpec.makeMeasureSpec(1080, View.MeasureSpec.EXACTLY));
        root.layout(0, 0, 1920, 1080);
    }

    private View card(int row, int card) {
        adapter.reveal(list, new ConsoleHomeAdapter.Cursor(row, card));
        layout();
        View view = adapter.cardView(list, new ConsoleHomeAdapter.Cursor(row, card));
        assertNotNull("card " + card + " of row " + row + " is laid out", view);
        return view;
    }

    private View press(View from, int direction) {
        View next = from.focusSearch(direction);
        assertNotNull(next);
        assertTrue(next.requestFocus());
        layout();
        return next;
    }

    @Test
    public void gamesNotAddedYetKeepTheirOwnBackdropKeysByTitle() {
        // The host found both but added neither, so both have app id 0: the title keeps them apart.
        String portal = HomeAmbience.gameKey("tower", 0, "Portal");
        String zelda = HomeAmbience.gameKey("tower", 0, "Zelda");
        assertFalse(portal.equals(zelda));
        assertEquals(portal, HomeAmbience.gameKey("tower", 0, "Portal"));
        // An added game is keyed by its app id, so a rename keeps its backdrop.
        assertEquals(HomeAmbience.gameKey("tower", 11, "Portal"), HomeAmbience.gameKey("tower", 11, "Portal 2"));
        assertFalse(HomeAmbience.gameKey("tower", 11, "Portal").equals(portal));
        assertFalse(HomeAmbience.gameKey("den", 0, "Portal").equals(portal));
    }

    @Test
    public void downFromACardLandsOnTheNearestCardOfTheNextShelfAndUpReturnsToIt() {
        assertEquals("shelf:favourites", adapter.rowKey(0));
        assertEquals("shelf:store:steam", adapter.rowKey(1));
        View fifth = card(0, 5);
        assertTrue(fifth.requestFocus());
        layout();
        int[] where = new int[2];
        fifth.getLocationInWindow(where);
        int centre = where[0] + fifth.getWidth() / 2;

        View below = press(fifth, View.FOCUS_DOWN);
        ConsoleHomeAdapter.Cursor landed = adapter.focusedCard(list);
        assertEquals("the next shelf", 1, landed.row);
        // No other card of that shelf is closer across.
        int best = Integer.MAX_VALUE;
        RecyclerView cards = ((ConsoleHomeAdapter.ShelfHolder) list.findViewHolderForAdapterPosition(1)).cards;
        for (int i = 0; i < cards.getChildCount(); i++) {
            cards.getChildAt(i).getLocationInWindow(where);
            best = Math.min(best, Math.abs(where[0] + cards.getChildAt(i).getWidth() / 2 - centre));
        }
        below.getLocationInWindow(where);
        assertEquals(best, Math.abs(where[0] + below.getWidth() / 2 - centre));

        View back = press(below, View.FOCUS_UP);
        assertEquals("back on the card it left", new ConsoleHomeAdapter.Cursor(0, 5), adapter.focusedCard(list));
        assertSame(fifth, back);
    }

    private ConsoleHomeAdapter.ShelfHolder holder(int row) {
        return (ConsoleHomeAdapter.ShelfHolder) list.findViewHolderForAdapterPosition(row);
    }

    private int firstVisible(int row) {
        return holder(row).layout.findFirstVisibleItemPosition();
    }

    private int offsetOf(int row) {
        ConsoleHomeAdapter.ShelfHolder shelf = holder(row);
        return shelf.layout.findViewByPosition(firstVisible(row)).getLeft();
    }

    @Test
    public void aShelfKeepsItsPlaceAndFocusThroughARefreshAndAReturnFromTheGamePage() {
        View eighth = card(1, 8);
        assertTrue(eighth.requestFocus());
        layout();
        int first = firstVisible(1);
        int offset = offsetOf(1);
        assertTrue("the shelf is scrolled", first > 0);
        String focused = adapter.keyAt(adapter.focusedCard(list));
        ConsoleHomeAdapter.ShelfHolder before = holder(1);

        // The host feed polls: the same model comes in again, as it does when the game page is left with Back.
        for (int i = 0; i < 3; i++) {
            adapter.submit(model);
            layout();
        }

        assertSame("the same shelf view", before, holder(1));
        assertEquals("the same first card", first, firstVisible(1));
        assertEquals("at the same offset", offset, offsetOf(1));
        ConsoleHomeAdapter.Cursor at = adapter.focusedCard(list);
        assertNotNull("focus stayed on a card", at);
        assertEquals("and on the same one", focused, adapter.keyAt(at));
    }

    /** A model of the hosts alone: no favourites, no library, so those shelves leave the list and their holders are recycled. */
    private HomeModel hostsOnly() throws Exception {
        rig.transport.reply("tower", "library", ok(library("t1")));
        for (int i = 0; i < 8; i++) rig.repository.setFavourite("tower", "Tower", "t-" + i, 100 + i, "Game " + i, false);
        return HomeModel.of(rig.repository.refresh(Collections.singletonList(nativeHost("tower", "Tower", true))),
                new HashSet<>(Collections.singletonList("tower")));
    }

    @Test
    public void aShelfRecycledAndBroughtBackStaysWhereTheUserScrolledItOnARefresh() throws Exception {
        card(1, 8);
        int saved = firstVisible(1);
        assertTrue(saved > 0);
        // The library leaves the list, which recycles its holder (and keeps where it was), then comes back at that place.
        adapter.submit(hostsOnly());
        layout();
        adapter.submit(model);
        layout();
        assertEquals("the library is back where it was left", saved, firstVisible(1));

        // The user scrolls it on to another card; that, not the place saved at the recycle, is where a refresh must keep it.
        View eleventh = card(1, 11);
        assertTrue(eleventh.requestFocus());
        layout();
        int first = firstVisible(1);
        int offset = offsetOf(1);
        assertTrue("further along than it was saved", first > saved);
        String focused = adapter.keyAt(adapter.focusedCard(list));

        for (int i = 0; i < 3; i++) {
            adapter.submit(model);
            layout();
        }

        assertEquals("the same first card", first, firstVisible(1));
        assertEquals("at the same offset", offset, offsetOf(1));
        ConsoleHomeAdapter.Cursor at = adapter.focusedCard(list);
        assertNotNull("focus stayed on a card", at);
        assertEquals("and on the same one", focused, adapter.keyAt(at));
    }

    @Test
    public void aShelfThatTakesAHolderSetsItToItsOwnPlaceNotTheLastOnes() throws Exception {
        card(1, 8);
        int first = firstVisible(1);
        assertTrue(first > 0);
        // Only the PCs remain: their shelf takes up a holder that showed the scrolled library.
        HomeModel hosts = hostsOnly();
        adapter.submit(hosts);
        layout();
        assertEquals(1, adapter.rowCount());
        assertEquals("shelf:hosts", adapter.rowKey(0));
        assertEquals(0, firstVisible(0));
        // The library is back where it was left.
        adapter.submit(model);
        layout();
        int row = 1;
        assertEquals("shelf:store:steam", adapter.rowKey(row));
        assertEquals(first, firstVisible(row));
    }

    @Test
    public void downFromTheShellBarLandsOnTheCardTheFirstShelfRemembers() {
        View fifth = card(0, 4);
        assertTrue(fifth.requestFocus());
        layout();
        // Nearest across to the bar would be the first card.
        assertTrue(bar.requestFocus());
        layout();
        assertTrue(bar.hasFocus());
        View next = bar.focusSearch(View.FOCUS_DOWN);
        assertSame("the remembered card, not the one nearest across", fifth, next);
        assertTrue(next.requestFocus());
        layout();
        assertEquals(new ConsoleHomeAdapter.Cursor(0, 4), adapter.focusedCard(list));
    }

    @Test
    public void backOnAMarkedHostCardOnlyUnmarksTheDotsAndACancelledOneDoesNot() {
        ConsoleHomeAdapter.Cursor hostAt = new ConsoleHomeAdapter.Cursor(adapter.rowCount() - 1, 0);
        View host = card(hostAt.row, hostAt.card);
        assertTrue(host.requestFocus());
        layout();
        View more = host.findViewById(R.id.console_more);
        host.dispatchKeyEvent(new android.view.KeyEvent(android.view.KeyEvent.ACTION_DOWN, android.view.KeyEvent.KEYCODE_DPAD_RIGHT));
        assertTrue("the end of the shelf marks the dots", more.isHovered());

        host.dispatchKeyEvent(new android.view.KeyEvent(android.view.KeyEvent.ACTION_DOWN, android.view.KeyEvent.KEYCODE_BACK));
        host.dispatchKeyEvent(new android.view.KeyEvent(0, 0, android.view.KeyEvent.ACTION_UP, android.view.KeyEvent.KEYCODE_BACK,
                0, 0, 0, 0, android.view.KeyEvent.FLAG_CANCELED));
        assertTrue("a cancelled Back does nothing", more.isHovered());

        assertTrue("Back is taken by the card, so the screen stays", host.dispatchKeyEvent(
                new android.view.KeyEvent(android.view.KeyEvent.ACTION_DOWN, android.view.KeyEvent.KEYCODE_BACK)));
        host.dispatchKeyEvent(new android.view.KeyEvent(android.view.KeyEvent.ACTION_UP, android.view.KeyEvent.KEYCODE_BACK));
        assertFalse("Back only unmarks", more.isHovered());
        assertEquals("and focus stays on the card", hostAt, adapter.focusedCard(list));
    }

    @Test
    public void aShelfIsNamedForTheListOfItsCards() {
        ConsoleHomeAdapter.ShelfHolder shelf = holder(0);
        assertEquals(shelf.title.getText().toString(), shelf.cards.getContentDescription().toString());
        assertEquals(shelf.cards.getId(), shelf.title.getLabelFor());
    }

    @Test
    public void aShelfRemembersItsCardWhileFocusIsOnAnotherShelf() {
        View first = card(0, 0);
        assertTrue(first.requestFocus());
        layout();
        View below = press(first, View.FOCUS_DOWN);
        View right = press(below, View.FOCUS_RIGHT);
        View right2 = press(right, View.FOCUS_RIGHT);
        ConsoleHomeAdapter.Cursor onLibrary = adapter.focusedCard(list);
        assertEquals(1, onLibrary.row);
        assertEquals(2, onLibrary.card);
        // Up to the favourites, then down again: the library shelf has its third card.
        press(right2, View.FOCUS_UP);
        assertEquals(0, adapter.focusedCard(list).row);
        View again = press(adapter.cardView(list, adapter.focusedCard(list)), View.FOCUS_DOWN);
        assertEquals(onLibrary, adapter.focusedCard(list));
        assertSame(right2, again);
    }

    @Test
    public void theCardFocusIsHeldAtTheEndOfAShelfAndTheFirstShelfHasNothingAbove() {
        View last = card(0, adapter.cardCount(0) - 1);
        assertTrue(last.requestFocus());
        layout();
        assertSame("nothing to the right of the last card", last, last.focusSearch(View.FOCUS_RIGHT));
        View first = card(0, 0);
        assertTrue(first.requestFocus());
        layout();
        assertSame("nothing to the left of the first", first, first.focusSearch(View.FOCUS_LEFT));
    }

    @Test
    public void aFocusedCardIsFullyInViewAndTheNextOnePeeksIn() {
        RecyclerView cards = ((ConsoleHomeAdapter.ShelfHolder) list.findViewHolderForAdapterPosition(0)).cards;
        View view = card(0, 5);
        assertTrue(view.requestFocus());
        layout();
        int end = cards.getWidth() - cards.getPaddingRight();
        assertTrue("fully in view at its right edge", view.getRight() <= end);
        assertTrue("and at its left", view.getLeft() >= cards.getPaddingLeft());
        View next = adapter.cardView(list, new ConsoleHomeAdapter.Cursor(0, 6));
        assertNotNull("the next card is laid out", next);
        assertTrue("and starts inside the strip", next.getLeft() < cards.getWidth());
    }

    @Test
    public void cardShiftKeepsTheNextCardInViewAtTheEndAndTheFocusedOneFullyAtTheStart() {
        // A strip 0..1000 of 200-wide cards.
        assertEquals("inside", 0, ShelfLayoutManager.cardShift(400, 600, 0, 1000));
        assertEquals("the last one that leaves the next a peek", 0, ShelfLayoutManager.cardShift(600, 800, 0, 1000));
        assertEquals("one more and the next must show", 100, ShelfLayoutManager.cardShift(700, 900, 0, 1000));
        assertEquals("a card at the very end", 200, ShelfLayoutManager.cardShift(800, 1000, 0, 1000));
        assertEquals("a card past the end", 400, ShelfLayoutManager.cardShift(1000, 1200, 0, 1000));
        assertEquals("cut off at the start", -50, ShelfLayoutManager.cardShift(-50, 150, 0, 1000));
        assertEquals("padding is the start", -20, ShelfLayoutManager.cardShift(28, 228, 48, 1000));
    }

    @Test
    public void aShelfScrollsAtOnceWhenMotionIsReduced() {
        RecyclerView cards = ((ConsoleHomeAdapter.ShelfHolder) list.findViewHolderForAdapterPosition(0)).cards;
        assertEquals(0, cards.computeHorizontalScrollOffset());
        View view = adapter.cardView(list, new ConsoleHomeAdapter.Cursor(0, 6));
        assertNotNull("laid out past the edge", view);
        view.requestFocus();
        layout();
        assertTrue("scrolled in the same pass", cards.computeHorizontalScrollOffset() > 0);
    }

    @Test
    public void aShelfScrollsOverTheDurationOfTheEasingWhenMotionIsFull() {
        PreferenceManager.getDefaultSharedPreferences(activity).edit().putString(Motion.REDUCE_MOTION, Motion.FULL).commit();
        RecyclerView cards = ((ConsoleHomeAdapter.ShelfHolder) list.findViewHolderForAdapterPosition(0)).cards;
        View view = adapter.cardView(list, new ConsoleHomeAdapter.Cursor(0, 6));
        assertNotNull("laid out past the edge", view);
        assertEquals(0, cards.computeHorizontalScrollOffset());
        view.requestFocus();
        layout();
        assertEquals("the smooth scroll has not started yet", 0, cards.computeHorizontalScrollOffset());
        ShadowLooper.idleMainLooper(ShelfLayoutManager.CARD_MS + 200, TimeUnit.MILLISECONDS);
        assertTrue("and has got there", cards.computeHorizontalScrollOffset() > 0);
    }

    @Test
    public void theHeroIsToldOncePerFrameWhateverTheKeysDid() {
        LinearLayout hero = new LinearLayout(activity);
        CountingText meta = new CountingText(activity, R.id.home_hero_meta);
        CountingText title = new CountingText(activity, R.id.home_hero_title);
        CountingText hints = new CountingText(activity, R.id.home_hero_hints);
        hero.addView(meta);
        hero.addView(title);
        hero.addView(hints);
        HomeAmbience ambience = new HomeAmbience(activity, null, hero, uuid -> null, () -> null, () -> true);
        for (int i = 0; i < 6; i++) {
            ConsoleHomeAdapter.Cursor at = new ConsoleHomeAdapter.Cursor(1, i);
            ambience.onCardFocus("shelf:library", adapter.itemAt(at), card(1, i));
        }
        assertEquals("nothing is written between frames", 0, title.writes);
        ShadowLooper.idleMainLooper(100, TimeUnit.MILLISECONDS);
        assertEquals("one write for six cards", 1, title.writes);
        assertEquals(adapter.itemAt(new ConsoleHomeAdapter.Cursor(1, 5)).game.title, title.getText().toString());
        // The same card told again costs no text layout.
        ambience.onCardFocus("shelf:library", adapter.itemAt(new ConsoleHomeAdapter.Cursor(1, 5)), card(1, 5));
        ShadowLooper.idleMainLooper(100, TimeUnit.MILLISECONDS);
        assertEquals(1, title.writes);
        assertEquals(adapter.keyAt(new ConsoleHomeAdapter.Cursor(1, 5)), ambience.current());
    }

    /** Counts what is written to it, to show the hero is not rewritten for every card a key passes. */
    private static final class CountingText extends TextView {
        int writes;

        CountingText(Context context, int id) {
            super(context);
            setId(id);
            writes = 0; // the constructor of a TextView writes its empty text

        }

        @Override public void setText(CharSequence text, BufferType type) {
            writes++;
            super.setText(text, type);
        }
    }

    private static final class NoListener implements ConsoleHomeAdapter.Listener {
        @Override public void onGame(HomeModel.GameCard card) { }

        @Override public void onGameMenu(HomeModel.GameCard card) { }

        @Override public void onGameToggleFavourite(HomeModel.GameCard card) { }

        @Override public void onHost(HomeModel.HostCard card) { }

        @Override public void onHostOptions(HomeModel.HostCard card) { }
    }
}
