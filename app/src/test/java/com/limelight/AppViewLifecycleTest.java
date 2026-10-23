package com.limelight;

import android.app.Application;
import android.content.Intent;
import android.content.ServiceConnection;
import android.view.View;
import android.widget.TextView;
import com.limelight.grid.AppGridAdapter;
import com.limelight.computers.ComputerManagerListener;
import com.limelight.computers.ComputerManagerService;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.preferences.PreferenceConfiguration;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.util.ReflectionHelpers;
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
                assertEquals(View.VISIBLE, screen.findViewById(R.id.library_retry).getVisibility());
            TextView status = screen.findViewById(R.id.app_list_status);
            assertEquals(screen.getString(R.string.applist_empty), status.getText().toString());
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

    private static final String LIBRARY = "<root status_code=\"200\">" +
            "<App><AppTitle>Desktop</AppTitle><ID>1</ID></App>" +
            "<App><AppTitle>Racing Game</AppTitle><ID>2</ID></App>" +
            "<App><AppTitle>Puzzle Night</AppTitle><ID>3</ID></App></root>";

    /** A library whose host runs Racing Game, fed through the same polling path as a real host. */
    private static android.widget.GridView libraryWithRunningGame(Library screen, String hostFixture) {
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

    private static int position(android.widget.GridView grid, String name) {
        for (int i = 0; i < grid.getCount(); i++) {
            if (((AppView.AppObject) grid.getItemAtPosition(i)).app.getAppName().equals(name)) return i;
        }
        throw new AssertionError("No tile for " + name);
    }

    private static List<String> sheetFor(android.widget.GridView grid, String name) {
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

    @Test public void aControllersXOpensTheSelectedGamesMenuLikeALongPress() {
        Intent intent = new Intent(RuntimeEnvironment.getApplication(), Library.class)
                .putExtra(AppView.UUID_EXTRA, "running-host").putExtra(AppView.NAME_EXTRA, "PC");
        ActivityController<Library> controller = Robolectric.buildActivity(Library.class, intent).setup();
        try {
            Library screen = controller.get();
            android.widget.GridView grid = libraryWithRunningGame(screen, "sunshine-serverinfo.xml");
            com.limelight.ui.UiNavigation.bindGrid(grid);
            grid.requestFocus();
            grid.setSelection(position(grid, "Desktop"));
            org.robolectric.Shadows.shadowOf(android.os.Looper.getMainLooper()).idle();
            long now = android.os.SystemClock.uptimeMillis();
            for (int action : new int[] {android.view.KeyEvent.ACTION_DOWN, android.view.KeyEvent.ACTION_UP}) {
                grid.dispatchKeyEvent(new android.view.KeyEvent(now, now, action, android.view.KeyEvent.KEYCODE_BUTTON_X, 0));
            }
            org.robolectric.Shadows.shadowOf(android.os.Looper.getMainLooper()).idle();
            android.app.Dialog dialog = org.robolectric.shadows.ShadowDialog.getLatestDialog();
            assertTrue(dialog instanceof com.limelight.ui.ActionSheet && dialog.isShowing());
            dialog.dismiss();
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test public void theRunningGameIsPinnedFirstWithARunningChipAndNoPlayLabels() {
        Intent intent = new Intent(RuntimeEnvironment.getApplication(), Library.class)
                .putExtra(AppView.UUID_EXTRA, "running-host").putExtra(AppView.NAME_EXTRA, "PC");
        ActivityController<Library> controller = Robolectric.buildActivity(Library.class, intent).setup();
        try {
            Library screen = controller.get();
            android.widget.GridView grid = libraryWithRunningGame(screen, "sunshine-serverinfo.xml");
            assertEquals("Racing Game", ((AppView.AppObject) grid.getItemAtPosition(0)).app.getAppName());
            View tile = grid.getAdapter().getView(0, null, grid);
            TextView chip = tile.findViewById(R.id.grid_status);
            assertEquals(View.VISIBLE, chip.getVisibility());
            assertEquals(screen.getString(R.string.app_running), chip.getText().toString());
            assertEquals("Racing Game, " + screen.getString(R.string.app_running), tile.getContentDescription().toString());
            View other = grid.getAdapter().getView(position(grid, "Desktop"), null, grid);
            assertEquals(View.GONE, ((TextView) other.findViewById(R.id.grid_status)).getVisibility());
            assertEquals("Desktop", other.getContentDescription().toString());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test public void startingAnotherGameAsksFirstWhereTheClientQuitsTheRunningOne() {
        Intent intent = new Intent(RuntimeEnvironment.getApplication(), Library.class)
                .putExtra(AppView.UUID_EXTRA, "running-host").putExtra(AppView.NAME_EXTRA, "PC");
        ActivityController<Library> controller = Robolectric.buildActivity(Library.class, intent).setup();
        try {
            Library screen = controller.get();
            android.widget.GridView grid = libraryWithRunningGame(screen, "sunshine-serverinfo.xml");
            List<String> others = sheetFor(grid, "Puzzle Night");
            assertEquals(screen.getString(R.string.library_quit_named_and_play, "Racing Game"), others.get(0));
            assertTrue(others.contains(screen.getString(R.string.applist_menu_hide_app)));
            assertEquals(screen.getString(R.string.applist_menu_details), others.get(others.size() - 1));
            List<String> running = sheetFor(grid, "Racing Game");
            assertEquals(List.of(screen.getString(R.string.applist_menu_resume), screen.getString(R.string.applist_menu_quit)),
                    running.subList(0, 2));
            assertFalse(running.contains(screen.getString(R.string.applist_menu_hide_app)));

            int puzzle = position(grid, "Puzzle Night");
            grid.performItemClick(null, puzzle, puzzle);
            androidx.appcompat.app.AlertDialog dialog =
                    (androidx.appcompat.app.AlertDialog) org.robolectric.shadows.ShadowDialog.getLatestDialog();
            assertEquals(screen.getString(R.string.library_replace_message_named, "Racing Game", "Puzzle Night"),
                    ((TextView) dialog.findViewById(android.R.id.message)).getText().toString());
            assertNull(org.robolectric.Shadows.shadowOf(screen).getNextStartedActivity());
            dialog.getButton(androidx.appcompat.app.AlertDialog.BUTTON_POSITIVE).performClick();
            org.robolectric.Shadows.shadowOf(android.os.Looper.getMainLooper()).idle();
            assertEquals(Game.class.getName(),
                    org.robolectric.Shadows.shadowOf(screen).getNextStartedActivity().getComponent().getClassName());

            // The running game itself resumes on a tap.
            grid.performItemClick(null, 0, 0);
            assertEquals(Game.class.getName(),
                    org.robolectric.Shadows.shadowOf(screen).getNextStartedActivity().getComponent().getClassName());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test public void aHostThatConfirmsReplacementIsLeftToAskItself() {
        Intent intent = new Intent(RuntimeEnvironment.getApplication(), Library.class)
                .putExtra(AppView.UUID_EXTRA, "running-host").putExtra(AppView.NAME_EXTRA, "PC");
        ActivityController<Library> controller = Robolectric.buildActivity(Library.class, intent).setup();
        try {
            Library screen = controller.get();
            android.widget.GridView grid = libraryWithRunningGame(screen, "rubylight-serverinfo.xml");
            String first = sheetFor(grid, "Puzzle Night").get(0);
            assertEquals(screen.getString(R.string.library_play) + "\n" +
                    screen.getString(R.string.library_host_asks_named, "Racing Game"), first);
            org.robolectric.shadows.ShadowDialog.reset();
            int puzzle = position(grid, "Puzzle Night");
            grid.performItemClick(null, puzzle, puzzle);
            assertNull("The host asks; the client does not", org.robolectric.shadows.ShadowDialog.getLatestDialog());
            assertEquals(Game.class.getName(),
                    org.robolectric.Shadows.shadowOf(screen).getNextStartedActivity().getComponent().getClassName());
        } finally {
            controller.pause().stop().destroy();
        }
    }
}
