package com.limelight;

import android.app.Application;
import android.content.Intent;
import android.content.ServiceConnection;
import android.os.Looper;
import android.view.View;
import android.widget.TextView;
import com.limelight.grid.AppGridAdapter;
import com.limelight.computers.ComputerManagerListener;
import com.limelight.computers.ComputerManagerService;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvApp;
import com.limelight.preferences.PreferenceConfiguration;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.Shadows;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.util.ReflectionHelpers;
import java.util.Arrays;
import java.util.Collections;
import java.util.List;
import static org.junit.Assert.*;

@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class,
        shadows = PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class AppViewLifecycleTest {
    public static class Library extends AppView {
        @Override public boolean bindService(Intent intent, ServiceConnection connection, int flags) {
            return false;
        }
    }

    /** The TV's safe-area padding is on the content view; it must not clip the backdrop that reaches past it. */
    @Test
    @Config(qualifiers = "television")
    public void theTvBackdropIsNotClippedToTheSafeAreaPadding() {
        Intent intent = new Intent(RuntimeEnvironment.getApplication(), Library.class)
                .putExtra(AppView.UUID_EXTRA, "test-host").putExtra(AppView.NAME_EXTRA, "PC");
        ActivityController<Library> controller = Robolectric.buildActivity(Library.class, intent).setup();
        try {
            Library screen = controller.get();
            assertNotNull(screen.findViewById(R.id.library_backdrop));
            assertFalse(((android.view.ViewGroup) screen.findViewById(android.R.id.content)).getClipChildren());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    /** A restored grid fragment can come back before the service connects and the adapter exists (Fire TV crash). */
    @Test
    @Config(qualifiers = "television")
    public void aGridFragmentRestoredBeforeTheAdapterExistsDoesNotCrash() {
        Intent intent = new Intent(RuntimeEnvironment.getApplication(), Library.class)
                .putExtra(AppView.UUID_EXTRA, "test-host").putExtra(AppView.NAME_EXTRA, "PC");
        ActivityController<Library> controller = Robolectric.buildActivity(Library.class, intent).setup();
        try {
            Library screen = controller.get();
            screen.getFragmentManager().beginTransaction()
                    .replace(R.id.appFragmentContainer, new com.limelight.ui.AdapterFragment())
                    .commitNow();
            assertFalse(screen.isFinishing());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void failedOrDeniedAppListsOfferRetryWithoutBlockingNavigation() {
        Intent intent = new Intent(RuntimeEnvironment.getApplication(), Library.class)
                .putExtra(AppView.UUID_EXTRA, "test-host").putExtra(AppView.NAME_EXTRA, "PC");
        ActivityController<Library> controller = Robolectric.buildActivity(Library.class, intent).setup();
        try {
            Library screen = controller.get();
            for (int message : new int[] {R.string.applist_load_failed, R.string.applist_permission_required}) {
                ReflectionHelpers.callInstanceMethod(screen, "loadAppsBlocking");
                assertEquals(View.VISIBLE, screen.findViewById(R.id.library_loading).getVisibility());
                assertTrue(screen.findViewById(R.id.library_back).isEnabled());
                ReflectionHelpers.callInstanceMethod(screen, "showAppListStatus",
                        ReflectionHelpers.ClassParameter.from(int.class, message));
                assertEquals(View.GONE, screen.findViewById(R.id.library_loading).getVisibility());
                assertEquals(View.VISIBLE, screen.findViewById(R.id.library_retry).getVisibility());
                TextView status = screen.findViewById(R.id.app_list_status);
                assertEquals(View.VISIBLE, status.getVisibility());
                assertEquals(screen.getString(message), status.getText().toString());
                assertFalse(screen.isFinishing());
            }
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void theCachedListIsReadOffTheUiThreadAndPaintedWhenItArrives() throws Exception {
        String uuid = "cache-host";
        Intent intent = new Intent(RuntimeEnvironment.getApplication(), Library.class)
                .putExtra(AppView.UUID_EXTRA, uuid).putExtra(AppView.NAME_EXTRA, "PC");
        ActivityController<Library> controller = Robolectric.buildActivity(Library.class, intent).setup();
        try {
            Library screen = controller.get();
            try (java.io.OutputStream out =
                         com.limelight.utils.CacheHelper.openCacheFileForOutput(screen.getCacheDir(), "applist", uuid)) {
                out.write(LIBRARY.getBytes(java.nio.charset.StandardCharsets.UTF_8));
            }
            ComputerDetails computer = new ComputerDetails();
            computer.uuid = uuid;
            computer.name = "PC";
            ReflectionHelpers.setField(screen, "computer", computer);
            AppGridAdapter adapter = new AppGridAdapter(screen, new PreferenceConfiguration(), computer, "test", false);
            ReflectionHelpers.setField(screen, "appGridAdapter", adapter);
            ReflectionHelpers.setField(screen, "managerServiceBound", true);

            ReflectionHelpers.callInstanceMethod(screen, "populateAppGridWithCache");
            // The list is read on a worker: nothing reached the grid yet, and the grid already says
            // it is loading rather than waiting for the reader to find a missing cache.
            assertEquals(0, adapter.getCount());
            assertEquals(View.VISIBLE, screen.findViewById(R.id.library_loading).getVisibility());

            long deadline = System.currentTimeMillis() + 5000;
            while (adapter.getCount() == 0 && System.currentTimeMillis() < deadline) {
                Shadows.shadowOf(Looper.getMainLooper()).idle();
                Thread.sleep(10);
            }
            assertEquals(3, adapter.getCount());
            assertEquals(LIBRARY, ReflectionHelpers.getField(screen, "lastRawApplist"));
            assertEquals(View.GONE, screen.findViewById(R.id.library_loading).getVisibility());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void validEmptyAppListExplainsItsStateAndStopsInlineLoading() {
        Intent intent = new Intent(RuntimeEnvironment.getApplication(), Library.class)
                .putExtra(AppView.UUID_EXTRA, "empty-host").putExtra(AppView.NAME_EXTRA, "PC");
        ActivityController<Library> controller = Robolectric.buildActivity(Library.class, intent).setup();
        try {
            Library screen = controller.get();
            ComputerDetails computer = new ComputerDetails();
            computer.uuid = "empty-host";
            computer.name = "PC";
            ReflectionHelpers.setField(screen, "computer", computer);
            ReflectionHelpers.setField(screen, "appGridAdapter",
                    new AppGridAdapter(screen, new PreferenceConfiguration(), computer, "test", false));
            ReflectionHelpers.callInstanceMethod(screen, "loadAppsBlocking");
            ReflectionHelpers.callInstanceMethod(screen, "updateUiWithAppList",
                    ReflectionHelpers.ClassParameter.from(List.class, Collections.emptyList()));
            assertEquals(View.GONE, screen.findViewById(R.id.library_loading).getVisibility());
            // One line and a Refresh, and no tile: a PC that lists nothing has no entry a tile could launch.
            TextView refresh = screen.findViewById(R.id.library_retry);
            assertEquals(View.VISIBLE, refresh.getVisibility());
            assertEquals(screen.getString(R.string.library_refresh), refresh.getText().toString());
            AppGridAdapter adapter = ReflectionHelpers.getField(screen, "appGridAdapter");
            assertEquals(0, adapter.getCount());
            TextView status = screen.findViewById(R.id.app_list_status);
            assertEquals(screen.getString(R.string.library_no_apps), status.getText().toString());
            assertEquals(View.VISIBLE, status.getVisibility());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void unchangedSuccessfulPollClearsFailureAndRestoredPermissionHints() {
        Intent intent = new Intent(RuntimeEnvironment.getApplication(), Library.class)
                .putExtra(AppView.UUID_EXTRA, "same-host").putExtra(AppView.NAME_EXTRA, "PC");
        ActivityController<Library> controller = Robolectric.buildActivity(Library.class, intent).setup();
        try {
            Library screen = controller.get();
            ComputerDetails computer = new ComputerDetails();
            computer.uuid = "same-host";
            computer.rawAppList = "cached-list";
            ReflectionHelpers.setField(screen, "computer", computer);
            ReflectionHelpers.setField(screen, "lastRawApplist", computer.rawAppList);
            ComputerManagerListener[] listener = new ComputerManagerListener[1];
            ComputerManagerService manager = new ComputerManagerService();
            ReflectionHelpers.setField(screen, "managerBinder", manager.new ComputerManagerBinder() {
                @Override public void startPolling(ComputerManagerListener callback) { listener[0] = callback; }
                @Override public void stopPolling() { }
                @Override public ComputerManagerService.ApplistPoller createAppListPoller(ComputerDetails details) {
                    return manager.new ApplistPoller(details) {
                        @Override public void start() { }
                        @Override public void stop() { }
                    };
                }
            });
            ReflectionHelpers.callInstanceMethod(screen, "startComputerUpdates");
            listener[0].notifyAppListUpdated(computer, false);
            TextView status = screen.findViewById(R.id.app_list_status);
            assertEquals(View.VISIBLE, status.getVisibility());
            listener[0].notifyAppListUpdated(computer, true);
            assertEquals(View.GONE, status.getVisibility());
            computer.permission = 0;
            listener[0].notifyAppListUpdated(computer, true);
            assertEquals(screen.getString(R.string.applist_permission_required), status.getText().toString());
            computer.permission = ComputerDetails.PERMISSION_LIST;
            listener[0].notifyAppListUpdated(computer, true);
            assertEquals(View.GONE, status.getVisibility());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test public void offlineHostKeepsCachedLibraryOpenAndOffersRetry() {
        Intent intent = new Intent(RuntimeEnvironment.getApplication(), Library.class)
                .putExtra(AppView.UUID_EXTRA, "offline-host").putExtra(AppView.NAME_EXTRA, "PC");
        ActivityController<Library> controller = Robolectric.buildActivity(Library.class, intent).setup();
        try {
            Library screen = controller.get();
            ComputerDetails computer = new ComputerDetails();
            computer.uuid = "offline-host";
            computer.state = ComputerDetails.State.OFFLINE;
            ReflectionHelpers.setField(screen, "computer", computer);
            ComputerManagerListener[] listener = new ComputerManagerListener[1];
            ComputerManagerService manager = new ComputerManagerService();
            ReflectionHelpers.setField(screen, "managerBinder", manager.new ComputerManagerBinder() {
                @Override public void startPolling(ComputerManagerListener callback) { listener[0] = callback; }
                @Override public void stopPolling() { }
                @Override public ComputerManagerService.ApplistPoller createAppListPoller(ComputerDetails details) {
                    return manager.new ApplistPoller(details) {
                        @Override public void start() { }
                        @Override public void stop() { }
                    };
                }
            });
            ReflectionHelpers.callInstanceMethod(screen, "startComputerUpdates");
            listener[0].notifyComputerUpdated(computer);
            assertFalse(screen.isFinishing());
            assertEquals(View.GONE, screen.findViewById(R.id.library_loading).getVisibility());
            assertEquals(View.VISIBLE, screen.findViewById(R.id.library_retry).getVisibility());
            assertEquals(screen.getString(R.string.library_offline),
                    ((TextView) screen.findViewById(R.id.app_list_status)).getText().toString());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    /** Offline, with the games a saved list still holds. */
    private static void savedLibraryOffline(Library screen, int apps) {
        ComputerDetails computer = new ComputerDetails();
        computer.uuid = "offline-host";
        computer.state = ComputerDetails.State.OFFLINE;
        ReflectionHelpers.setField(screen, "computer", computer);
        ReflectionHelpers.setField(screen, "appGridAdapter",
                new AppGridAdapter(screen, new PreferenceConfiguration(), computer, "test", false));
        NvApp[] list = new NvApp[apps];
        for (int i = 0; i < apps; i++) list[i] = new NvApp("Game " + i, i + 1, false);
        ReflectionHelpers.callInstanceMethod(screen, "updateUiWithAppList",
                ReflectionHelpers.ClassParameter.from(List.class, Arrays.asList(list)));
        ReflectionHelpers.callInstanceMethod(screen, "showAppListStatus",
                ReflectionHelpers.ClassParameter.from(int.class, R.string.library_offline));
        Shadows.shadowOf(Looper.getMainLooper()).idle();
    }

    /** nightjar#7: the offline line and Retry painted over the saved tiles; a TV gets the line above the grid instead. */
    @Test
    @Config(qualifiers = "w960dp-h540dp-land-television-xhdpi")
    public void aTvOfflineWithSavedGamesShowsTheStatusAboveTheGrid() {
        Intent intent = new Intent(RuntimeEnvironment.getApplication(), Library.class)
                .putExtra(AppView.UUID_EXTRA, "offline-host").putExtra(AppView.NAME_EXTRA, "PC");
        ActivityController<Library> controller = Robolectric.buildActivity(Library.class, intent).setup();
        try {
            Library screen = controller.get();
            savedLibraryOffline(screen, 4);
            assertEquals(View.VISIBLE, screen.findViewById(R.id.library_status_banner).getVisibility());
            assertEquals(View.GONE, screen.findViewById(R.id.library_status_panel).getVisibility());
            assertEquals(View.VISIBLE, screen.findViewById(R.id.library_retry_banner).getVisibility());
            assertEquals(screen.getString(R.string.library_offline),
                    ((TextView) screen.findViewById(R.id.app_list_status_banner)).getText().toString());
            // It stands in the chrome, which the grid is placed under, not over the games.
            assertSame(screen.findViewById(R.id.library_chrome),
                    screen.findViewById(R.id.library_status_banner).getParent());
            // Laid out, the line ends above where the grid's container starts.
            android.view.ViewGroup container = screen.findViewById(R.id.appFragmentContainer);
            View grid = new View(screen);
            grid.setId(R.id.fragmentView);
            container.addView(grid, new android.widget.FrameLayout.LayoutParams(
                    android.view.ViewGroup.LayoutParams.MATCH_PARENT, android.view.ViewGroup.LayoutParams.MATCH_PARENT));
            View root = screen.getWindow().getDecorView();
            for (int pass = 0; pass < 2; pass++) {
                root.measure(View.MeasureSpec.makeMeasureSpec(1920, View.MeasureSpec.EXACTLY),
                        View.MeasureSpec.makeMeasureSpec(1080, View.MeasureSpec.EXACTLY));
                root.layout(0, 0, 1920, 1080);
                Shadows.shadowOf(Looper.getMainLooper()).idle();
            }
            View banner = screen.findViewById(R.id.library_status_banner);
            assertTrue("the line ends above the grid: " + banner.getBottom() + " against " + container.getTop(),
                    banner.getBottom() <= container.getTop());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    /** A phone keeps the centred panel it had, over its grid: only a TV takes the line. */
    @Test
    public void aPhoneStillShowsTheOfflinePanelOverItsGrid() {
        Intent intent = new Intent(RuntimeEnvironment.getApplication(), Library.class)
                .putExtra(AppView.UUID_EXTRA, "offline-host").putExtra(AppView.NAME_EXTRA, "PC");
        ActivityController<Library> controller = Robolectric.buildActivity(Library.class, intent).setup();
        try {
            Library screen = controller.get();
            savedLibraryOffline(screen, 4);
            assertEquals(View.GONE, screen.findViewById(R.id.library_status_banner).getVisibility());
            assertEquals(View.VISIBLE, screen.findViewById(R.id.library_status_panel).getVisibility());
            assertEquals(View.VISIBLE, screen.findViewById(R.id.library_retry).getVisibility());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    /** An empty TV grid keeps the centred panel: nothing is behind it to cover. */
    @Test
    @Config(qualifiers = "w960dp-h540dp-land-television-xhdpi")
    public void anEmptyTvGridKeepsTheCentredStatusPanel() {
        Intent intent = new Intent(RuntimeEnvironment.getApplication(), Library.class)
                .putExtra(AppView.UUID_EXTRA, "offline-host").putExtra(AppView.NAME_EXTRA, "PC");
        ActivityController<Library> controller = Robolectric.buildActivity(Library.class, intent).setup();
        try {
            Library screen = controller.get();
            savedLibraryOffline(screen, 0);
            assertEquals(View.GONE, screen.findViewById(R.id.library_status_banner).getVisibility());
            assertEquals(View.VISIBLE, screen.findViewById(R.id.library_status_panel).getVisibility());
            assertEquals(View.VISIBLE, screen.findViewById(R.id.library_retry).getVisibility());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test public void aHostAnsweringAgainClearsTheOfflineNoticeWithoutWaitingForTheAppList() {
        Intent intent = new Intent(RuntimeEnvironment.getApplication(), Library.class)
                .putExtra(AppView.UUID_EXTRA, "returning-host").putExtra(AppView.NAME_EXTRA, "PC");
        ActivityController<Library> controller = Robolectric.buildActivity(Library.class, intent).setup();
        try {
            Library screen = controller.get();
            ComputerDetails computer = new ComputerDetails();
            computer.uuid = "returning-host";
            computer.state = ComputerDetails.State.OFFLINE;
            computer.rawAppList = "cached-list";
            ReflectionHelpers.setField(screen, "computer", computer);
            ReflectionHelpers.setField(screen, "lastRawApplist", computer.rawAppList);
            ComputerManagerListener[] listener = new ComputerManagerListener[1];
            ComputerManagerService manager = new ComputerManagerService();
            ReflectionHelpers.setField(screen, "managerBinder", manager.new ComputerManagerBinder() {
                @Override public void startPolling(ComputerManagerListener callback) { listener[0] = callback; }
                @Override public void stopPolling() { }
                @Override public ComputerManagerService.ApplistPoller createAppListPoller(ComputerDetails details) {
                    return manager.new ApplistPoller(details) {
                        @Override public void start() { }
                        @Override public void stop() { }
                    };
                }
            });
            ReflectionHelpers.callInstanceMethod(screen, "startComputerUpdates");
            listener[0].notifyComputerUpdated(computer);
            TextView status = screen.findViewById(R.id.app_list_status);
            assertEquals(screen.getString(R.string.library_offline), status.getText().toString());

            // The serverinfo poll succeeds; the cached app list is unchanged, so no app list update follows.
            computer.state = ComputerDetails.State.ONLINE;
            computer.pairState = com.limelight.nvstream.http.PairingManager.PairState.PAIRED;
            listener[0].notifyComputerUpdated(computer);
            assertEquals(View.GONE, status.getVisibility());
            assertEquals(View.GONE, screen.findViewById(R.id.library_status_panel).getVisibility());
            assertFalse(screen.isFinishing());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test public void systemBackDispatcherClosesSearchBeforeLeavingTheLibrary() {
        Intent intent = new Intent(RuntimeEnvironment.getApplication(), Library.class)
                .putExtra(AppView.UUID_EXTRA, "search-host").putExtra(AppView.NAME_EXTRA, "PC");
        ActivityController<Library> controller = Robolectric.buildActivity(Library.class, intent).setup();
        try {
            Library screen = controller.get();
            screen.openLibrarySearch();
            androidx.appcompat.widget.SearchView search = screen.findViewById(R.id.library_search);
            search.setQuery("Portal", false);
            // The Android 13+ system back gesture uses the dispatcher, not the legacy override.
            screen.getOnBackPressedDispatcher().onBackPressed();
            assertEquals(View.GONE, search.getVisibility());
            assertEquals("", search.getQuery().toString());
            assertFalse(screen.isFinishing());
            screen.getOnBackPressedDispatcher().onBackPressed();
            assertTrue(screen.isFinishing());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    static final String LIBRARY = "<root status_code=\"200\">" +
            "<App><AppTitle>Desktop</AppTitle><ID>1</ID></App>" +
            "<App><AppTitle>Racing Game</AppTitle><ID>2</ID></App>" +
            "<App><AppTitle>Puzzle Night</AppTitle><ID>3</ID></App></root>";

    /** A library whose host runs Racing Game, fed through the same polling path as a real host. */
    static android.widget.GridView libraryWithRunningGame(Library screen, String hostFixture) {
        ComputerDetails computer = new ComputerDetails();
        computer.uuid = "running-host";
        computer.name = "PC";
        computer.state = ComputerDetails.State.ONLINE;
        computer.pairState = com.limelight.nvstream.http.PairingManager.PairState.PAIRED;
        // A closed local port: artwork requests fail at once instead of reaching a network.
        computer.activeAddress = new ComputerDetails.AddressTuple("127.0.0.1", 1);
        computer.caps = com.limelight.nvstream.http.TestHosts.fixture(hostFixture);
        computer.runningGameId = 2;
        computer.rawAppList = LIBRARY;
        ReflectionHelpers.setField(screen, "computer", computer);
        ReflectionHelpers.setField(screen, "appGridAdapter",
                new AppGridAdapter(screen, new PreferenceConfiguration(), computer, "test", false));
        ComputerManagerListener[] listener = new ComputerManagerListener[1];
        ComputerManagerService manager = new ComputerManagerService();
        ReflectionHelpers.setField(screen, "managerBinder", manager.new ComputerManagerBinder() {
            @Override public void startPolling(ComputerManagerListener callback) { listener[0] = callback; }
            @Override public void stopPolling() { }
            @Override public String getUniqueId() { return "test"; }
            @Override public ComputerManagerService.ApplistPoller createAppListPoller(ComputerDetails details) {
                return manager.new ApplistPoller(details) {
                    @Override public void start() { }
                    @Override public void stop() { }
                    @Override public void pollNow() { }
                };
            }
        });
        ReflectionHelpers.callInstanceMethod(screen, "startComputerUpdates");
        listener[0].notifyComputerUpdated(computer);
        org.robolectric.Shadows.shadowOf(android.os.Looper.getMainLooper()).idle();
        android.widget.FrameLayout container = screen.findViewById(R.id.appFragmentContainer);
        screen.getLayoutInflater().inflate(R.layout.app_grid_view, container);
        android.widget.GridView grid = screen.findViewById(R.id.fragmentView);
        screen.receiveAbsListView(grid);
        return grid;
    }

    static int position(android.widget.GridView grid, String name) {
        for (int i = 0; i < grid.getCount(); i++) {
            if (((AppView.AppObject) grid.getItemAtPosition(i)).app.getAppName().equals(name)) return i;
        }
        throw new AssertionError("No tile for " + name);
    }

    static List<String> sheetFor(android.widget.GridView grid, String name) {
        int position = position(grid, name);
        grid.getOnItemLongClickListener().onItemLongClick(grid, null, position, position);
        com.limelight.ui.ActionSheet sheet = (com.limelight.ui.ActionSheet) org.robolectric.shadows.ShadowDialog.getLatestDialog();
        List<String> actions = new java.util.ArrayList<>();
        collect(sheet.findViewById(android.R.id.content), actions);
        sheet.dismiss();
        org.robolectric.Shadows.shadowOf(android.os.Looper.getMainLooper()).idle();
        return actions;
    }

    private static void collect(View view, List<String> actions) {
        if (view instanceof com.google.android.material.button.MaterialButton) {
            actions.add(((TextView) view).getText().toString());
        } else if (view instanceof android.view.ViewGroup) {
            for (int i = 0; i < ((android.view.ViewGroup) view).getChildCount(); i++) {
                collect(((android.view.ViewGroup) view).getChildAt(i), actions);
            }
        }
    }

    static com.google.android.material.button.MaterialButton sheetRow(android.app.Dialog sheet, String start) {
        List<com.google.android.material.button.MaterialButton> found = new java.util.ArrayList<>();
        findRows(sheet.findViewById(android.R.id.content), start, found);
        return found.isEmpty() ? null : found.get(0);
    }

    private static void findRows(View view, String start, List<com.google.android.material.button.MaterialButton> found) {
        if (view instanceof com.google.android.material.button.MaterialButton) {
            if (((TextView) view).getText().toString().startsWith(start)) {
                found.add((com.google.android.material.button.MaterialButton) view);
            }
        } else if (view instanceof android.view.ViewGroup) {
            for (int i = 0; i < ((android.view.ViewGroup) view).getChildCount(); i++) {
                findRows(((android.view.ViewGroup) view).getChildAt(i), start, found);
            }
        }
    }

    static android.app.Dialog openGameSheet(android.widget.GridView grid, String name) {
        int position = position(grid, name);
        grid.getOnItemLongClickListener().onItemLongClick(grid, null, position, position);
        return org.robolectric.shadows.ShadowDialog.getLatestDialog();
    }

    static void idle() {
        org.robolectric.Shadows.shadowOf(android.os.Looper.getMainLooper()).idle();
    }

    /** A PC's app grid on a 1080p TV: readable text, nothing read or focused in the overscan band. */
    static ActivityController<Library> runningLibrary() {
        Intent intent = new Intent(RuntimeEnvironment.getApplication(), Library.class)
                .putExtra(AppView.UUID_EXTRA, "running-host").putExtra(AppView.NAME_EXTRA, "PC");
        return Robolectric.buildActivity(Library.class, intent).setup().visible();
    }
}
