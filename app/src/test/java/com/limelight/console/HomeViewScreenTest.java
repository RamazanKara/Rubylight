package com.limelight.console;

import static com.limelight.nativeapi.NativeApiFixtures.library;
import static com.limelight.nativeapi.NativeApiFixtures.nativeHost;
import static com.limelight.nativeapi.NativeApiFixtures.ok;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;
import static org.robolectric.Shadows.shadowOf;

import android.app.Application;
import android.content.Context;
import android.content.SharedPreferences;
import android.os.Build;
import android.os.Looper;
import android.preference.PreferenceManager;
import android.view.KeyEvent;
import android.view.View;
import android.view.ViewGroup;
import android.widget.TextView;

import androidx.recyclerview.widget.GridLayoutManager;
import androidx.recyclerview.widget.RecyclerView;

import com.google.android.material.button.MaterialButton;
import com.limelight.PcViewLifecycleTest;
import com.limelight.R;
import com.limelight.nativeapi.NativeApiFixtures;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.preferences.GlPreferences;
import com.limelight.ui.ActionSheet;

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
import org.robolectric.shadows.ShadowDialog;
import org.robolectric.util.ReflectionHelpers;

import java.io.File;
import java.util.ArrayList;
import java.util.Collections;
import java.util.HashSet;
import java.util.List;

/** Home's View button: the layout, grouping and card size it offers, and Home built the way they were chosen. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class, shadows = PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class HomeViewScreenTest {
    @Rule public TemporaryFolder temporary = new TemporaryFolder();

    private Context context;
    private SharedPreferences prefs;

    @Before
    public void setUp() throws Exception {
        context = RuntimeEnvironment.getApplication();
        prefs = PreferenceManager.getDefaultSharedPreferences(context);
        prefs.edit().clear().commit();
        GlPreferences gl = GlPreferences.readPreferences(context);
        gl.savedFingerprint = Build.FINGERPRINT;
        gl.glRenderer = "Test renderer";
        gl.writePreferences();
        File directory = new File(context.getFilesDir(), "console-library");
        assertTrue(directory.isDirectory() || directory.mkdirs());
        assertTrue(new File(directory, "tower.json").createNewFile());
    }

    @After
    public void tearDown() {
        prefs.edit().clear().commit();
    }

    private static ActivityController<HomeShellTest.Home> home() {
        return Robolectric.buildActivity(HomeShellTest.Home.class).setup().visible();
    }

    /** Tower's library: three Steam games and two Epic ones, all installed. */
    private HomeModel storeModel() throws Exception {
        ConsoleRig rig = new ConsoleRig(temporary.newFolder());
        try {
            rig.transport.reply("tower", "capabilities", ok("{\"api_version\":1,\"features\":[\"library\"],"
                    + "\"permissions\":{\"list\":true,\"view\":true,\"launch\":true}}"));
            rig.transport.reply("tower", "library", ok(library("t",
                    HomeLibraryRowsTest.game(1, "Portal", "steam", true, null),
                    HomeLibraryRowsTest.game(2, "Hades", "epic", true, null),
                    HomeLibraryRowsTest.game(3, "Celeste", "steam", true, null),
                    HomeLibraryRowsTest.game(4, "Alan Wake", "epic", true, null),
                    HomeLibraryRowsTest.game(5, "Doom", "steam", true, null),
                    HomeLibraryRowsTest.game(6, "Desktop", null, true, null))));
            rig.transport.reply("tower", "session", ok(NativeApiFixtures.IDLE));
            List<ComputerDetails> hosts = Collections.singletonList(nativeHost("tower", "Tower", true));
            return HomeModel.of(rig.repository.refresh(hosts), new HashSet<>(Collections.singletonList("tower")));
        } finally {
            rig.close();
        }
    }

    private static void layout(View list, int width, int height) {
        list.measure(View.MeasureSpec.makeMeasureSpec(width, View.MeasureSpec.EXACTLY),
                View.MeasureSpec.makeMeasureSpec(height, View.MeasureSpec.EXACTLY));
        list.layout(0, 0, width, height);
    }

    private static List<String> headers(RecyclerView list) {
        List<String> titles = new ArrayList<>();
        for (int i = 0; i < list.getChildCount(); i++) {
            View child = list.getChildAt(i);
            if (child instanceof TextView) titles.add(((TextView) child).getText().toString());
        }
        return titles;
    }

    private static ActionSheet openSheet() {
        shadowOf(Looper.getMainLooper()).idle();
        android.app.Dialog dialog = ShadowDialog.getLatestDialog();
        return dialog instanceof ActionSheet && dialog.isShowing() ? (ActionSheet) dialog : null;
    }

    private static MaterialButton button(View view, String label) {
        if (view instanceof MaterialButton && label.contentEquals(((MaterialButton) view).getText())) {
            return (MaterialButton) view;
        }
        if (view instanceof ViewGroup) {
            ViewGroup group = (ViewGroup) view;
            for (int i = 0; i < group.getChildCount(); i++) {
                MaterialButton found = button(group.getChildAt(i), label);
                if (found != null) return found;
            }
        }
        return null;
    }

    @Test
    public void aPhoneOpensOnShelvesByStore() throws Exception {
        HomeShellTest.Home home = home().get();
        assertEquals(HomeView.defaults(false), home.view());
        RecyclerView list = home.findViewById(R.id.console_list);
        assertFalse(list.getLayoutManager() instanceof GridLayoutManager);
        assertNotNull(ReflectionHelpers.getField(home, "shelves"));
    }

    @Test
    @Config(qualifiers = "w960dp-h540dp-land-television-xhdpi")
    public void aTvKeepsItsShelvesPerStore() {
        HomeShellTest.Home home = home().get();
        assertEquals(HomeView.defaults(true), home.view());
        RecyclerView list = home.findViewById(R.id.console_list);
        assertTrue(list.getLayoutManager() instanceof ShelfLayoutManager);
        assertNull(ReflectionHelpers.getField(home, "adapter"));
    }

    @Test
    @Config(qualifiers = "w960dp-h540dp-land-television-xhdpi")
    public void aTvGridScrollsDownWithPostersUnderEachStore() throws Exception {
        prefs.edit().putString(HomeView.KEY_LAYOUT, "grid").commit();
        HomeShellTest.Home home = home().get();
        RecyclerView list = home.findViewById(R.id.console_list);
        assertTrue(list.getLayoutManager() instanceof GridLayoutManager);
        assertEquals(RecyclerView.VERTICAL, ((GridLayoutManager) list.getLayoutManager()).getOrientation());
        assertNull(ReflectionHelpers.getField(home, "shelves"));
        ConsoleHomeListAdapter adapter = ReflectionHelpers.getField(home, "adapter");
        adapter.submit(storeModel(), home.view().group);
        layout(list, 1920, 1080);
        List<String> headers = headers(list);
        assertEquals(java.util.Arrays.asList("Steam", "Epic Games"), headers.subList(0, 2));
        assertFalse(headers.contains(home.getString(R.string.console_section_library)));
        int portal = adapter.positionOfKey(adapter.keyAt(adapter.firstCard()));
        RecyclerView.ViewHolder holder = list.findViewHolderForAdapterPosition(portal);
        assertNotNull(holder);
        // The shelves' poster, which the hero names, not the phone's captioned cover.
        assertTrue(holder instanceof ShelfAdapter.CardHolder);
        assertNull(holder.itemView.findViewById(R.id.console_title));
    }

    @Test
    @Config(qualifiers = "w960dp-h540dp-land-television-xhdpi")
    public void aTvGridOfOneLibraryHasOneHeading() throws Exception {
        prefs.edit().putString(HomeView.KEY_LAYOUT, "grid").putString(HomeView.KEY_GROUP, "none").commit();
        HomeShellTest.Home home = home().get();
        ConsoleHomeListAdapter adapter = ReflectionHelpers.getField(home, "adapter");
        adapter.submit(storeModel(), home.view().group);
        RecyclerView list = home.findViewById(R.id.console_list);
        layout(list, 1920, 1080);
        // The library under one heading, then the PCs: no store's.
        assertEquals(java.util.Arrays.asList(home.getString(R.string.console_section_library),
                home.getString(R.string.console_section_hosts)), headers(list));
    }

    @Test
    @Config(qualifiers = "w960dp-h540dp-land-television-xhdpi")
    public void aTvGridByTypeKeepsTheAppsApartFromTheGames() throws Exception {
        prefs.edit().putString(HomeView.KEY_LAYOUT, "grid").putString(HomeView.KEY_GROUP, "type").commit();
        HomeShellTest.Home home = home().get();
        ConsoleHomeListAdapter adapter = ReflectionHelpers.getField(home, "adapter");
        adapter.submit(storeModel(), home.view().group);
        RecyclerView list = home.findViewById(R.id.console_list);
        layout(list, 1920, 1080);
        // The PCs follow below the fold.
        assertEquals(java.util.Arrays.asList(home.getString(R.string.console_row_games),
                home.getString(R.string.console_row_other)), headers(list).subList(0, 2));
        assertFalse(headers(list).contains(home.getString(R.string.console_section_library)));
    }

    @Test
    @Config(qualifiers = "w960dp-h540dp-land-television-xhdpi")
    public void tvShelvesByTypeAreGamesThenOtherApps() throws Exception {
        prefs.edit().putString(HomeView.KEY_GROUP, "type").commit();
        HomeShellTest.Home home = home().get();
        ConsoleHomeAdapter shelves = ReflectionHelpers.getField(home, "shelves");
        shelves.submit(storeModel());
        List<String> keys = new java.util.ArrayList<>();
        for (int i = 0; i < shelves.rowCount(); i++) keys.add(shelves.rowKey(i));
        assertEquals(java.util.Arrays.asList("shelf:games", "shelf:other", "shelf:hosts"), keys);
        assertEquals(5, shelves.cardCount(0));
        assertEquals(1, shelves.cardCount(1));
    }

    @Test
    @Config(qualifiers = "w960dp-h540dp-land-television-xhdpi")
    public void largerCardsMeanFewerColumnsOnTheTvGrid() {
        prefs.edit().putString(HomeView.KEY_LAYOUT, "grid").putString(HomeView.KEY_SIZE, "small").commit();
        HomeShellTest.Home small = home().get();
        RecyclerView list = small.findViewById(R.id.console_list);
        layout(list, 1920, 1080);
        shadowOf(Looper.getMainLooper()).idle();
        int smallColumns = ((GridLayoutManager) list.getLayoutManager()).getSpanCount();

        prefs.edit().putString(HomeView.KEY_SIZE, "large").commit();
        HomeShellTest.Home large = home().get();
        list = large.findViewById(R.id.console_list);
        layout(list, 1920, 1080);
        shadowOf(Looper.getMainLooper()).idle();
        int largeColumns = ((GridLayoutManager) list.getLayoutManager()).getSpanCount();
        assertTrue(smallColumns + " small against " + largeColumns + " large", smallColumns > largeColumns);
    }

    /** Each List line under Home's library, top to bottom, with the line under its title. */
    private static List<String> lines(RecyclerView list) {
        List<String> found = new ArrayList<>();
        for (int i = 0; i < list.getChildCount(); i++) {
            View child = list.getChildAt(i);
            RecyclerView.ViewHolder holder = list.getChildViewHolder(child);
            if (holder.getItemViewType() != ConsoleHomeListAdapter.LIST_GAME) continue;
            found.add(((TextView) child.findViewById(R.id.console_title)).getText() + " / "
                    + ((TextView) child.findViewById(R.id.console_subtitle)).getText());
        }
        return found;
    }

    private void showsTheLibraryAsLines(int width, int height) throws Exception {
        prefs.edit().putString(HomeView.KEY_LAYOUT, "list").putString(HomeView.KEY_GROUP, "none").commit();
        HomeShellTest.Home home = home().get();
        assertEquals(HomeView.Layout.LIST, home.view().layout);
        RecyclerView list = home.findViewById(R.id.console_list);
        assertTrue(list.getLayoutManager() instanceof GridLayoutManager);
        ConsoleHomeListAdapter adapter = ReflectionHelpers.getField(home, "adapter");
        adapter.submit(storeModel(), home.view().group);
        layout(list, width, height);
        List<String> lines = lines(list);
        assertTrue(lines.toString(), lines.size() >= 3);
        // The fixture's host says nothing about playing: the store alone.
        assertTrue(lines.toString(), lines.contains("Celeste / Steam"));
        assertTrue(lines.toString(), lines.contains("Hades / Epic Games"));
        // A line spans the width, so each is under the one before it and the D-pad walks down them.
        int first = adapter.positionOfKey(adapter.keyAt(adapter.firstCard()));
        View top = list.findViewHolderForAdapterPosition(first).itemView;
        View next = list.findViewHolderForAdapterPosition(first + 1).itemView;
        assertEquals(top.getLeft(), next.getLeft());
        assertTrue(next.getTop() >= top.getBottom());
        assertTrue(top.isFocusable());
        top.requestFocus();
        assertEquals(next, top.focusSearch(View.FOCUS_DOWN));
    }

    @Test
    public void aPhoneListShowsALinePerGame() throws Exception {
        showsTheLibraryAsLines(1080, 2200);
    }

    @Test
    @Config(qualifiers = "w960dp-h540dp-land-television-xhdpi")
    public void aTvListShowsALinePerGame() throws Exception {
        showsTheLibraryAsLines(1920, 1080);
    }

    @Test
    @Config(qualifiers = "w960dp-h540dp-land-television-xhdpi")
    public void onATvAListLineTakesXMenuAndTheHeldDpadLikeAPoster() throws Exception {
        prefs.edit().putString(HomeView.KEY_LAYOUT, "list").putString(HomeView.KEY_GROUP, "none").commit();
        HomeShellTest.Home home = home().get();
        RecyclerView list = home.findViewById(R.id.console_list);
        List<String> menus = new ArrayList<>();
        ConsoleHomeAdapter.Listener recorder = (ConsoleHomeAdapter.Listener) java.lang.reflect.Proxy.newProxyInstance(
                getClass().getClassLoader(), new Class<?>[] {ConsoleHomeAdapter.Listener.class}, (proxy, method, args) -> {
                    if (method.getName().equals("onGameMenu")) menus.add(((HomeModel.GameCard) args[0]).title);
                    return method.getReturnType() == boolean.class ? false : null;
                });
        ConsoleHomeListAdapter adapter = new ConsoleHomeListAdapter(home, null, uuid -> null, recorder, true);
        adapter.setListLayout(true);
        // Home's span lookup reads its adapter field: this one, with a listener that records.
        ReflectionHelpers.setField(home, "adapter", adapter);
        list.setAdapter(adapter);
        adapter.submit(storeModel(), home.view().group);
        layout(list, 1920, 1080);
        View line = list.findViewHolderForAdapterPosition(adapter.firstCard()).itemView;
        line.requestFocus();
        // Y is the key the hint names (Nightjar #81: left to Android it is Space, which starts the game).
        for (int key : new int[] {KeyEvent.KEYCODE_BUTTON_X, KeyEvent.KEYCODE_MENU, KeyEvent.KEYCODE_BUTTON_Y, KeyEvent.KEYCODE_F}) {
            assertTrue(line.dispatchKeyEvent(new KeyEvent(KeyEvent.ACTION_DOWN, key)));
            assertTrue(line.dispatchKeyEvent(new KeyEvent(KeyEvent.ACTION_UP, key)));
        }
        assertEquals("X, Menu, Y and F open the game's menu", 4, menus.size());
        // A tap moves one line; a held D-pad is fast travel, as on the shelves.
        line.dispatchKeyEvent(new KeyEvent(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_DPAD_DOWN));
        assertFalse(adapter.isFastTravel());
        line.dispatchKeyEvent(new KeyEvent(0, 0, KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_DPAD_DOWN, 3));
        assertTrue(adapter.isFastTravel());
    }

    @Test
    public void aListLineOfAPcThatIsAwaySaysSoAloneAndRunsNothing() throws Exception {
        ConsoleRig rig = new ConsoleRig(temporary.newFolder());
        HomeModel model;
        try {
            rig.transport.reply("tower", "capabilities", ok("{\"api_version\":1,\"features\":[\"library\"],"
                    + "\"permissions\":{\"list\":true,\"view\":true,\"launch\":true}}"));
            rig.transport.reply("tower", "library", ok(library("t", HomeLibraryRowsTest.game(6, "Desktop", null, true, null))));
            // Its last answer: Desktop running.
            rig.transport.reply("tower", "session", ok(NativeApiFixtures.session("streaming", "you",
                    "{\"install_id\":\"i6\",\"app_id\":6,\"title\":\"Desktop\"}", "[]")));
            List<ComputerDetails> hosts = Collections.singletonList(nativeHost("tower", "Tower", true));
            model = HomeModel.of(rig.repository.refresh(hosts), new HashSet<>(Collections.singletonList("tower")));
        } finally {
            rig.close();
        }
        prefs.edit().putString(HomeView.KEY_LAYOUT, "list").putString(HomeView.KEY_GROUP, "none").commit();
        HomeShellTest.Home home = home().get();
        RecyclerView list = home.findViewById(R.id.console_list);
        for (boolean away : new boolean[] {false, true}) {
            ComputerDetails tower = nativeHost("tower", "Tower", true);
            if (away) tower.state = ComputerDetails.State.OFFLINE;
            ConsoleHomeAdapter.Listener none = (ConsoleHomeAdapter.Listener) java.lang.reflect.Proxy.newProxyInstance(
                    getClass().getClassLoader(), new Class<?>[] {ConsoleHomeAdapter.Listener.class},
                    (proxy, method, args) -> method.getReturnType() == boolean.class ? false : null);
            ConsoleHomeListAdapter adapter = new ConsoleHomeListAdapter(home, null, uuid -> tower, none);
            adapter.setListLayout(true);
            ReflectionHelpers.setField(home, "adapter", adapter);
            list.setAdapter(adapter);
            adapter.submit(model, HomeView.Group.NONE);
            layout(list, 1080, 2200);
            View line = null;
            for (int i = 0; i < list.getChildCount(); i++) {
                View child = list.getChildAt(i);
                if (list.getChildViewHolder(child).getItemViewType() == ConsoleHomeListAdapter.LIST_GAME) line = child;
            }
            assertNotNull("the library's line", line);
            TextView badge = line.findViewById(R.id.console_badge);
            TextView subtitle = line.findViewById(R.id.console_subtitle);
            if (away) {
                assertEquals("no separator before it", home.getString(R.string.console_badge_asleep), subtitle.getText().toString());
                assertEquals("nothing runs on a PC that is away", View.GONE, badge.getVisibility());
                assertEquals("Desktop, " + home.getString(R.string.console_badge_asleep), line.getContentDescription().toString());
            } else {
                assertEquals("", subtitle.getText().toString());
                assertEquals(home.getString(R.string.console_badge_running), badge.getText().toString());
                assertEquals("Desktop, " + home.getString(R.string.console_badge_running), line.getContentDescription().toString());
            }
        }
    }

    @Test
    public void aGameWithoutACoverShowsItsTitleNotTheHostsPlaceholder() throws Exception {
        ConsoleRig rig = new ConsoleRig(temporary.newFolder());
        HomeModel model;
        try {
            rig.transport.reply("tower", "capabilities", ok("{\"api_version\":1,\"features\":[\"library\"],"
                    + "\"permissions\":{\"list\":true,\"view\":true,\"launch\":true}}"));
            // Portal has a cover; the host sends none for Notes.
            rig.transport.reply("tower", "library", ok(library("t", NativeApiFixtures.game("t-portal", 11, "Portal", null, false),
                    HomeLibraryRowsTest.game(12, "Notes", null, true, null))));
            rig.transport.reply("tower", "session", ok(NativeApiFixtures.IDLE));
            List<ComputerDetails> hosts = Collections.singletonList(nativeHost("tower", "Tower", true));
            model = HomeModel.of(rig.repository.refresh(hosts), new HashSet<>(Collections.singletonList("tower")));
        } finally {
            rig.close();
        }
        prefs.edit().putString(HomeView.KEY_LAYOUT, "grid").putString(HomeView.KEY_GROUP, "none").commit();
        HomeShellTest.Home home = home().get();
        RecyclerView list = home.findViewById(R.id.console_list);
        ComputerDetails tower = nativeHost("tower", "Tower", true);
        ConsoleHomeAdapter.Listener none = (ConsoleHomeAdapter.Listener) java.lang.reflect.Proxy.newProxyInstance(
                getClass().getClassLoader(), new Class<?>[] {ConsoleHomeAdapter.Listener.class},
                (proxy, method, args) -> method.getReturnType() == boolean.class ? false : null);
        ConsoleHomeListAdapter adapter = new ConsoleHomeListAdapter(home, null, uuid -> tower, none);
        CoverLoaders covers = new CoverLoaders(home, "unique", 120);
        try {
            adapter.setCovers(covers);
            ReflectionHelpers.setField(home, "adapter", adapter);
            list.setAdapter(adapter);
            adapter.submit(model, HomeView.Group.NONE);
            layout(list, 1080, 2200);
            View portal = null, notes = null;
            for (int i = 0; i < list.getChildCount(); i++) {
                View child = list.getChildAt(i);
                if (list.getChildViewHolder(child).getItemViewType() != ConsoleHomeListAdapter.COVER_GAME) continue;
                String title = ((TextView) child.findViewById(R.id.console_title)).getText().toString();
                if (title.equals("Portal")) portal = child;
                if (title.equals("Notes")) notes = child;
            }
            TextView notesText = notes.findViewById(R.id.console_cover_text);
            assertEquals("its title on the cover's surface", View.VISIBLE, notesText.getVisibility());
            assertEquals("Notes", notesText.getText().toString());
            assertNull("nothing loads", ((android.widget.ImageView) notes.findViewById(R.id.console_cover)).getDrawable());
            assertNotNull("a cover loads as before", ((android.widget.ImageView) portal.findViewById(R.id.console_cover)).getDrawable());
        } finally {
            covers.release();
        }
    }

    @Test
    public void aListLineSaysWhenTheGameWasLastPlayed() {
        long now = 1_800_000_000L;
        assertEquals("0 is the host's never played", "Steam · Not played yet", ConsoleHomeListAdapter.listLine(context, "Steam", 0L, now));
        assertEquals("no host says: nothing about playing", "Steam", ConsoleHomeListAdapter.listLine(context, "Steam", null, now));
        assertEquals("", ConsoleHomeListAdapter.listLine(context, null, null, now));
        assertEquals("Steam · Played today", ConsoleHomeListAdapter.listLine(context, "Steam", now - 3_600, now));
        assertEquals("Steam · Played yesterday", ConsoleHomeListAdapter.listLine(context, "Steam", now - 86_400 - 60, now));
        assertEquals("Steam · Played 9 days ago", ConsoleHomeListAdapter.listLine(context, "Steam", now - 9 * 86_400, now));
        assertEquals("Played today", ConsoleHomeListAdapter.listLine(context, null, now, now));
    }

    @Test
    public void aPhoneCanShowTheShelves() {
        prefs.edit().putString(HomeView.KEY_LAYOUT, "rows").commit();
        HomeShellTest.Home home = home().get();
        RecyclerView list = home.findViewById(R.id.console_list);
        assertTrue(list.getLayoutManager() instanceof ShelfLayoutManager);
        assertNull(ReflectionHelpers.getField(home, "adapter"));
    }

    @Test
    public void theViewSheetSavesTheChoiceAndRebuildsHome() {
        ActivityController<HomeShellTest.Home> controller = home();
        HomeShellTest.Home home = controller.get();
        View button = home.findViewById(R.id.console_view);
        assertEquals(home.getString(R.string.home_view), button.getContentDescription().toString());
        assertTrue(button.performClick());
        ActionSheet sheet = openSheet();
        assertNotNull(sheet);
        View content = sheet.findViewById(android.R.id.content);
        for (int label : new int[] {R.string.home_view_layout_grid, R.string.home_view_layout_rows,
                R.string.home_view_layout_list, R.string.home_view_group_store, R.string.home_view_group_type, R.string.home_view_group_none,
                R.string.home_view_size_small,
                R.string.home_view_size_medium, R.string.home_view_size_large}) {
            assertNotNull(home.getString(label), button(content, home.getString(label)));
        }
        MaterialButton grid = button(content, home.getString(R.string.home_view_layout_grid));
        MaterialButton rows = button(content, home.getString(R.string.home_view_layout_rows));
        MaterialButton lines = button(content, home.getString(R.string.home_view_layout_list));
        // Grid, List, Rows, as on the desktop.
        android.view.ViewGroup layouts = (android.view.ViewGroup) grid.getParent();
        assertEquals(java.util.Arrays.asList(0, 1, 2), java.util.Arrays.asList(layouts.indexOfChild(grid),
                layouts.indexOfChild(lines), layouts.indexOfChild(rows)));
        assertTrue(rows.isChecked());
        assertFalse(grid.isChecked());
        // A list's lines have one size: Size goes while List is chosen, and comes back with the covers.
        View sizes = (View) button(content, home.getString(R.string.home_view_size_large)).getParent();
        assertEquals(View.VISIBLE, sizes.getVisibility());
        assertTrue(lines.performClick());
        assertEquals(View.GONE, sizes.getVisibility());
        assertTrue(rows.performClick());
        assertEquals(View.VISIBLE, sizes.getVisibility());
        // One choice per part.
        assertTrue(rows.isChecked());
        assertFalse(grid.isChecked());
        assertTrue(button(content, home.getString(R.string.home_view_size_large)).performClick());
        assertTrue(button(content, home.getString(R.string.home_view_size_large)).isChecked());
        assertFalse(button(content, home.getString(R.string.home_view_size_medium)).isChecked());
        // Nothing is stored or rebuilt until the sheet closes.
        assertNull(prefs.getString(HomeView.KEY_LAYOUT, null));
        sheet.dismiss();
        shadowOf(Looper.getMainLooper()).idle();
        assertEquals("rows", prefs.getString(HomeView.KEY_LAYOUT, null));
        assertEquals("store", prefs.getString(HomeView.KEY_GROUP, null));
        assertEquals("large", prefs.getString(HomeView.KEY_SIZE, null));
        HomeShellTest.Home rebuilt = controller.get();
        assertEquals(new HomeView(HomeView.Layout.ROWS, HomeView.Group.STORE, HomeView.Size.LARGE), rebuilt.view());
        RecyclerView list = rebuilt.findViewById(R.id.console_list);
        assertTrue(list.getLayoutManager() instanceof ShelfLayoutManager);
    }

    @Test
    @Config(qualifiers = "w960dp-h540dp-land-television-xhdpi")
    public void onATelevisionTheViewSheetHangsUnderItsButtonNotOnTheFoot() {
        HomeShellTest.Home home = home().get();
        home.findViewById(R.id.console_view).performClick();
        ActionSheet sheet = openSheet();
        assertNotNull(sheet);
        assertTrue("a panel, not a sheet across the bottom", sheet.isPanel());
        sheet.dismiss();
        shadowOf(Looper.getMainLooper()).idle();
    }

    /**
     * On the real 1080p TV the View sheet's last part, Card size, was cut off inside its panel: its toggles ran past the
     * panel's foot. With every part shown the whole sheet fits under its button with nothing to scroll.
     */
    @Test
    @Config(qualifiers = "de-w960dp-h540dp-land-television-xhdpi")
    @org.robolectric.annotation.GraphicsMode(org.robolectric.annotation.GraphicsMode.Mode.NATIVE)
    public void onATelevisionTheWholeViewSheetFitsWithNothingToScroll() {
        HomeShellTest.Home home = home().get();
        View root = home.getWindow().getDecorView();
        root.measure(View.MeasureSpec.makeMeasureSpec(1920, View.MeasureSpec.EXACTLY),
                View.MeasureSpec.makeMeasureSpec(1080, View.MeasureSpec.EXACTLY));
        root.layout(0, 0, 1920, 1080);
        home.findViewById(R.id.console_view).performClick();
        ActionSheet sheet = openSheet();
        assertNotNull(sheet);
        View decor = sheet.getWindow().getDecorView();
        decor.measure(View.MeasureSpec.makeMeasureSpec(1920, View.MeasureSpec.EXACTLY),
                View.MeasureSpec.makeMeasureSpec(1080, View.MeasureSpec.EXACTLY));
        decor.layout(0, 0, 1920, 1080);
        View size = button(sheet.findViewById(android.R.id.content), home.getString(R.string.home_view_size_large));
        assertEquals(View.VISIBLE, ((View) size.getParent()).getVisibility());
        androidx.core.widget.NestedScrollView scroll = null;
        for (android.view.ViewParent at = size.getParent(); at != null; at = at.getParent()) {
            if (at instanceof androidx.core.widget.NestedScrollView) scroll = (androidx.core.widget.NestedScrollView) at;
        }
        assertNotNull(scroll);
        assertTrue("rows " + scroll.getChildAt(0).getHeight() + " px in a scroll of " + scroll.getHeight(),
                scroll.getChildAt(0).getHeight() <= scroll.getHeight());
        View body = (View) scroll.getParent();
        int[] at = new int[2];
        body.getLocationInWindow(at);
        assertTrue("the panel stays on screen: " + (at[1] + body.getHeight()), at[1] + body.getHeight() <= 1080);
        sheet.dismiss();
        shadowOf(Looper.getMainLooper()).idle();
    }

    @Test
    public void aPhonesListFadesOutUnderTheTopRowOnlyAtTheTop() {
        HomeShellTest.Home home = home().get();
        shadowOf(Looper.getMainLooper()).idle();
        HomeList list = home.findViewById(R.id.console_list);
        assertTrue("scrolled rows fade under the PC capsule instead of passing whole under it", list.isVerticalFadingEdgeEnabled());
        assertTrue(list.getPaddingTop() > 0);
        assertEquals("the fade covers the top row, down to where the list rests", list.getPaddingTop(), list.getVerticalFadingEdgeLength());
        assertEquals("the foot has its own fade over the tabs", 0f,
                (float) ReflectionHelpers.callInstanceMethod(list, "getBottomFadingEdgeStrength"), 0f);
    }

    @Test
    public void closingTheSheetUnchangedStoresNothing() {
        HomeShellTest.Home home = home().get();
        home.findViewById(R.id.console_view).performClick();
        ActionSheet sheet = openSheet();
        assertNotNull(sheet);
        sheet.dismiss();
        shadowOf(Looper.getMainLooper()).idle();
        assertNull(prefs.getString(HomeView.KEY_LAYOUT, null));
        assertNull(prefs.getString(HomeView.KEY_SIZE, null));
    }
}
