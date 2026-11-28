package com.limelight.console;

import static com.limelight.nativeapi.NativeApiFixtures.library;
import static com.limelight.nativeapi.NativeApiFixtures.nativeHost;
import static com.limelight.nativeapi.NativeApiFixtures.ok;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertSame;
import static org.junit.Assert.assertTrue;
import static org.robolectric.Shadows.shadowOf;

import android.app.Application;
import android.content.res.Resources;
import android.graphics.Color;
import android.graphics.Rect;
import android.graphics.drawable.GradientDrawable;
import android.os.Looper;
import android.view.Gravity;
import android.view.KeyEvent;
import android.view.View;
import android.view.ViewGroup;
import android.widget.FrameLayout;
import android.widget.TextView;

import androidx.core.view.ViewCompat;
import androidx.core.view.WindowInsetsCompat;
import androidx.recyclerview.widget.RecyclerView;

import com.limelight.PcViewLifecycleTest;
import com.limelight.R;
import com.limelight.nativeapi.NativeApiFixtures;
import com.limelight.ui.GlassForm;
import com.limelight.ui.ShellBar;
import com.limelight.ui.TvAudit;
import com.limelight.ui.TvScreensPreviewTest;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.GraphicsMode;
import org.robolectric.shadows.ShadowDialog;
import org.robolectric.shadows.ShadowLooper;
import org.robolectric.util.ReflectionHelpers;

import java.io.File;
import java.time.Duration;
import java.util.Collections;
import java.util.HashSet;

/** D-pad focus navigation, TV overscan/scroll/clipping, hero ambience and search bar behavior on Home. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class, shadows = PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class HomeShellNavigationTest extends HomeShellCase {

    @Test
    @Config(qualifiers = "television")
    public void onTelevisionTheDpadWalksTheShellBarInOrderAndStepsBetweenBarAndCards() throws Exception {
        ActivityController<HomeShellTest.Home> controller = home();
        try {
            HomeShellTest.Home home = controller.get();
            showHosts(home, NativeApiFixtures.nativeHost("tower", "Tower", true));
            View card = hostCard(home, "tower");
            View homeButton = home.findViewById(R.id.shell_home);
            View pcs = home.findViewById(R.id.shell_pcs);
            View settings = home.findViewById(R.id.shell_settings);
            assertSame("Home, then PCs", pcs, HomeShellTest.focusAfter(home, homeButton, View.FOCUS_RIGHT));
            assertSame("then Settings", settings, HomeShellTest.focusAfter(home, pcs, View.FOCUS_RIGHT));
            assertSame("and back", pcs, HomeShellTest.focusAfter(home, settings, View.FOCUS_LEFT));
            View below = HomeShellTest.focusAfter(home, homeButton, View.FOCUS_DOWN);
            assertNotNull("down from the bar reaches a card", below);
            assertTrue("a card of the list", isInside(list(home), below));
            // The library's game is first on the page; the PC's card sits under it.
            ConsoleHomeAdapter adapter = ReflectionHelpers.getField(home, "shelves");
            View first = adapter.cardView(list(home), adapter.initialFocus());
            assertNotNull(first);
            assertSame("up from the PC's card reaches the game above it", first,
                    HomeShellTest.focusAfter(home, card, View.FOCUS_UP));
            View above = HomeShellTest.focusAfter(home, first, View.FOCUS_UP);
            assertNotNull("up from the first card reaches the bar", above);
            assertTrue("a button of the bar", isInside(home.findViewById(R.id.shell_bar), above));
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void aGameTheHostHasNotAddedOffersNoStar() throws Exception {
        ActivityController<HomeShellTest.Home> controller = home();
        ConsoleRig rig = new ConsoleRig(temporary.newFolder());
        try {
            HomeShellTest.Home home = controller.get();
            ReflectionHelpers.setField(home, "repository", rig.repository);
            rig.transport.reply("tower", "capabilities", ok("{\"api_version\":1,\"features\":[\"library\","
                    + "\"library.unadded\"],\"permissions\":{\"list\":true,\"view\":true,\"launch\":true}}"));
            rig.transport.reply("tower", "library", ok(library("t", HomeLibraryRowsTest.game(1, "Portal", "steam", true,
                    null), HomeLibraryRowsTest.found("Hades", "epic"))));
            rig.transport.reply("tower", "session", ok(NativeApiFixtures.IDLE));
            HomeModel model = HomeModel.of(rig.repository.refresh(Collections.singletonList(
                    nativeHost("tower", "Tower", true))), new HashSet<>(Collections.singletonList("tower")));
            HomeModel.GameCard portal = null;
            HomeModel.GameCard hades = null;
            for (HomeModel.GameCard card : model.allGames) {
                if (card.title.equals("Portal")) portal = card;
                if (card.title.equals("Hades")) hades = card;
            }
            assertTrue(hades.notAdded());

            home.askFavourite(hades);
            assertNull("no menu for a game without an app id", ShadowDialog.getLatestDialog());
            home.askFavourite(portal);
            assertNotNull(ShadowDialog.getLatestDialog());
        } finally {
            rig.close();
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void onAPhoneWithAGamepadTheDpadReachesTheShellBarFromTheTopCardAndBack() throws Exception {
        chooseGrid();
        ActivityController<HomeShellTest.Home> controller = home();
        try {
            HomeShellTest.Home home = controller.get();
            showHosts(home, NativeApiFixtures.nativeHost("tower", "Tower", true));
            ConsoleHomeListAdapter adapter = ReflectionHelpers.getField(home, "adapter");
            assertNotNull("a phone keeps the list of cards", adapter);
            RecyclerView.ViewHolder top = list(home).findViewHolderForAdapterPosition(adapter.initialFocus());
            assertNotNull(top);
            View above = HomeShellTest.focusAfter(home, top.itemView, View.FOCUS_UP);
            assertNotNull("up from the first card reaches the top row", above);
            assertTrue("a control of the top row (the tabs are at the foot)", isInside(home.findViewById(R.id.home_top_row), above));
            View below = HomeShellTest.focusAfter(home, above, View.FOCUS_DOWN);
            assertNotNull("down from the bar reaches a card", below);
            assertTrue("a card of the list", isInside(list(home), below));
        } finally {
            controller.pause().stop().destroy();
        }
    }

    /**
     * Presses Up as the system does: the activity first, then the focus search. Walks from {@code from} until focus is on
     * the shell bar, laying the list out after each step.
     */
    private static View walkUp(HomeShellTest.Home home, View from, int limit) {
        View at = from;
        int stuck = 0;
        for (int i = 0; i < limit; i++) {
            shadowOf(Looper.getMainLooper()).idle();
            at.requestFocus();
            HomeShellTest.focusAfter(home, at, View.FOCUS_UP);
            if (!home.dispatchKeyEvent(new KeyEvent(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_DPAD_UP))) {
                View next = HomeShellTest.focusAfter(home, at, View.FOCUS_UP);
                if (next == null) return null;
                if (next != at) next.requestFocus();
            }
            View now = home.getWindow().getDecorView().findFocus();
            assertNotNull("focus after step " + i + " from " + at, now);
            if (isInside(home.findViewById(R.id.home_top_row), now)) return now;
            // A press may only scroll the list to the header above the first row; the next one moves on.
            if (now == at && ++stuck > 2) return at;
            if (now != at) stuck = 0;
            at = now;
        }
        return at;
    }

    /** Home waits for the first key to place its start. Once a key has placed focus, a refresh must not take it back there. */
    @Test
    public void aRefreshLeavesFocusWhereAKeyPutIt() {
        ActivityController<HomeShellTest.Home> controller = home();
        try {
            HomeShellTest.Home home = controller.get();
            View tab = home.findViewById(R.id.shell_home);
            assertTrue(tab.requestFocusFromTouch());
            ReflectionHelpers.setField(home, "focusPending", true);
            home.dispatchKeyEvent(new KeyEvent(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_DPAD_LEFT));
            assertFalse(ReflectionHelpers.<Boolean>getField(home, "focusPending"));
        } finally {
            controller.pause().stop().destroy();
        }
    }

    /** On a phone the hero fills the width under the top row, so the pairing banner rests on the bar instead of covering the art. */
    @Test
    public void onAPhoneThePairingBannerStaysOffTheHero() {
        ActivityController<HomeShellTest.Home> controller = home();
        try {
            HomeShellTest.Home home = controller.get();
            View root = home.findViewById(R.id.home_pairing_banner);
            ViewCompat.dispatchApplyWindowInsets(home.getWindow().getDecorView(), new WindowInsetsCompat.Builder().build());
            ShadowLooper.idleMainLooper();
            FrameLayout.LayoutParams banner = (FrameLayout.LayoutParams) root.getLayoutParams();
            assertEquals(Gravity.BOTTOM, banner.gravity & Gravity.VERTICAL_GRAVITY_MASK);
            assertTrue(banner.bottomMargin >= home.getResources().getDimensionPixelSize(R.dimen.nj_size_tab_bar_height));
            // A pad's hints stand there too: they ran through each other. The banner rests on them, and comes back down.
            View hints = home.findViewById(R.id.home_hints);
            View decor = home.getWindow().getDecorView();
            hints.setVisibility(View.VISIBLE);
            hints.layout(0, 0, 100, 40);
            decor.getViewTreeObserver().dispatchOnGlobalLayout();
            assertTrue("lifted over the hints: " + root.getTranslationY(), root.getTranslationY() <= -40);
            hints.setVisibility(View.GONE);
            decor.getViewTreeObserver().dispatchOnGlobalLayout();
            assertEquals(0f, root.getTranslationY(), 0f);
            // Over a bright sky their words vanished, and a cover's label read through the chip.
            int fill = ((GradientDrawable) root.getBackground()).getColor().getDefaultColor();
            assertTrue("dense over art: " + Integer.toHexString(fill), Color.alpha(fill) >= 0xD0);
            assertEquals(R.drawable.glass_chip_over_art, shadowOf(hints.getBackground()).getCreatedFromResId());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void onAPhoneUpFromALongLibraryReachesTheShellBar() throws Exception {
        chooseGrid();
        ActivityController<HomeShellTest.Home> controller = home();
        try {
            HomeShellTest.Home home = controller.get();
            showHosts(home, 14, NativeApiFixtures.nativeHost("tower", "Tower", true));
            ConsoleHomeListAdapter adapter = ReflectionHelpers.getField(home, "adapter");
            int last = adapter.getItemCount() - 1;
            list(home).scrollToPosition(last);
            HomeShellTest.focusAfter(home, home.findViewById(R.id.shell_home), View.FOCUS_UP);
            RecyclerView.ViewHolder holder = list(home).findViewHolderForAdapterPosition(last);
            assertNotNull(holder);
            assertTrue(holder.itemView.requestFocus());
            View end = walkUp(home, holder.itemView, 40);
            assertNotNull("focus does not get stuck", end);
            assertTrue("ends in the top row (the tabs are at the foot), not on " + end,
                    isInside(home.findViewById(R.id.home_top_row), end));
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    @Config(qualifiers = "television")
    public void onTelevisionUpFromTheTopShelfOfALongLibraryReachesTheShellBar() throws Exception {
        ActivityController<HomeShellTest.Home> controller = home();
        try {
            HomeShellTest.Home home = controller.get();
            showHosts(home, 14, NativeApiFixtures.nativeHost("tower", "Tower", true));
            ConsoleHomeAdapter shelves = ReflectionHelpers.getField(home, "shelves");
            View card = shelves.cardView(list(home), new ConsoleHomeAdapter.Cursor(0, 9));
            assertNotNull("a card in the first shelf", card);
            assertTrue(card.requestFocus());
            View end = walkUp(home, card, 40);
            assertNotNull("focus does not get stuck", end);
            assertTrue("ends on a button of the bar, not on " + end, end == home.findViewById(R.id.shell_home));
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    @Config(qualifiers = "television")
    public void onTelevisionHomeKeepsItsTabBeforeAnyLibraryIsSavedAndUpFromTheShelvesLandsOnIt() throws Exception {
        // Home opens before its first library is on disk (a new PC, cleared data, the demo's first start).
        assertTrue(new File(new File(context.getFilesDir(), "console-library"), "tower.json").delete());
        assertFalse(ShellBar.homeAvailable(context));
        ActivityController<HomeShellTest.Home> controller = home();
        try {
            HomeShellTest.Home home = controller.get();
            View homeButton = home.findViewById(R.id.shell_home);
            assertEquals("the current tab stays in the bar", View.VISIBLE, homeButton.getVisibility());
            assertTrue(homeButton.isSelected());
            showHosts(home, 14, NativeApiFixtures.nativeHost("tower", "Tower", true));
            controller.pause().resume();
            assertEquals("and stays when Home comes back", View.VISIBLE, homeButton.getVisibility());
            ConsoleHomeAdapter shelves = ReflectionHelpers.getField(home, "shelves");
            View card = shelves.cardView(list(home), new ConsoleHomeAdapter.Cursor(0, 9));
            assertNotNull("a card in the first shelf", card);
            assertTrue(card.requestFocus());
            View end = walkUp(home, card, 40);
            assertSame("Up lands on Home, not on " + end, homeButton, end);
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    @Config(qualifiers = "television")
    public void onTelevisionTheHeroShowsTheFirstCardWhileNoCardHasFocus() throws Exception {
        ActivityController<HomeShellTest.Home> controller = home();
        try {
            HomeShellTest.Home home = controller.get();
            RecyclerView list = showHosts(home, 3, NativeApiFixtures.nativeHost("tower", "Tower", true));
            // Started from a touch (or adb), no card takes focus until a key is pressed.
            Object root = ReflectionHelpers.callInstanceMethod(home.getWindow().getDecorView(), "getViewRootImpl");
            ReflectionHelpers.callInstanceMethod(root, "ensureTouchMode",
                    ReflectionHelpers.ClassParameter.from(boolean.class, true));
            assertTrue(list.isInTouchMode());
            ReflectionHelpers.callInstanceMethod(home, "focusFirstCard",
                    ReflectionHelpers.ClassParameter.from(boolean.class, false));
            shadowOf(Looper.getMainLooper()).idleFor(Duration.ofMillis(100));
            ConsoleHomeAdapter shelves = ReflectionHelpers.getField(home, "shelves");
            assertNull("no card has focus", shelves.focusedCard(list));
            ConsoleHomeAdapter.Cursor first = shelves.initialFocus();
            HomeAmbience ambience = ReflectionHelpers.getField(home, "ambience");
            assertEquals("the hero follows the card focus would start on", shelves.keyAt(first), ambience.current());
            assertEquals(shelves.itemAt(first).game.title,
                    ((TextView) home.findViewById(R.id.home_hero_title)).getText().toString());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    /**
     * nightjar#7: a scrim of the night canvas over the TV list's top edge rose with the first row's place, so it flashed as
     * a dark strip under the hero with every row that scrolled past, over a bright ambient. The list's own fading edge
     * dissolves rows into what lies behind them instead, and nothing is painted over that edge.
     */
    @Test
    @Config(qualifiers = "w960dp-h540dp-land-television-xhdpi")
    public void onTelevisionNothingIsPaintedOverTheTopOfTheListWhileItScrolls() throws Exception {
        ActivityController<HomeShellTest.Home> controller = home();
        try {
            HomeShellTest.Home home = controller.get();
            View root = home.getWindow().getDecorView();
            root.measure(View.MeasureSpec.makeMeasureSpec(1920, View.MeasureSpec.EXACTLY),
                    View.MeasureSpec.makeMeasureSpec(1080, View.MeasureSpec.EXACTLY));
            root.layout(0, 0, 1920, 1080);
            RecyclerView list = showHosts(home, 14, NativeApiFixtures.nativeHost("tower", "Tower", true));
            list.measure(View.MeasureSpec.makeMeasureSpec(1920, View.MeasureSpec.EXACTLY),
                    View.MeasureSpec.makeMeasureSpec(1080, View.MeasureSpec.EXACTLY));
            list.layout(0, 0, 1920, 1080);
            shadowOf(Looper.getMainLooper()).idle();
            int fade = list.getVerticalFadingEdgeLength();
            assertTrue("the list fades its own rows at its top", list.isVerticalFadingEdgeEnabled() && fade > 0);
            float atRest = ReflectionHelpers.callInstanceMethod(list, "getTopFadingEdgeStrength");
            assertEquals("at its start nothing is cut, so nothing fades", 0f, atRest, 0f);
            ViewGroup content = home.findViewById(R.id.home_content);
            int at = content.indexOfChild(list);
            // Mid-row and a row further on: the old scrim's strength jumped between these as rows left the list.
            for (int dy : new int[] {200, 333}) {
                list.scrollBy(0, dy);
                shadowOf(Looper.getMainLooper()).idle();
                float strength = ReflectionHelpers.callInstanceMethod(list, "getTopFadingEdgeStrength");
                assertEquals("scrolled, the rows dissolve at the edge", 1f, strength, 0f);
                for (int i = at + 1; i < content.getChildCount(); i++) {
                    View over = content.getChildAt(i);
                    boolean coversEdge = over.getVisibility() == View.VISIBLE && over.getTop() < list.getTop() + fade
                            && over.getBottom() > list.getTop();
                    assertFalse("nothing is drawn over the list's top edge: " + home.getResources().getResourceEntryName(over.getId()),
                            coversEdge);
                }
            }
        } finally {
            controller.pause().stop().destroy();
        }
    }

    /** Home on a 1080p TV: no text under the TV caption size, and nothing read or focused in the overscan band. */
    @Test
    @Config(qualifiers = "w960dp-h540dp-land-television-xhdpi")
    @GraphicsMode(GraphicsMode.Mode.NATIVE)
    public void onTelevisionHomeKeepsItsTextReadableAndInsideTheOverscan() throws Exception {
        ActivityController<HomeShellTest.Home> controller = home();
        try {
            HomeShellTest.Home home = controller.get();
            showHosts(home, 6, true, nativeHost("tower", "Tower", true), nativeHost("attic", "Attic", false));
            assertEquals(Collections.emptyList(), TvAudit.findings(home));
            TvScreensPreviewTest.previewIfRequested(home, "Home-shelves");
        } finally {
            controller.pause().stop().destroy();
        }
    }

    /** The capsule the other pages are held to: Home's, at the overscan. */
    @Test
    @Config(qualifiers = "w960dp-h540dp-land-television-xhdpi")
    public void theCapsuleOnATelevisionStandsAtTheOverscan() {
        ActivityController<HomeShellTest.Home> controller = home();
        try {
            assertEquals(HomeShellTest.tvCapsule(), TvAudit.topCapsule(controller.get()));
        } finally {
            controller.pause().stop().destroy();
        }
    }

    /**
     * The second launch on the real TV: what was saved came first, the hosts' answer right after it, and the place on the list
     * asked for under the first landed on another game in the second. Steam at the top, sideways, and no ring.
     */
    @Test
    @Config(qualifiers = "w960dp-h540dp-land-television-xhdpi")
    public void theStartFollowsTheHostsAnswerUntilAKeyIsPressed() throws Exception {
        ActivityController<HomeShellTest.Home> controller = home();
        try {
            HomeShellTest.Home home = controller.get();
            RecyclerView list = showHosts(home, 12, NativeApiFixtures.nativeHost("tower", "Tower", true));
            Object root = ReflectionHelpers.callInstanceMethod(home.getWindow().getDecorView(), "getViewRootImpl");
            ReflectionHelpers.callInstanceMethod(root, "ensureTouchMode",
                    ReflectionHelpers.ClassParameter.from(boolean.class, false));
            ConsoleHomeAdapter shelves = ReflectionHelpers.getField(home, "shelves");
            layOut(home);
            ConsoleHomeAdapter.Cursor last = new ConsoleHomeAdapter.Cursor(0, shelves.cardCount(0) - 1);
            // A stale wish: asked for before the hosts answered, run after.
            ReflectionHelpers.callInstanceMethod(home, "restoreFocus",
                    ReflectionHelpers.ClassParameter.from(ConsoleHomeAdapter.Cursor.class, last),
                    ReflectionHelpers.ClassParameter.from(int.class, 0));
            ReflectionHelpers.callInstanceMethod(home, "focusFirstCard",
                    ReflectionHelpers.ClassParameter.from(boolean.class, false));
            for (int i = 0; i < 10; i++) {
                shadowOf(Looper.getMainLooper()).idleFor(Duration.ofMillis(32));
                layOut(home);
            }
            ConsoleHomeAdapter.Cursor focused = shelves.focusedCard(list);
            assertNotNull("a card has the ring", focused);
            assertEquals("the start, not the stale place", shelves.keyAt(shelves.initialFocus()), shelves.keyAt(focused));
        } finally {
            controller.pause().stop().destroy();
        }
    }

    /** A TV's lens opened All games, which has no field: it searches on the page, as every other form does, and comes back. */
    @Test
    @Config(qualifiers = "w960dp-h540dp-land-television-xhdpi")
    public void onATelevisionTheLensSearchesOnThePage() {
        ActivityController<HomeShellTest.Home> controller = home();
        try {
            HomeShellTest.Home home = controller.get();
            layOut(home);
            ViewCompat.dispatchApplyWindowInsets(home.getWindow().getDecorView(), new WindowInsetsCompat.Builder().build());
            View lens = home.findViewById(R.id.home_search);
            lens.performClick();
            ShadowLooper.idleMainLooper();
            assertNull("no other page", shadowOf(home).getNextStartedActivity());
            View bar = home.findViewById(R.id.home_search_bar);
            assertEquals(View.VISIBLE, bar.getVisibility());
            assertEquals(View.INVISIBLE, home.findViewById(R.id.home_top_row).getVisibility());
            assertEquals("the title makes room", View.INVISIBLE, home.findViewById(R.id.home_hero).getVisibility());
            layOut(home);
            int foot = ((ViewGroup.MarginLayoutParams) bar.getLayoutParams()).topMargin + bar.getHeight();
            View results = home.findViewById(R.id.home_search_results);
            assertEquals("covers end at the field's foot and go no further", foot, results.getTop());
            assertEquals(new Rect(0, 0, results.getWidth(), results.getHeight()), results.getClipBounds());
            assertEquals("a fading edge stands at the padding", 0, results.getPaddingTop());
            home.getOnBackPressedDispatcher().onBackPressed();
            assertEquals(View.GONE, bar.getVisibility());
            assertEquals(View.VISIBLE, home.findViewById(R.id.home_top_row).getVisibility());
            assertEquals(View.VISIBLE, home.findViewById(R.id.home_hero).getVisibility());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    /** Right along the top of a TV: tabs, PC, View, search, then a wall. The remote has no other way up there. */
    @Test
    @Config(qualifiers = "w960dp-h540dp-land-television-xhdpi")
    public void onATelevisionRightWalksTheWholeTopRow() {
        ActivityController<HomeShellTest.Home> controller = home();
        try {
            HomeShellTest.Home home = controller.get();
            Object root = ReflectionHelpers.callInstanceMethod(home.getWindow().getDecorView(), "getViewRootImpl");
            ReflectionHelpers.callInstanceMethod(root, "ensureTouchMode", ReflectionHelpers.ClassParameter.from(boolean.class, false));
            home.findViewById(R.id.home_pc).setVisibility(View.VISIBLE);
            layOut(home);
            View at = home.findViewById(R.id.shell_settings);
            for (int id : new int[] {R.id.home_pc, R.id.console_view, R.id.home_search}) {
                View next = at.focusSearch(View.FOCUS_RIGHT);
                assertEquals(home.findViewById(id), next);
                at = next;
            }
            assertEquals("a wall", at, at.focusSearch(View.FOCUS_RIGHT));
        } finally {
            controller.pause().stop().destroy();
        }
    }

    /** The first card of a shelf is where a TV starts: grown and ringed, it must not touch the shelf's heading. */
    @Test
    @Config(qualifiers = "w960dp-h540dp-land-television-xhdpi")
    public void onATelevisionAFocusedCoverStaysClearOfItsHeading() throws Exception {
        ActivityController<HomeShellTest.Home> controller = home();
        try {
            HomeShellTest.Home home = controller.get();
            RecyclerView list = showHosts(home, 12, NativeApiFixtures.nativeHost("tower", "Tower", true));
            layOut(home);
            shadowOf(Looper.getMainLooper()).idle();
            layOut(home);
            boolean seen = false;
            for (int i = 0; i < list.getChildCount() && !seen; i++) {
                View shelf = list.getChildAt(i);
                View title = shelf.findViewById(R.id.shelf_title);
                ViewGroup cards = shelf.findViewById(R.id.shelf_cards);
                if (title == null || cards == null || cards.getChildCount() == 0
                        || !(title.getParent() == shelf.findViewById(R.id.shelf_head))) continue;
                seen = true;
                View card = cards.getChildAt(0);
                float density = home.getResources().getDisplayMetrics().density;
                float grown = card.getHeight() * 0.06f + 2 * density;
                int titleBottom = shelf.findViewById(R.id.shelf_head).getTop() + title.getBottom();
                assertTrue("title ends at " + titleBottom + ", the ring starts at " + (cards.getTop() - grown),
                        titleBottom <= cards.getTop() - grown);
            }
            assertTrue("a shelf was laid out", seen);
        } finally {
            controller.pause().stop().destroy();
        }
    }

    /** One game on two shelves: a refresh leaves focus on the shelf it was on, not on the first place the game is. */
    @Test
    @Config(qualifiers = "w960dp-h540dp-land-television-xhdpi")
    public void aRefreshKeepsFocusOnItsShelf() throws Exception {
        ActivityController<HomeShellTest.Home> controller = home();
        try {
            HomeShellTest.Home home = controller.get();
            showHosts(home, 3, NativeApiFixtures.nativeHost("tower", "Tower", true));
            ConsoleHomeAdapter shelves = ReflectionHelpers.getField(home, "shelves");
            int row = 0;
            String key = shelves.keyAt(new ConsoleHomeAdapter.Cursor(row, 0));
            assertEquals(row, shelves.positionOfKey(shelves.rowKey(row), key).row);
            assertEquals("a shelf that is gone: the first place it is", row, shelves.positionOfKey("shelf:gone", key).row);
        } finally {
            controller.pause().stop().destroy();
        }
    }

    /**
     * nightjar#7 (TCL C6K): scrolled, the TV's shelves drew over the hero and the top row, since nothing clipped the
     * list; and the last shelf could not reach the top. The list now clips at its own top, fades there, and has the
     * foot to bring its last shelf up.
     */
    @Test
    @Config(qualifiers = "television")
    public void onTelevisionTheListClipsAndFadesAtItsTop() {
        ActivityController<HomeShellTest.Home> controller = home();
        try {
            HomeShellTest.Home screen = controller.get();
            RecyclerView list = screen.findViewById(R.id.console_list);
            assertTrue(list instanceof HomeList);
            assertTrue("fades at its top", list.isVerticalFadingEdgeEnabled());
            float density = screen.getResources().getDisplayMetrics().density;
            assertTrue("room for the last shelf to reach the top", list.getPaddingBottom() >= Math.round(244 * density));
            list.measure(View.MeasureSpec.makeMeasureSpec(800, View.MeasureSpec.EXACTLY),
                    View.MeasureSpec.makeMeasureSpec(400, View.MeasureSpec.EXACTLY));
            list.layout(0, 168, 800, 568);
            assertEquals(new Rect(0, 0, 800, 400), list.getClipBounds());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    /** nightjar#7: a TV's grid and list ran into the overscan band; they stand back to it, the shelves as before. */
    @Test
    @Config(qualifiers = "television")
    public void onTelevisionTheGridAndListStandBackToTheOverscanEdge() {
        Resources res = context.getResources();
        int overscan = res.getDimensionPixelSize(R.dimen.shell_overscan_horizontal);
        int card = res.getDimensionPixelSize(R.dimen.cover_inset);
        assertTrue(overscan > 0);
        assertEquals(overscan - card, HomeChrome.listInset(res, GlassForm.TELEVISION, false));
        assertEquals(0, HomeChrome.listInset(res, GlassForm.TELEVISION, true));
    }

    @Test
    @Config(qualifiers = "w960dp-h540dp-land-television-xhdpi")
    public void onATelevisionDpadDownFromPcTabMovesToTheFirstAppCardInsteadOfHomeTab() throws Exception {
        ActivityController<HomeShellTest.Home> controller = home();
        try {
            HomeShellTest.Home home = controller.get();
            showHosts(home, NativeApiFixtures.nativeHost("tower", "Tower", true));
            View pcTab = home.findViewById(R.id.home_pc);
            pcTab.setVisibility(View.VISIBLE);
            layOut(home);
            TvScreensPreviewTest.leaveTouchMode(home);
            assertTrue(pcTab.requestFocus());

            View focusDown = pcTab.focusSearch(View.FOCUS_DOWN);
            org.junit.Assert.assertNotSame("D-pad Down from PC tab must not jump to Home tab",
                    home.findViewById(R.id.shell_home), focusDown);
            assertTrue("nextFocusDown points to list or its first card",
                    focusDown == list(home) || isInside(list(home), focusDown));

            assertTrue(home.dispatchKeyEvent(new KeyEvent(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_DPAD_DOWN)));
            View focused = home.getWindow().getDecorView().findFocus();
            assertNotNull("focus moved after DPAD_DOWN from PC tab", focused);
            org.junit.Assert.assertNotSame("focus did not jump to shell_home", home.findViewById(R.id.shell_home), focused);
            assertTrue("focus landed on a card inside console_list", isInside(list(home), focused));
        } finally {
            controller.pause().stop().destroy();
        }
    }
}
