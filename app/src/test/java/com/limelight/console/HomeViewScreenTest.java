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
    public void aPhoneKeepsItsGridOfOneLibrary() throws Exception {
        HomeShellTest.Home home = home().get();
        assertEquals(HomeView.defaults(false), home.view());
        RecyclerView list = home.findViewById(R.id.console_list);
        assertTrue(list.getLayoutManager() instanceof GridLayoutManager);
        assertNull(ReflectionHelpers.getField(home, "shelves"));
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
        assertEquals(home.getString(R.string.home_view), ((TextView) button).getText().toString());
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
        assertTrue(grid.isChecked());
        assertFalse(rows.isChecked());
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
        assertEquals("none", prefs.getString(HomeView.KEY_GROUP, null));
        assertEquals("large", prefs.getString(HomeView.KEY_SIZE, null));
        HomeShellTest.Home rebuilt = controller.get();
        assertEquals(new HomeView(HomeView.Layout.ROWS, HomeView.Group.NONE, HomeView.Size.LARGE), rebuilt.view());
        RecyclerView list = rebuilt.findViewById(R.id.console_list);
        assertTrue(list.getLayoutManager() instanceof ShelfLayoutManager);
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
