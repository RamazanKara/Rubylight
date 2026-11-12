package com.limelight.console;

import static com.limelight.console.HomeLibraryRowsTest.game;
import static com.limelight.nativeapi.NativeApiFixtures.library;
import static com.limelight.nativeapi.NativeApiFixtures.nativeHost;
import static com.limelight.nativeapi.NativeApiFixtures.ok;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotEquals;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertSame;
import static org.junit.Assert.assertTrue;
import static org.robolectric.Shadows.shadowOf;

import android.app.Application;
import android.content.Context;
import android.content.Intent;
import android.content.ServiceConnection;
import android.os.Build;
import android.os.Looper;
import android.preference.PreferenceManager;
import android.view.View;
import android.view.ViewGroup;

import androidx.recyclerview.widget.RecyclerView;

import com.google.android.material.button.MaterialButton;
import com.limelight.PcViewLifecycleTest;
import com.limelight.R;
import com.limelight.computers.ComputerManagerListener;
import com.limelight.computers.ComputerManagerService;
import com.limelight.nativeapi.ConsoleRepository;
import com.limelight.nativeapi.NativeApiFixtures;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.preferences.GlPreferences;

import org.junit.After;
import org.junit.Before;
import org.junit.Rule;
import org.junit.Test;
import org.junit.rules.TemporaryFolder;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.util.ReflectionHelpers;

import java.time.Duration;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.AbstractExecutorService;
import java.util.concurrent.TimeUnit;

/** The All games screen on a TV: chips over the grid, a filter that empties, focus across a refresh, Home. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class, qualifiers = "w960dp-h540dp-television-xhdpi",
        shadows = PcViewLifecycleTest.NoNativeMoonBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
public class AllGamesScreenTest {
    @Rule public TemporaryFolder temporary = new TemporaryFolder();

    /** The repository the screen reads; the test scripts its host. */
    static ConsoleRepository repository;

    /** All games without the computer manager service, over the test's repository. */
    public static class Grid extends AllGamesActivity {
        @Override public boolean bindService(Intent intent, ServiceConnection connection, int flags) {
            return false;
        }

        @Override protected ConsoleRepository createRepository(String uniqueId) {
            return repository;
        }
    }

    /** Runs the screen's loads on the calling thread, so a test sees each one finished. */
    private static final class Direct extends AbstractExecutorService {
        private boolean shut;
        @Override public void shutdown() { shut = true; }
        @Override public List<Runnable> shutdownNow() { shut = true; return new ArrayList<>(); }
        @Override public boolean isShutdown() { return shut; }
        @Override public boolean isTerminated() { return shut; }
        @Override public boolean awaitTermination(long timeout, TimeUnit unit) { return true; }
        @Override public void execute(Runnable command) { command.run(); }
    }

    private Context context;
    private ConsoleRig rig;
    private final ComputerDetails tower = nativeHost("tower", "Tower", true);
    private ActivityController<Grid> controller;

    @Before
    public void setUp() throws Exception {
        context = RuntimeEnvironment.getApplication();
        PreferenceManager.getDefaultSharedPreferences(context).edit().clear().commit();
        GlPreferences gl = GlPreferences.readPreferences(context);
        gl.savedFingerprint = Build.FINGERPRINT;
        gl.glRenderer = "Test renderer";
        gl.writePreferences();
        rig = new ConsoleRig(temporary.newFolder());
        repository = rig.repository;
        rig.transport.reply("tower", "capabilities", ok("{\"api_version\":1,\"features\":[\"library\"],"
                + "\"permissions\":{\"list\":true,\"view\":true,\"launch\":true}}"));
        rig.transport.reply("tower", "session", ok(NativeApiFixtures.IDLE));
    }

    @After
    public void tearDown() {
        if (controller != null) controller.pause().stop().destroy();
        rig.close();
        repository = null;
        PreferenceManager.getDefaultSharedPreferences(context).edit().clear().commit();
    }

    private void tower(String... games) {
        rig.transport.reply("tower", "library", ok(library("t", games)));
    }

    private static final String DOOM = game(1, "Doom", "steam", true, null);
    private static final String HADES = game(2, "Hades", "epic", true, null);
    private static final String PORTAL = game(3, "Portal", "steam", true, null);

    private Grid open(String filter) {
        Intent intent = new Intent(context, Grid.class);
        if (filter != null) intent.putExtra(AllGamesActivity.EXTRA_FILTER, filter);
        controller = Robolectric.buildActivity(Grid.class, intent).create();
        Grid grid = controller.get();
        ReflectionHelpers.setField(grid, "worker", new Direct());
        HostFeed feed = ReflectionHelpers.getField(grid, "feed");
        ReflectionHelpers.setField(feed, "binder", binder());
        feed.update(tower);
        controller.start().resume().visible();
        settle();
        return grid;
    }

    private ComputerManagerService.ComputerManagerBinder binder() {
        return new ComputerManagerService().new ComputerManagerBinder() {
            @Override public boolean isPersistent() { return false; }
            @Override public void startPolling(ComputerManagerListener listener) { }
            @Override public void stopPolling() { }
            @Override public void invalidateStateForComputer(String uuid) { }
            @Override public String getUniqueId() { return "unique"; }
            @Override public ComputerDetails getComputer(String uuid) { return "tower".equals(uuid) ? tower : null; }
            @Override public void waitForPollingStopped() { }
        };
    }

    /** Past the reload's debounce, then the layout and the focus it posts. */
    private static void settle() {
        shadowOf(Looper.getMainLooper()).idleFor(Duration.ofMillis(300));
        shadowOf(Looper.getMainLooper()).idle();
    }

    /** Leaving and coming back reads the host again. */
    private void refresh() {
        controller.pause().resume();
        settle();
    }

    private static RecyclerView list(Grid grid) {
        return grid.findViewById(R.id.all_games_grid);
    }

    private static String focusedTitle(Grid grid) {
        RecyclerView list = list(grid);
        View focused = list.getFocusedChild();
        assertNotNull("a card has focus", focused);
        ShelfAdapter adapter = (ShelfAdapter) list.getAdapter();
        return adapter.item(list.getChildAdapterPosition(focused)).game.title;
    }

    private static View chip(Grid grid, String key) {
        ViewGroup row = grid.filterRow();
        for (int i = 0; i < row.getChildCount(); i++) {
            if (key.equals(row.getChildAt(i).getTag())) return row.getChildAt(i);
        }
        throw new AssertionError("no chip " + key);
    }

    private static boolean isInside(View parent, View child) {
        for (View v = child; v != null; v = v.getParent() instanceof View ? (View) v.getParent() : null) {
            if (v == parent) return true;
        }
        return false;
    }

    @Test
    public void onATelevisionThePageStandsOnTheSkyAndTheGridFadesUnderTheHeading() {
        tower(DOOM, HADES, PORTAL);
        Grid grid = open(null);
        assertEquals("the night sky of the other pages, not a flat canvas", View.VISIBLE,
                grid.findViewById(R.id.all_games_sky).getVisibility());
        View rows = list(grid);
        assertTrue("rows fade out at the grid's edge instead of a hard crop", rows.isVerticalFadingEdgeEnabled());
        // Over the glow's room above the first row too, so no row stands unfaded under the focused title.
        assertEquals(grid.getResources().getDimensionPixelSize(R.dimen.all_games_fade)
                        + Math.round(AllGamesActivity.GLOW_ROOM_DP * grid.getResources().getDisplayMetrics().density),
                rows.getVerticalFadingEdgeLength());
        assertEquals("the fade starts at the frame's edge", 0, rows.getPaddingTop());
        // On the real TV the bottom fade ended at the bottom padding's edge, a hard line with bright rows below it.
        assertTrue("rows fade at the top only", rows instanceof TopFadeRecyclerView);
        assertEquals(0f, ReflectionHelpers.<Float>callInstanceMethod(rows, "getBottomFadingEdgeStrength"), 0f);
        // And the sky stopped at the TV margin, a lighter frame inset from the screen's edge.
        HomeShellTest.focusAfter(grid, list(grid).getFocusedChild(), View.FOCUS_RIGHT);
        View sky = grid.findViewById(R.id.all_games_sky);
        View content = grid.findViewById(android.R.id.content);
        assertTrue("the TV margin is there", content.getPaddingLeft() > 0);
        int[] skyAt = new int[2];
        int[] contentAt = new int[2];
        sky.getLocationInWindow(skyAt);
        content.getLocationInWindow(contentAt);
        assertEquals("the sky runs edge to edge", contentAt[0], skyAt[0]);
        assertEquals(contentAt[1], skyAt[1]);
        assertEquals(content.getWidth(), sky.getWidth());
        assertEquals(content.getHeight(), sky.getHeight());
    }

    /**
     * On TV the focused poster's glow was cut square at the grid frame's top and left: the frame clipped 12 dp above the first
     * row and 24 dp beside it. The frame stands 40 dp further out there, and the list pads by as much.
     */
    @Test
    public void onATelevisionTheFocusedPostersGlowHasRoomInsideTheGridsFrame() {
        tower(DOOM, HADES, PORTAL);
        Grid grid = open(null);
        View first = list(grid).getFocusedChild();
        HomeShellTest.focusAfter(grid, first, View.FOCUS_RIGHT);
        View frame = (View) list(grid).getParent();
        int room = Math.round(40 * grid.getResources().getDisplayMetrics().density);
        int top = list(grid).getTop() + first.getTop();
        int start = list(grid).getLeft() + first.getLeft();
        assertTrue("above the first row: " + top, top >= room);
        assertTrue("beside the first poster: " + start, start >= room);
        assertTrue("and at the end", list(grid).getPaddingRight() >= room && list(grid).getRight() == frame.getWidth());
        int[] title = new int[2];
        int[] card = new int[2];
        grid.findViewById(R.id.all_games_focus).getLocationInWindow(title);
        first.getLocationInWindow(card);
        assertTrue("the first row stays under the focused title",
                card[1] >= title[1] + grid.findViewById(R.id.all_games_focus).getHeight());
    }

    /**
     * On the real TV a lower row that took focus scrolled to the frame's very top, 40 dp up in the focus line: its title stood
     * over the poster and the glow was cut square there. Walking down and back up, the focused row keeps that room.
     */
    @Test
    public void onATelevisionALowerFocusedRowStaysUnderTheFocusLineWithItsGlowRoom() {
        String[] games = new String[40];
        for (int i = 0; i < games.length; i++) games[i] = game(i + 1, "Game " + (i + 1), "steam", true, null);
        tower(games);
        Grid grid = open(null);
        RecyclerView list = list(grid);
        View focusLine = grid.findViewById(R.id.all_games_focus);
        int firstTop = list.getTop() + list.getFocusedChild().getTop();
        int room = Math.round(AllGamesActivity.GLOW_ROOM_DP * grid.getResources().getDisplayMetrics().density);
        assertTrue("the first row has the glow's room: " + firstTop, firstTop >= room);
        boolean scrolled = false;
        for (int direction : new int[] {View.FOCUS_DOWN, View.FOCUS_UP}) {
            for (int step = 0; step < 4; step++) {
                View next = HomeShellTest.focusAfter(grid, list.getFocusedChild(), direction);
                assertNotNull("step " + step, next);
                assertTrue(isInside(list, next));
                next.requestFocus();
                shadowOf(Looper.getMainLooper()).idleFor(Duration.ofSeconds(1));
                HomeShellTest.focusAfter(grid, list.getFocusedChild(), direction);
                scrolled |= list.computeVerticalScrollOffset() > 0;
                View card = list.getFocusedChild();
                int[] line = new int[2];
                int[] at = new int[2];
                focusLine.getLocationInWindow(line);
                card.getLocationInWindow(at);
                int top = list.getTop() + card.getTop();
                String where = (direction == View.FOCUS_DOWN ? "down " : "up ") + step + ": card at " + top + " in the frame";
                assertTrue(where + ", the first row's place " + firstTop, top >= firstTop);
                // As drawn, with the focus scale: what a person sees stays under the focus line.
                assertTrue(where + ", drawn at " + at[1] + ", under the focus line ending at " + (line[1] + focusLine.getHeight()),
                        at[1] >= line[1] + focusLine.getHeight());
            }
        }
        assertTrue("the grid scrolled at all", scrolled);
    }

    /**
     * On the real TV the frame ended 120 px above the screen's bottom, and every focused row's ring and glow were cut there.
     * The frame now reaches the TV's safe edge, and a poster is at most as tall as leaves its focused row as much room below
     * as the first row has above, where the glow fits: walking down and back up, every focused row fits whole.
     */
    @Test
    public void onATelevisionAFocusedRowsRingAndGlowFitTheFrame() {
        String[] games = new String[40];
        for (int i = 0; i < games.length; i++) games[i] = game(i + 1, "Game " + (i + 1), "steam", true, null);
        tower(games);
        Grid grid = open(null);
        RecyclerView list = list(grid);
        View frame = (View) list.getParent();
        int screen = grid.getWindow().getDecorView().getHeight();
        int[] at = new int[2];
        frame.getLocationInWindow(at);
        assertEquals("the frame ends at the safe edge", screen - Math.round(screen * AllGamesActivity.SAFE_EDGE),
                at[1] + frame.getHeight());
        int above = list.getFocusedChild().getTop();
        int fitRoom = Math.round(AllGamesActivity.FIT_ROOM_DP * grid.getResources().getDisplayMetrics().density);
        View cover = list.getFocusedChild().findViewById(R.id.console_cover_frame);
        assertEquals("a poster stays 2:3", cover.getWidth() * 3 / 2, cover.getHeight(), 2);
        for (int direction : new int[] {View.FOCUS_DOWN, View.FOCUS_RIGHT, View.FOCUS_UP}) {
            for (int step = 0; step < (direction == View.FOCUS_RIGHT ? 3 : 6); step++) {
                View card = list.getFocusedChild();
                String where = direction + " step " + step + ": card " + card.getTop() + ".." + card.getBottom()
                        + " in a frame of " + list.getHeight();
                assertTrue(where + ", room above " + above, card.getTop() >= above);
                assertTrue(where, list.getHeight() - card.getBottom() >= above + fitRoom);
                View next = HomeShellTest.focusAfter(grid, card, direction);
                assertNotNull(where, next);
                next.requestFocus();
                shadowOf(Looper.getMainLooper()).idleFor(Duration.ofSeconds(1));
            }
        }
        // The last row comes up to the same place.
        list.scrollToPosition(39);
        shadowOf(Looper.getMainLooper()).idleFor(Duration.ofSeconds(1));
        list.getLayoutManager().findViewByPosition(39).requestFocus();
        shadowOf(Looper.getMainLooper()).idleFor(Duration.ofSeconds(1));
        View last = list.getFocusedChild();
        assertTrue("last row " + last.getTop() + ".." + last.getBottom() + " of " + list.getHeight(),
                list.getHeight() - last.getBottom() >= above + fitRoom && last.getTop() >= above);
    }

    /**
     * A handheld in the forced TV layout rotates without the page being made again, and the frame kept the height it had
     * in the old orientation: focused rows ran off the screen. It ends at the new screen's safe edge.
     */
    @Test
    public void onATelevisionLayoutARotationKeepsTheFrameAtTheSafeEdge() {
        String[] games = new String[20];
        for (int i = 0; i < games.length; i++) games[i] = game(i + 1, "Game " + (i + 1), "steam", true, null);
        tower(games);
        // As the manifest declares for the real screen: a rotation reaches onConfigurationChanged, no recreate.
        android.content.pm.ActivityInfo info = new android.content.pm.ActivityInfo();
        info.name = Grid.class.getName();
        info.packageName = context.getPackageName();
        info.configChanges = android.content.pm.ActivityInfo.CONFIG_ORIENTATION | android.content.pm.ActivityInfo.CONFIG_SCREEN_SIZE
                | android.content.pm.ActivityInfo.CONFIG_SMALLEST_SCREEN_SIZE | android.content.pm.ActivityInfo.CONFIG_SCREEN_LAYOUT;
        shadowOf(context.getPackageManager()).addOrUpdateActivity(info);
        Grid grid = open(null);
        View decor = grid.getWindow().getDecorView();
        int before = decor.getHeight();
        RuntimeEnvironment.setQualifiers("+w540dp-h960dp-port");
        controller.configurationChange(RuntimeEnvironment.getApplication().getResources().getConfiguration());
        settle();
        assertSame("rotated in place", grid, controller.get());
        int screen = decor.getHeight();
        assertNotEquals("the window turned", before, screen);
        View frame = (View) list(grid).getParent();
        int[] at = new int[2];
        frame.getLocationInWindow(at);
        assertEquals("the frame ends at the new safe edge", screen - Math.round(screen * AllGamesActivity.SAFE_EDGE),
                at[1] + frame.getHeight());
    }

    /** A phone keeps its frame and posters as wide as their columns: the TV's fit is the TV's. */
    @Test
    @Config(qualifiers = "w411dp-h891dp-xxhdpi")
    public void onAPhoneTheGridKeepsItsFrameAndFullWidthPosters() {
        String[] games = new String[12];
        for (int i = 0; i < games.length; i++) games[i] = game(i + 1, "Game " + (i + 1), "steam", true, null);
        tower(games);
        Grid grid = open(null);
        RecyclerView list = list(grid);
        View frame = (View) list.getParent();
        assertEquals(0, ((ViewGroup.MarginLayoutParams) frame.getLayoutParams()).bottomMargin);
        assertEquals(((View) frame.getParent()).getHeight(), frame.getBottom());
        int columns = ((androidx.recyclerview.widget.GridLayoutManager) list.getLayoutManager()).getSpanCount();
        int content = list.getWidth() - list.getPaddingLeft() - list.getPaddingRight();
        View card = list.getChildAt(0);
        ViewGroup.MarginLayoutParams margins = (ViewGroup.MarginLayoutParams) card.getLayoutParams();
        assertEquals(content / columns, card.getWidth() + margins.leftMargin + margins.rightMargin);
    }

    /** The frame's glow room stands over the focus line and filters: a touch there was the grid's on touch-capable TVs. */
    @Test
    public void onATelevisionATouchInTheGlowRoomIsForWhatLiesUnderIt() {
        tower(DOOM, HADES, PORTAL);
        Grid grid = open(null);
        View frame = (View) list(grid).getParent();
        int room = Math.round(AllGamesActivity.GLOW_ROOM_DP * grid.getResources().getDisplayMetrics().density);
        List<String> got = new ArrayList<>();
        list(grid).setOnTouchListener((v, e) -> got.add("grid") && false);
        grid.findViewById(R.id.all_games_focus).setOnTouchListener((v, e) -> got.add("focus") && false);
        grid.findViewById(R.id.all_games_filters_scroll).setOnTouchListener((v, e) -> got.add("filters") && false);
        int[] at = new int[2];
        frame.getLocationInWindow(at);
        View decor = grid.getWindow().getDecorView();

        touch(decor, at[0] + frame.getWidth() / 2f, at[1] + room / 2f);
        assertFalse("the grid takes none of it: " + got, got.contains("grid"));
        assertTrue("what lies under it does: " + got, got.contains("focus") || got.contains("filters"));

        got.clear();
        touch(decor, at[0] + frame.getWidth() / 2f, at[1] + room + 4);
        assertTrue("below the strip it is the grid's: " + got, got.contains("grid"));
    }

    private static void touch(View root, float x, float y) {
        long now = android.os.SystemClock.uptimeMillis();
        for (int action : new int[] {android.view.MotionEvent.ACTION_DOWN, android.view.MotionEvent.ACTION_UP}) {
            android.view.MotionEvent event = android.view.MotionEvent.obtain(now, now, action, x, y, 0);
            event.setSource(android.view.InputDevice.SOURCE_TOUCHSCREEN);
            root.dispatchTouchEvent(event);
            event.recycle();
        }
    }

    @Test
    public void theFirstGameHasFocusAndTheDpadMovesBetweenTheChipsAndTheGrid() {
        tower(DOOM, HADES, PORTAL);
        Grid grid = open(null);
        assertEquals("Doom", focusedTitle(grid));
        assertTrue(chip(grid, HomeModel.FILTER_ALL).isSelected());

        View first = list(grid).getFocusedChild();
        View up = HomeShellTest.focusAfter(grid, first, View.FOCUS_UP);
        assertNotNull("up from the first row", up);
        assertTrue("up from the first row reaches the chips", isInside(grid.filterRow(), up));
        View down = HomeShellTest.focusAfter(grid, chip(grid, HomeModel.FILTER_ALL), View.FOCUS_DOWN);
        assertNotNull("down from the chips", down);
        assertTrue("down from the chips is the grid again", isInside(list(grid), down));
    }

    @Test
    public void aChosenFilterThatEmptiesFallsBackToAll() {
        tower(DOOM, HADES, PORTAL);
        Grid grid = open("store:epic");
        assertEquals("store:epic", grid.filter());
        assertEquals(1, list(grid).getAdapter().getItemCount());

        tower(DOOM, PORTAL);
        refresh();
        assertEquals(HomeModel.FILTER_ALL, grid.filter());
        assertTrue(chip(grid, HomeModel.FILTER_ALL).isSelected());
        assertEquals(2, list(grid).getAdapter().getItemCount());
    }

    @Test
    public void theFocusedGameKeepsFocusWhenARefreshAddsOneBeforeIt() {
        tower(DOOM, HADES, PORTAL);
        Grid grid = open(null);
        RecyclerView list = list(grid);
        list.findViewHolderForAdapterPosition(1).itemView.requestFocus();
        assertEquals("Hades", focusedTitle(grid));

        tower(game(4, "Celeste", "steam", true, null), DOOM, HADES, PORTAL);
        refresh();
        assertEquals(4, list.getAdapter().getItemCount());
        assertEquals("Hades", focusedTitle(grid));
    }

    @Test
    public void aCountThatChangesKeepsTheChipsAndSaysTheNewCount() {
        tower(DOOM, HADES, PORTAL);
        Grid grid = open(null);
        View steam = chip(grid, "store:steam");
        assertTrue(((MaterialButton) steam).getText().toString().endsWith("2"));

        tower(game(4, "Celeste", "steam", true, null), DOOM, HADES, PORTAL);
        refresh();
        assertSame("the chip is kept", steam, chip(grid, "store:steam"));
        assertTrue(((MaterialButton) steam).getText().toString().endsWith("3"));
        assertTrue(steam.getContentDescription().toString().contains("3"));
    }

    @Test
    public void homeInTheBarClosesAllGames() {
        tower(DOOM);
        Grid grid = open(null);
        grid.findViewById(R.id.shell_home).performClick();
        assertTrue(grid.isFinishing());
        assertFalse(shadowOf(grid).getNextStartedActivity() != null);
    }

    /** Home's top row on a TV: the tabs in glass, then the PC capsule naming the PC and Search, which a remote reaches along it. */
    @Test
    public void onATelevisionTheTopRowHasHomesPcCapsuleAndSearch() {
        tower(DOOM, HADES, PORTAL);
        Grid grid = open(null);
        View row = grid.findViewById(R.id.all_games_top_row);
        View capsule = grid.findViewById(R.id.all_games_pc);
        View search = grid.findViewById(R.id.all_games_search);
        assertEquals(View.VISIBLE, capsule.getVisibility());
        assertEquals("Tower", ((android.widget.TextView) capsule.findViewById(R.id.home_pc_name)).getText().toString());
        assertEquals("Tower", capsule.getContentDescription().toString());
        assertTrue(isInside(row, capsule));
        assertTrue(isInside(row, search));
        assertTrue("the tabs stand in glass, as on Home", isInside(grid.findViewById(R.id.all_games_tabs), grid.findViewById(R.id.shell_bar)));
        ViewGroup tabs = grid.findViewById(R.id.shell_bar_items);
        assertEquals("right past the last tab is the capsule", R.id.all_games_pc,
                tabs.getChildAt(tabs.getChildCount() - 1).getNextFocusRightId());
        assertEquals(R.id.all_games_search, capsule.getNextFocusRightId());

        // One PC: the capsule opens its options, the PCs page's list.
        capsule.performClick();
        shadowOf(Looper.getMainLooper()).idle();
        android.app.Dialog options = org.robolectric.shadows.ShadowDialog.getLatestDialog();
        assertTrue("the PC's options", options instanceof com.limelight.ui.ActionSheet && options.isShowing());
        options.dismiss();
    }

    /** Search on the page: the field takes the top row's place, the grid narrows as one types, and Back brings every game back. */
    @Test
    public void searchNarrowsTheGridAndBackBringsEveryGameBack() {
        tower(DOOM, HADES, PORTAL);
        Grid grid = open(null);
        RecyclerView list = list(grid);
        View search = grid.findViewById(R.id.all_games_search);
        View bar = grid.findViewById(R.id.home_search_bar);
        android.widget.EditText field = grid.findViewById(R.id.home_search_field);
        assertEquals(View.GONE, bar.getVisibility());

        search.performClick();
        assertEquals(View.VISIBLE, bar.getVisibility());
        assertEquals("the field takes the row's place", View.INVISIBLE, grid.findViewById(R.id.all_games_top_row).getVisibility());
        assertTrue(field.hasFocus());

        field.setText("por");
        shadowOf(Looper.getMainLooper()).idle();
        assertEquals(1, list.getAdapter().getItemCount());
        assertEquals("Portal", ((ShelfAdapter) list.getAdapter()).item(0).game.title);
        field.setText("zelda");
        shadowOf(Looper.getMainLooper()).idle();
        assertEquals(0, list.getAdapter().getItemCount());
        android.widget.TextView empty = grid.findViewById(R.id.all_games_empty);
        assertEquals(View.VISIBLE, empty.getVisibility());
        assertEquals(grid.getString(R.string.library_no_matches), empty.getText().toString());

        // A search keeps to the chosen filter.
        field.setText("");
        grid.findViewById(R.id.all_games_filters).findViewWithTag("store:steam").performClick();
        field.setText("o");
        shadowOf(Looper.getMainLooper()).idle();
        assertEquals("Doom and Portal are on Steam", 2, list.getAdapter().getItemCount());

        grid.getOnBackPressedDispatcher().onBackPressed();
        shadowOf(Looper.getMainLooper()).idle();
        assertEquals(View.GONE, bar.getVisibility());
        assertEquals(View.VISIBLE, grid.findViewById(R.id.all_games_top_row).getVisibility());
        assertEquals("every Steam game again", 2, list.getAdapter().getItemCount());
        assertFalse("Back closed the search, not the screen", grid.isFinishing());
        assertEquals(View.GONE, grid.findViewById(R.id.all_games_empty).getVisibility());
    }

    /** A phone keeps its tabs across the top; the capsule and Search stand at the end of the title's line. */
    @Test
    @Config(qualifiers = "w411dp-h891dp-xxhdpi")
    public void onAPhoneTheCapsuleAndSearchStandOnTheTitlesLine() {
        tower(DOOM, HADES);
        Grid grid = open(null);
        View titleRow = grid.findViewById(R.id.all_games_title_row);
        assertTrue(isInside(titleRow, grid.findViewById(R.id.all_games_pc)));
        assertTrue(isInside(titleRow, grid.findViewById(R.id.all_games_search)));
        assertTrue(isInside(grid.findViewById(R.id.all_games_top_row), grid.findViewById(R.id.shell_bar)));
        assertEquals(View.VISIBLE, grid.findViewById(R.id.all_games_pc).getVisibility());

        grid.findViewById(R.id.all_games_search).performClick();
        assertEquals("the title's line keeps its title", View.INVISIBLE, grid.findViewById(R.id.all_games_tools).getVisibility());
        assertEquals(View.VISIBLE, grid.findViewById(R.id.all_games_title).getVisibility());
        grid.getOnBackPressedDispatcher().onBackPressed();
        assertEquals(View.VISIBLE, grid.findViewById(R.id.all_games_tools).getVisibility());
    }
}
