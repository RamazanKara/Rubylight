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
@Config(sdk = 29, application = Application.class)
public class FavouritesRowTest {
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

    @Test
    public void theFavouritesRowComesBeforeContinuePlayingAndTheLibrary() throws Exception {
        rig.recents.add(new com.limelight.nativeapi.RecentsStore.Recent("tower", "Tower", 11, "Portal", 10));
        rig.repository.setFavourite("tower", "Tower", 12, "Zelda", true);
        List<RecordedMenu> menus = new ArrayList<>();
        ConsoleHomeAdapter adapter = new ConsoleHomeAdapter(context, null, uuid -> null, recorder(menus));
        adapter.submit(model());

        // Header, Zelda (favourite), header, Portal (recent), header, library, header, hosts.
        assertEquals(ConsoleHomeAdapter.HEADER, adapter.viewTypeAt(0));
        assertEquals(ConsoleHomeAdapter.COVER_GAME, adapter.viewTypeAt(1));
        assertEquals(ConsoleHomeAdapter.HEADER, adapter.viewTypeAt(2));
        assertEquals(ConsoleHomeAdapter.WIDE_GAME, adapter.viewTypeAt(3));
        FrameLayout parent = new FrameLayout(context);
        RecyclerView.ViewHolder first = adapter.onCreateViewHolder(parent, ConsoleHomeAdapter.HEADER);
        adapter.onBindViewHolder(first, 0);
        assertEquals(context.getString(R.string.console_section_favourites),
                ((android.widget.TextView) first.itemView).getText().toString());
        assertEquals(1, adapter.firstCard());
    }

    @Test
    public void aLongPressOnAGameCardOpensItsMenuAndAHostCardOpensTheHostOptions() throws Exception {
        rig.repository.setFavourite("tower", "Tower", 12, "Zelda", true);
        List<RecordedMenu> menus = new ArrayList<>();
        ConsoleHomeAdapter adapter = new ConsoleHomeAdapter(context, null, uuid -> null, recorder(menus));
        HomeModel model = model();
        adapter.submit(model);
        FrameLayout parent = new FrameLayout(context);

        RecyclerView.ViewHolder card = adapter.onCreateViewHolder(parent, ConsoleHomeAdapter.COVER_GAME);
        adapter.onBindViewHolder(card, 1);
        assertTrue(card.itemView.isFocusable());
        assertTrue(card.itemView.performLongClick());
        assertEquals(1, menus.size());
        assertEquals("Zelda", menus.get(0).title);
        assertTrue(menus.get(0).favourite);
        assertTrue(card.itemView.getContentDescription().toString()
                .endsWith(context.getString(R.string.console_favourite_description)));

        // A host card has no game menu; its long press opens the PC's options instead.
        RecyclerView.ViewHolder host = adapter.onCreateViewHolder(parent, ConsoleHomeAdapter.HOST);
        adapter.onBindViewHolder(host, adapter.getItemCount() - 1);
        assertTrue(host.itemView.performLongClick());
        assertEquals(1, menus.size());
        assertEquals(1, hostOptions.size());
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
    @Config(qualifiers = "w960dp-h540dp-television-xhdpi")
    public void aHeldKeyPutsTheNewHomeCardStraightIntoFocusAndTheOldOneAtRest() throws Exception {
        ConsoleHomeAdapter adapter = new ConsoleHomeAdapter(context, null, uuid -> null, recorder(new ArrayList<>()));
        adapter.submit(model());
        List<Integer> covers = new ArrayList<>();
        for (int i = 0; i < adapter.getItemCount(); i++) {
            if (adapter.viewTypeAt(i) == ConsoleHomeAdapter.COVER_GAME) covers.add(i);
        }
        assertTrue(covers.size() >= 2);
        android.widget.LinearLayout row = new android.widget.LinearLayout(context);
        RecyclerView.ViewHolder first = adapter.onCreateViewHolder(row, ConsoleHomeAdapter.COVER_GAME);
        RecyclerView.ViewHolder second = adapter.onCreateViewHolder(row, ConsoleHomeAdapter.COVER_GAME);
        adapter.onBindViewHolder(first, covers.get(0));
        adapter.onBindViewHolder(second, covers.get(1));
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
    public void theMenuKeyOnAGameCardTogglesItsFavouriteOnceAndAHostCardIgnoresIt() throws Exception {
        rig.repository.setFavourite("tower", "Tower", 12, "Zelda", true);
        ConsoleHomeAdapter adapter = new ConsoleHomeAdapter(context, null, uuid -> null, recorder(new ArrayList<>()));
        adapter.submit(model());
        FrameLayout parent = new FrameLayout(context);
        RecyclerView.ViewHolder card = adapter.onCreateViewHolder(parent, ConsoleHomeAdapter.COVER_GAME);
        adapter.onBindViewHolder(card, 1);
        focus(parent, card.itemView);

        card.itemView.dispatchKeyEvent(new android.view.KeyEvent(android.view.KeyEvent.ACTION_DOWN,
                android.view.KeyEvent.KEYCODE_MENU));
        assertTrue(toggles.isEmpty());
        card.itemView.dispatchKeyEvent(new android.view.KeyEvent(android.view.KeyEvent.ACTION_UP,
                android.view.KeyEvent.KEYCODE_MENU));
        assertEquals(java.util.Collections.singletonList("Zelda"), toggles);

        RecyclerView.ViewHolder host = adapter.onCreateViewHolder(parent, ConsoleHomeAdapter.HOST);
        adapter.onBindViewHolder(host, adapter.getItemCount() - 1);
        parent.removeAllViews();
        focus(parent, host.itemView);
        host.itemView.dispatchKeyEvent(new android.view.KeyEvent(android.view.KeyEvent.ACTION_UP,
                android.view.KeyEvent.KEYCODE_MENU));
        assertEquals(1, toggles.size());
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

        // Favourites row first, then Continue playing with the running Portal.
        assertEquals(ConsoleHomeAdapter.HEADER, adapter.viewTypeAt(0));
        assertEquals(1, adapter.firstCard());
        assertEquals("favourite:tower:t-zelda", adapter.keyAt(1));
        int focus = adapter.initialFocus();
        assertEquals(ConsoleHomeAdapter.WIDE_GAME, adapter.viewTypeAt(focus));
        assertTrue(adapter.keyAt(focus).startsWith("continue:tower:11"));
    }

    @Test
    public void aCardIsFoundAgainByItsStableIdAndAMissingOneHasANeighbour() throws Exception {
        rig.repository.setFavourite("tower", "Tower", 12, "Zelda", true);
        ConsoleHomeAdapter adapter = new ConsoleHomeAdapter(context, null, uuid -> null, recorder(new ArrayList<>()));
        adapter.submit(model());
        int library = adapter.positionOfKey("game:host:tower:12");
        assertTrue(library > 1);
        assertEquals("game:host:tower:12", adapter.keyAt(library));
        assertEquals(-1, adapter.positionOfKey("favourite:tower:gone"));
        assertNull(adapter.keyAt(0));
        // Where a header or a removed card was, the nearest card takes over.
        assertEquals(1, adapter.nearestCard(0));
        assertNotNull(adapter.keyAt(adapter.nearestCard(2)));
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
