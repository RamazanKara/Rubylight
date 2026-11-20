package com.limelight.console;

import static com.limelight.nativeapi.NativeApiFixtures.library;
import static com.limelight.nativeapi.NativeApiFixtures.nativeHost;
import static com.limelight.nativeapi.NativeApiFixtures.ok;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertTrue;

import android.app.Application;
import android.content.Context;
import android.content.SharedPreferences;
import android.os.Build;
import android.preference.PreferenceManager;
import android.view.View;

import androidx.recyclerview.widget.RecyclerView;

import com.limelight.R;
import com.limelight.nativeapi.NativeApiFixtures;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.preferences.GlPreferences;
import com.limelight.ui.Springs;

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
import org.robolectric.shadows.ShadowLooper;
import org.robolectric.util.ReflectionHelpers;

import java.io.File;
import java.util.ArrayList;
import java.util.Collections;
import java.util.HashSet;
import java.util.List;
import java.util.concurrent.TimeUnit;

/**
 * nightjar#7 on an Android TV, Home's grid and list views: where a focused card comes to rest. The list's tall foot
 * padding left the platform's scroll a padded view only a sliver high, so every focused card rested with its top on the
 * list's clipped and faded edge, its ring cut and its section's title gone.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class, qualifiers = "w960dp-h540dp-land-television-xhdpi")
public class HomeTvGridFocusTest {
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

    /** Three apps on Steam and thirty Epic games: the Epic part comes first, as the larger one. */
    private static HomeModel model(ConsoleRig rig) throws Exception {
        List<String> games = new ArrayList<>();
        games.add(HomeLibraryRowsTest.game(1, "Desktop", "steam", true, null));
        games.add(HomeLibraryRowsTest.game(2, "Playnite", "steam", true, null));
        games.add(HomeLibraryRowsTest.game(3, "Steam Big Picture", "steam", true, null));
        for (int i = 0; i < 30; i++) games.add(HomeLibraryRowsTest.game(10 + i, "Game " + i, "epic", true, null));
        rig.transport.reply("tower", "capabilities", ok("{\"api_version\":1,\"features\":[\"library\"],"
                + "\"permissions\":{\"list\":true,\"view\":true,\"launch\":true}}"));
        rig.transport.reply("tower", "library", ok(library("t", games.toArray(new String[0]))));
        rig.transport.reply("tower", "session", ok(NativeApiFixtures.IDLE));
        List<ComputerDetails> hosts = Collections.singletonList(nativeHost("tower", "Tower", true));
        return HomeModel.of(rig.repository.refresh(hosts), new HashSet<>(Collections.singletonList("tower")));
    }

    /** Home on a 1080p TV showing the model, after its own first loads have settled. */
    private static HomeShellTest.Home open(ActivityController<HomeShellTest.Home> controller, HomeModel model) {
        HomeShellTest.Home home = controller.get();
        // Past the settle and the wait for the PCs, so Home's own reload does not replace the model below.
        ShadowLooper.idleMainLooper(12, TimeUnit.SECONDS);
        ConsoleHomeListAdapter adapter = ReflectionHelpers.getField(home, "adapter");
        adapter.submit(model, home.view().group);
        layOut(home);
        ShadowLooper.idleMainLooper(600, TimeUnit.MILLISECONDS);
        layOut(home);
        return home;
    }

    private static void layOut(HomeShellTest.Home home) {
        View root = home.getWindow().getDecorView();
        root.measure(View.MeasureSpec.makeMeasureSpec(1920, View.MeasureSpec.EXACTLY),
                View.MeasureSpec.makeMeasureSpec(1080, View.MeasureSpec.EXACTLY));
        root.layout(0, 0, 1920, 1080);
    }

    /** As a key does: the window's focus search and the move, then the scroll it starts runs to its end. */
    private static View press(RecyclerView list, int direction) {
        View next = list.findFocus().focusSearch(direction);
        if (next != null) next.requestFocus(direction);
        ShadowLooper.idleMainLooper(600, TimeUnit.MILLISECONDS);
        return next;
    }

    private static View laidOut(RecyclerView list, int position) {
        RecyclerView.ViewHolder holder = list.findViewHolderForAdapterPosition(position);
        return holder == null ? null : holder.itemView;
    }

    /** The top of the focus ring a lifted poster draws, in the list's coordinates. */
    private static float ringTop(View card) {
        Springs.finish(card);
        View frame = card.findViewById(R.id.console_cover_frame);
        android.content.res.Resources res = card.getResources();
        int ring = res.getDimensionPixelSize(R.dimen.glass_ring_gap) + res.getDimensionPixelSize(R.dimen.glass_ring_width);
        float pivot = card.getPivotY();
        return card.getTop() + pivot + (frame.getTop() - ring - pivot) * card.getScaleY();
    }

    @Test
    public void aFocusedPosterRestsWithItsRingInsideTheListAndItsTitleAboveTheFirstRow() throws Exception {
        prefs.edit().putString(HomeView.KEY_LAYOUT, "grid").putString(HomeView.KEY_GROUP, "store").commit();
        ActivityController<HomeShellTest.Home> controller = Robolectric.buildActivity(HomeShellTest.Home.class).setup().visible();
        ConsoleRig rig = new ConsoleRig(temporary.newFolder());
        try {
            HomeShellTest.Home home = open(controller, model(rig));
            ConsoleHomeListAdapter adapter = ReflectionHelpers.getField(home, "adapter");
            RecyclerView list = home.findViewById(R.id.console_list);
            int fade = list.getVerticalFadingEdgeLength();
            assertTrue("the list fades at its top", list.isVerticalFadingEdgeEnabled() && fade > 0);

            View first = laidOut(list, adapter.firstCard());
            assertTrue(first.requestFocus());
            ShadowLooper.idleMainLooper(600, TimeUnit.MILLISECONDS);
            View title = laidOut(list, 0);
            assertNotNull("the first card keeps its section's title on screen", title);
            assertEquals("the title starts the list", list.getPaddingTop(), title.getTop());
            assertTrue("the lifted card's ring stays inside the list", ringTop(first) >= 0);

            for (int row = 2; row <= 4; row++) {
                View card = press(list, View.FOCUS_DOWN);
                assertNotNull(card);
                assertTrue("row " + row + ": the ring stands below the list's clipped, faded edge (ring at " + ringTop(card)
                        + ", fade " + fade + ")", ringTop(card) >= fade);
                assertTrue("and the card is on screen", card.getBottom() <= list.getHeight());
            }
            for (int row = 0; row < 3; row++) press(list, View.FOCUS_UP);
            assertEquals(adapter.firstCard(), list.getChildAdapterPosition(list.getFocusedChild()));
            title = laidOut(list, 0);
            assertNotNull("back on the first row, its title comes back although it had left the list", title);
            assertEquals(list.getPaddingTop(), title.getTop());
        } finally {
            rig.close();
            controller.pause().stop().destroy();
        }
    }

    /**
     * The reporter's Home: Desktop and Playnite to continue, the library as List lines. Down from Desktop and back Up went
     * to Playnite, the poster nearest the middle of a line as wide as the screen.
     */
    @Test
    public void upFromTheLineDownLandedOnGoesBackToTheCardItCameFrom() throws Exception {
        prefs.edit().putString(HomeView.KEY_LAYOUT, "list").putString(HomeView.KEY_GROUP, "none").commit();
        ActivityController<HomeShellTest.Home> controller = Robolectric.buildActivity(HomeShellTest.Home.class).setup().visible();
        ConsoleRig rig = new ConsoleRig(temporary.newFolder());
        try {
            rig.recents.add(new com.limelight.nativeapi.RecentsStore.Recent("tower", "Tower", 1, "Desktop", 20));
            rig.recents.add(new com.limelight.nativeapi.RecentsStore.Recent("tower", "Tower", 2, "Playnite", 10));
            HomeShellTest.Home home = open(controller, model(rig));
            ConsoleHomeListAdapter adapter = ReflectionHelpers.getField(home, "adapter");
            RecyclerView list = home.findViewById(R.id.console_list);
            int desktop = adapter.firstCard();
            assertEquals("continue:tower:1", adapter.keyAt(desktop));
            assertEquals("continue:tower:2", adapter.keyAt(desktop + 1));
            assertEquals(ConsoleHomeListAdapter.LIST_GAME, adapter.viewTypeAt(desktop + 3));

            assertTrue(laidOut(list, desktop).requestFocus());
            ShadowLooper.idleMainLooper(600, TimeUnit.MILLISECONDS);
            View line = press(list, View.FOCUS_DOWN);
            assertEquals("Down goes to the first line", desktop + 3, list.getChildAdapterPosition(line));
            View back = press(list, View.FOCUS_UP);
            assertEquals("Up goes back to Desktop, not to Playnite", desktop, list.getChildAdapterPosition(back));

            // From Playnite the same way back leads to Playnite.
            assertEquals(desktop + 1, list.getChildAdapterPosition(press(list, View.FOCUS_RIGHT)));
            press(list, View.FOCUS_DOWN);
            assertEquals(desktop + 1, list.getChildAdapterPosition(press(list, View.FOCUS_UP)));

            // A line reached otherwise than by Down from a card keeps the platform's search.
            press(list, View.FOCUS_DOWN);
            press(list, View.FOCUS_DOWN);
            View up = press(list, View.FOCUS_UP);
            assertEquals("Up from the second line is the first line", desktop + 3, list.getChildAdapterPosition(up));
            press(list, View.FOCUS_UP);
            assertTrue("and from the first line, a card of the row above",
                    list.getChildAdapterPosition(list.getFocusedChild()) == desktop
                            || list.getChildAdapterPosition(list.getFocusedChild()) == desktop + 1);
        } finally {
            rig.close();
            controller.pause().stop().destroy();
        }
    }
}
