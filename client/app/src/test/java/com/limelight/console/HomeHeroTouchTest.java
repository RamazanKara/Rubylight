package com.limelight.console;

import static com.limelight.nativeapi.NativeApiFixtures.game;
import static com.limelight.nativeapi.NativeApiFixtures.library;
import static com.limelight.nativeapi.NativeApiFixtures.nativeHost;
import static com.limelight.nativeapi.NativeApiFixtures.ok;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertTrue;
import static org.robolectric.Shadows.shadowOf;

import android.app.Application;
import android.content.Intent;
import android.content.ServiceConnection;
import android.os.Looper;
import android.preference.PreferenceManager;
import android.view.InputDevice;
import android.view.MotionEvent;
import android.view.View;

import androidx.recyclerview.widget.RecyclerView;

import com.limelight.PcViewLifecycleTest;
import com.limelight.R;
import com.limelight.computers.ComputerManagerListener;
import com.limelight.computers.ComputerManagerService;
import com.limelight.nativeapi.ConsoleSnapshot;
import com.limelight.nativeapi.NativeApiFixtures;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.http.NvApp;

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

import java.util.ArrayList;
import java.util.Collections;
import java.util.HashSet;
import java.util.List;
import java.util.Set;

/**
 * A finger on the hero's Play starts the game on the first tap, also right after a pad left the ring on it (the Retroid: the
 * tap only dropped the ring). Entering touch mode handed the keys' focus to Home's list, which gave it to the shelf strip
 * below the hero; that shelf's snap started a scroll, and the list took the finger's down to stop it.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class, shadows = PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni", qualifiers = "w891dp-h411dp-land-xxhdpi")
public class HomeHeroTouchTest {
    /** Home without the computer manager service; a start is written down instead of streamed. */
    public static class Home extends ConsoleHomeActivity {
        final List<String> played = new ArrayList<>();

        @Override protected void play(NvApp app, ComputerDetails computer, String source) {
            played.add(app.getAppName());
        }

        @Override public boolean bindService(Intent intent, ServiceConnection connection, int flags) {
            return false;
        }
    }

    @Rule public TemporaryFolder temporary = new TemporaryFolder();

    private ConsoleRig rig;

    @Before
    public void setUp() throws Exception {
        PreferenceManager.getDefaultSharedPreferences(RuntimeEnvironment.getApplication()).edit().clear().commit();
        rig = new ConsoleRig(temporary.newFolder());
    }

    @After
    public void tearDown() {
        rig.close();
        PreferenceManager.getDefaultSharedPreferences(RuntimeEnvironment.getApplication()).edit().clear().commit();
    }

    /** One PC with a library of {@code games}: a handheld's hero over the library's shelves, nothing running, no panel. */
    private RecyclerView show(Home home, int games) throws Exception {
        ComputerDetails tower = nativeHost("tower", "Tower", true);
        // As HomeHeroPlayTest's PC: Play starts at once with nothing running.
        tower.caps = HostCapabilities.UNKNOWN;
        HostFeed feed = ReflectionHelpers.getField(home, "feed");
        ReflectionHelpers.setField(feed, "binder", new ComputerManagerService().new ComputerManagerBinder() {
            @Override public boolean isPersistent() { return false; }
            @Override public void startPolling(ComputerManagerListener listener) { }
            @Override public void stopPolling() { }
            @Override public void invalidateStateForComputer(String uuid) { }
            @Override public String getUniqueId() { return null; }
            @Override public void waitForPollingStopped() { }
        });
        feed.update(tower);
        String[] listed = new String[games];
        for (int i = 0; i < games; i++) listed[i] = game("tower-g" + i, i + 1, "Game " + i, null, false);
        rig.transport.reply("tower", "library", ok(library("r", listed)));
        rig.transport.reply("tower", "session", ok(NativeApiFixtures.IDLE));
        ConsoleSnapshot snapshot = rig.repository.refresh(Collections.singletonList(nativeHost("tower", "Tower", true)));
        ReflectionHelpers.setField(home, "shownSnapshot", snapshot);
        Set<String> online = new HashSet<>(Collections.singletonList("tower"));
        ConsoleHomeAdapter shelves = ReflectionHelpers.getField(home, "shelves");
        shelves.submit(HomeModel.of(snapshot, online));
        home.findViewById(R.id.console_loading).setVisibility(View.GONE);
        layOut(home);
        layOut(home);
        return home.findViewById(R.id.console_list);
    }

    @Test
    public void aTapOnPlayRightAfterThePadStartsTheGameOnce() throws Exception {
        ActivityController<Home> controller = Robolectric.buildActivity(Home.class).setup().visible();
        try {
            Home home = controller.get();
            RecyclerView list = show(home, 12);
            ConsoleHomeAdapter shelves = ReflectionHelpers.getField(home, "shelves");
            assertNotNull("a hero", shelves.heroItem());
            View play = shelves.heroPlay(list);
            assertNotNull("the hero's Play is laid out", play);
            assertTrue("a shelf lies below the hero", list.getChildCount() > 1);

            // The pad was used: the window is out of touch mode and Play wears the ring; the list rests.
            Object root = ReflectionHelpers.callInstanceMethod(home.getWindow().getDecorView(), "getViewRootImpl");
            ReflectionHelpers.callInstanceMethod(root, "ensureTouchMode", ReflectionHelpers.ClassParameter.from(boolean.class, false));
            assertTrue(play.requestFocus());
            shadowOf(Looper.getMainLooper()).idleFor(java.time.Duration.ofSeconds(1));
            layOut(home);
            list.stopScroll();
            assertTrue("Play has the ring", play.isFocused());
            int offset = list.computeVerticalScrollOffset();

            // A finger lands: the window enters touch mode before the down reaches any view, as ViewRootImpl does it.
            ReflectionHelpers.callInstanceMethod(root, "ensureTouchMode", ReflectionHelpers.ClassParameter.from(boolean.class, true));
            assertFalse("the ring leaves Play", play.isFocused());
            assertFalse("the keys' focus is not handed to the list", list.hasFocus());
            assertEquals("nothing starts to scroll under the finger", RecyclerView.SCROLL_STATE_IDLE, list.getScrollState());
            assertEquals("the list stays where it was", offset, list.computeVerticalScrollOffset());

            int[] at = new int[2];
            play.getLocationInWindow(at);
            tap(home, at[0] + play.getWidth() / 2f, at[1] + play.getHeight() / 2f);
            shadowOf(Looper.getMainLooper()).idle();

            assertEquals("the first tap starts the hero's game, once",
                    Collections.singletonList(shelves.heroItem().game.title), home.played);
        } finally {
            controller.pause().stop().destroy();
        }
    }

    /** Keys keep everything they had: the list and its strips still take focus out of touch mode, only a finger skips them. */
    @Test
    public void theListAndItsStripsStayFocusableForKeysOnly() throws Exception {
        ActivityController<Home> controller = Robolectric.buildActivity(Home.class).setup().visible();
        try {
            Home home = controller.get();
            RecyclerView list = show(home, 12);
            assertTrue(list.isFocusable());
            assertFalse(list.isFocusableInTouchMode());
            int strips = 0;
            for (int i = 0; i < list.getChildCount(); i++) {
                View cards = list.getChildAt(i).findViewById(R.id.shelf_cards);
                if (cards == null) continue;
                strips++;
                assertTrue(cards.isFocusable());
                assertFalse(cards.isFocusableInTouchMode());
            }
            assertTrue("strips were laid out", strips > 0);
        } finally {
            controller.pause().stop().destroy();
        }
    }

    private static void tap(Home home, float x, float y) {
        long now = android.os.SystemClock.uptimeMillis();
        for (int action : new int[] {MotionEvent.ACTION_DOWN, MotionEvent.ACTION_UP}) {
            MotionEvent event = MotionEvent.obtain(now, now, action, x, y, 0);
            event.setSource(InputDevice.SOURCE_TOUCHSCREEN);
            home.dispatchTouchEvent(event);
            event.recycle();
        }
    }

    private static void layOut(Home home) {
        View root = home.getWindow().getDecorView();
        int w = home.getResources().getDisplayMetrics().widthPixels;
        int h = home.getResources().getDisplayMetrics().heightPixels;
        root.measure(View.MeasureSpec.makeMeasureSpec(w, View.MeasureSpec.EXACTLY), View.MeasureSpec.makeMeasureSpec(h, View.MeasureSpec.EXACTLY));
        root.layout(0, 0, w, h);
    }
}
