package com.limelight.console;

import static com.limelight.console.HomeLibraryRowsTest.game;
import static com.limelight.nativeapi.NativeApiFixtures.library;
import static com.limelight.nativeapi.NativeApiFixtures.nativeHost;
import static com.limelight.nativeapi.NativeApiFixtures.ok;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
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
}
