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

}
