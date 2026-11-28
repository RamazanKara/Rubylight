package com.limelight.console;

import static com.limelight.nativeapi.NativeApiFixtures.nativeHost;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotEquals;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

import android.app.Activity;
import android.app.Application;
import android.content.Context;
import android.widget.ImageView;
import android.widget.LinearLayout;
import android.widget.TextView;

import androidx.recyclerview.widget.GridLayoutManager;
import androidx.recyclerview.widget.RecyclerView;

import com.limelight.PcViewLifecycleTest;
import com.limelight.computers.HostActions;
import com.limelight.computers.HostMenu;
import com.limelight.grid.assets.CachedAppAssetLoader;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvApp;
import com.limelight.ui.ToolbarIconTintTest;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.Shadows;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.util.ReflectionHelpers;

import java.util.Collections;
import java.util.List;

/** Regression tests for console lists, HostCapsule actions, focus safety, cover tags, fact filter, and resume refresh. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class,
        shadows = PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class ConsoleRegressionTest {
    private final Context context = RuntimeEnvironment.getApplication();

    @Test
    public void homeListAndTopFadeRecyclerViewExtendTopFadeIntoTopPadding() {
        HomeList homeList = new HomeList(context);
        homeList.setPadding(0, 96, 0, 48);
        assertTrue(homeList.isPaddingOffsetRequired());
        assertEquals(-96, homeList.getTopPaddingOffset());
        assertEquals(0f, homeList.getBottomFadingEdgeStrength(), 0f);

        TopFadeRecyclerView allGames = new TopFadeRecyclerView(context);
        allGames.setPadding(0, 72, 0, 24);
        assertTrue(allGames.isPaddingOffsetRequired());
        assertEquals(-72, allGames.getTopPaddingOffset());
        assertEquals(0f, allGames.getBottomFadingEdgeStrength(), 0f);
    }

    @Test
    public void hostCapsuleRoutesConnectionAddressActionToHostActions() {
        ActivityController<ToolbarIconTintTest.Screen> controller =
                Robolectric.buildActivity(ToolbarIconTintTest.Screen.class).setup();
        try {
            ToolbarIconTintTest.Screen activity = controller.get();
            HostActions actions = new HostActions(activity, new HostActions.Callbacks() {
                @Override public com.limelight.computers.ComputerManagerService.ComputerManagerBinder binder() { return null; }
                @Override public boolean isForeground() { return true; }
                @Override public void onPaired(ComputerDetails computer) { }
                @Override public void onRemoved(ComputerDetails computer) { }
                @Override public void onRenamed(ComputerDetails computer) { }
                @Override public void onWakeSent(ComputerDetails computer) { }
                @Override public void pausePolling() { }
                @Override public void resumePolling() { }
            });
            HostCapsule capsule = new HostCapsule(activity, null, null, new HostCapsule.Screen() {
                @Override public HomeModel model() { return HomeModel.empty(); }
                @Override public ComputerDetails host(String uuid) { return null; }
                @Override public com.limelight.computers.ComputerManagerService.ComputerManagerBinder binder() { return null; }
                @Override public HostActions actions() { return actions; }
                @Override public void setHostFilter(String uuid) { }
            });
            ComputerDetails pc = nativeHost("addr-pc", "Desk", true);
            android.app.Dialog before = org.robolectric.shadows.ShadowDialog.getLatestDialog();
            ReflectionHelpers.callInstanceMethod(capsule, "onAction",
                    ReflectionHelpers.ClassParameter.from(ComputerDetails.class, pc),
                    ReflectionHelpers.ClassParameter.from(int.class, HostMenu.CONNECTION_ADDRESS));
            android.app.Dialog shown = org.robolectric.shadows.ShadowDialog.getLatestDialog();
            assertTrue("CONNECTION_ADDRESS must open the choose-address dialog",
                    shown != null && shown != before && shown.isShowing());
            shown.dismiss();
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void homeFocusManagerHandlesOutOfBoundsRevealSafelyAfterShrink() {
        ActivityController<ToolbarIconTintTest.Screen> controller =
                Robolectric.buildActivity(ToolbarIconTintTest.Screen.class).setup();
        try {
            Activity activity = controller.get();
            RecyclerView list = new RecyclerView(activity);
            GridLayoutManager grid = new GridLayoutManager(activity, 2);
            ConsoleHomeListAdapter adapter = new ConsoleHomeListAdapter(activity, null, uuid -> null, null, false);
            list.setLayoutManager(grid);
            list.setAdapter(adapter);
            HomeFocusManager focus = new HomeFocusManager(new HomeFocusManager.Host() {
                @Override public Activity activity() { return activity; }
                @Override public RecyclerView list() { return list; }
                @Override public GridLayoutManager grid() { return grid; }
                @Override public void setGrid(GridLayoutManager g) { }
                @Override public ConsoleHomeListAdapter adapter() { return adapter; }
                @Override public void setAdapter(ConsoleHomeListAdapter a) { }
                @Override public ConsoleHomeAdapter shelves() { return null; }
                @Override public void setShelves(ConsoleHomeAdapter s) { }
                @Override public HomeAmbience ambience() { return null; }
                @Override public void setAmbience(HomeAmbience a) { }
                @Override public HomeHints hints() { return null; }
                @Override public void setHints(HomeHints h) { }
                @Override public HomeChrome chrome() { return null; }
                @Override public HomeView view() {
                    return HomeView.read(activity.getSharedPreferences("console-regression", Context.MODE_PRIVATE), false);
                }
                @Override public HostFeed feed() { return null; }
                @Override public CoverLoaders covers() { return null; }
                @Override public boolean isTelevision() { return false; }
                @Override public boolean isFocusPending() { return false; }
                @Override public void setFocusPending(boolean pending) { }
                @Override public boolean isAtStart() { return true; }
                @Override public void onInput(com.limelight.ui.InputSource source, int vendor) { }
            });
            // Empty adapter: stale positions must return safe reveal without IndexOutOfBoundsException.
            ConsoleHomeListAdapter.Reveal reveal = focus.revealFor(5);
            assertEquals(0, reveal.position);
            assertEquals(0, reveal.offset);
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void coverLoadersClearsStaleGameInfoTagWhenRebindingToHostCover() {
        CoverLoaders covers = new CoverLoaders(context, "uid", 140);
        try {
            ImageView image = new ImageView(context);
            TextView text = new TextView(context);
            image.setTag("game-info:stale");
            ComputerDetails host = nativeHost("pc", "PC", true);
            covers.bind(host, new NvApp("Portal", 5, false), image, text);
            assertNull("binding a host cover clears prior game-info tag", image.getTag());
            CachedAppAssetLoader.clearBoundView(image);
        } finally {
            covers.release();
        }
    }

    @Test
    public void allGamesFactFilterDestroyCancelsPendingCallbacks() {
        ActivityController<ToolbarIconTintTest.Screen> controller =
                Robolectric.buildActivity(ToolbarIconTintTest.Screen.class).setup();
        try {
            Activity activity = controller.get();
            AllGamesFactFilter filter = new AllGamesFactFilter(activity, new LinearLayout(activity),
                    HomeModel::empty, () -> HomeModel.FILTER_ALL, reset -> { });
            ReflectionHelpers.callInstanceMethod(filter, "factsArrived");
            Shadows.shadowOf(android.os.Looper.getMainLooper()).idle();
            filter.destroy();
            assertFalse((Boolean) ReflectionHelpers.getField(filter, "factsReloadPending"));
            assertTrue((Boolean) ReflectionHelpers.getField(filter, "destroyed"));
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void consoleHomeSignatureIncludesSessionIdleTransition() {
        ComputerDetails running = nativeHost("pc", "PC", true);
        running.runningGameId = 42;
        running.sessionIdle = false;
        ComputerDetails suspended = nativeHost("pc", "PC", true);
        suspended.runningGameId = 42;
        suspended.sessionIdle = true;
        assertNotEquals("sessionIdle change must invalidate refreshedSignature",
                ConsoleHomeActivity.signature(Collections.singletonList(running)),
                ConsoleHomeActivity.signature(Collections.singletonList(suspended)));
    }

    @Test
    public void noteStreamEndedInvalidatesTargetHostOnlyOnResume() {
        List<String> invalidated = new java.util.ArrayList<>();
        com.limelight.computers.ComputerManagerService service = new com.limelight.computers.ComputerManagerService();
        com.limelight.computers.ComputerManagerService.ComputerManagerBinder recorder =
                service.new ComputerManagerBinder() {
                    @Override
                    public void invalidateStateForComputer(String uuid) {
                        invalidated.add(uuid);
                    }
                };

        HostFeed.noteStreamEnded("host-a");
        assertTrue("first invalidateRecentStreams handles ended host", HostFeed.invalidateRecentStreams(recorder));
        assertEquals(Collections.singletonList("host-a"), invalidated);
        assertFalse("subsequent call does not re-invalidate once drained", HostFeed.invalidateRecentStreams(recorder));
    }
}
