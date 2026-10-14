package com.limelight;

import java.io.IOException;
import java.io.StringReader;
import java.util.HashMap;
import java.util.HashSet;
import java.util.List;
import java.util.Map;

import com.limelight.computers.ComputerManagerListener;
import com.limelight.computers.ComputerManagerService;
import com.limelight.computers.HostCapabilityCache;
import com.limelight.grid.AppGridAdapter;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.http.NvHTTP;
import com.limelight.nvstream.http.PairingManager;
import com.limelight.preferences.PreferenceConfiguration;
import com.limelight.ui.AdapterFragment;
import com.limelight.ui.AdapterFragmentCallbacks;
import com.limelight.ui.TopAppBar;
import com.google.android.material.appbar.MaterialToolbar;
import com.limelight.utils.CacheHelper;
import com.limelight.utils.Dialog;
import com.limelight.utils.ServerHelper;
import com.limelight.utils.ShortcutHelper;
import androidx.appcompat.widget.SearchView;
import com.google.android.material.snackbar.Snackbar;
import com.limelight.utils.UiHelper;

import androidx.appcompat.app.AppCompatActivity;
import androidx.activity.OnBackPressedCallback;
import android.app.Service;
import android.content.ComponentName;
import android.content.Intent;
import android.content.ServiceConnection;
import android.content.SharedPreferences;
import android.content.res.Configuration;
import android.graphics.Bitmap;
import android.graphics.drawable.BitmapDrawable;
import android.os.Build;
import android.os.Bundle;
import android.os.IBinder;
import android.view.View;
import com.limelight.ui.ActionSheet;
import android.widget.AbsListView;
import android.widget.AdapterView;
import android.widget.AdapterView.OnItemClickListener;
import android.widget.ImageView;
import android.widget.TextView;
import android.widget.Toast;

import org.xmlpull.v1.XmlPullParserException;

public class AppView extends AppCompatActivity implements AdapterFragmentCallbacks {
    private AppGridAdapter appGridAdapter;
    private ActionSheet appMenu;
    private String uuidString;
    private ShortcutHelper shortcutHelper;

    private ComputerDetails computer;
    private ComputerManagerService.ApplistPoller poller;
    private SearchView librarySearch;
    private ActionSheet libraryMenu;
    private String searchQuery = "";
    private boolean loadingApps = true;
    private int libraryStatus;
    private boolean runningPolling;
    private String lastRawApplist;
    private boolean lastAppListEmpty;
    private int lastRunningAppId;
    private boolean suspendGridUpdates;
    private volatile boolean inForeground;
    private boolean showHiddenApps;
    private HashSet<Integer> hiddenAppIds = new HashSet<>();

    private final static int START_OR_RESUME_ID = 1;
    private final static int QUIT_ID = 2;
    private final static int START_WITH_QUIT = 4;
    private final static int VIEW_DETAILS_ID = 5;
    private final static int CREATE_SHORTCUT_ID = 6;
    private final static int HIDE_APP_ID = 7;

    public final static String HIDDEN_APPS_PREF_FILENAME = "HiddenApps";

    public final static String NAME_EXTRA = "Name";
    public final static String UUID_EXTRA = "UUID";
    public final static String NEW_PAIR_EXTRA = "NewPair";
    public final static String SHOW_HIDDEN_APPS_EXTRA = "ShowHiddenApps";

    private volatile ComputerManagerService.ComputerManagerBinder managerBinder;
    private boolean managerServiceBound;
    private Thread serviceWaitThread;
    private final ServiceConnection serviceConnection = new ServiceConnection() {
        public void onServiceConnected(ComponentName className, IBinder binder) {
            final ComputerManagerService.ComputerManagerBinder localBinder =
                    ((ComputerManagerService.ComputerManagerBinder)binder);

            // Wait in a separate thread to avoid stalling the UI
            serviceWaitThread = new Thread() {
                @Override
                public void run() {
                    // Wait for the binder to be ready
                    if (!localBinder.waitForReady() || isFinishing() || isDestroyed()) {
                        return;
                    }

                    // Get the computer object
                    computer = localBinder.getComputer(uuidString);
                    if (computer == null) {
                        finish();
                        return;
                    }

                    // Add a launcher shortcut for this PC (forced, since this is user interaction)
                    if (localBinder.isPersistent()) {
                        shortcutHelper.createAppViewShortcut(computer, true, getIntent().getBooleanExtra(NEW_PAIR_EXTRA, false));
                        shortcutHelper.reportComputerShortcutUsed(computer);
                    }

                    final AppGridAdapter adapter;
                    try {
                        adapter = new AppGridAdapter(AppView.this,
                                PreferenceConfiguration.readPreferences(AppView.this),
                                computer, localBinder.getUniqueId(),
                                showHiddenApps);
                    } catch (Exception e) {
                        e.printStackTrace();
                        finish();
                        return;
                    }

                    runOnUiThread(new Runnable() {
                        @Override
                        public void run() {
                            if (!managerServiceBound || isFinishing() || isDestroyed()) {
                                adapter.cancelQueuedOperations();
                                return;
                            }
                            appGridAdapter = adapter;
                            appGridAdapter.updateHiddenApps(hiddenAppIds, true);
                            appGridAdapter.setSearchQuery(searchQuery);
                            managerBinder = localBinder;
                            populateAppGridWithCache();
                            startComputerUpdates();
                            if (!inForeground) {
                                appGridAdapter.cancelQueuedOperations();
                            }

                            // Despite my best efforts to catch all conditions that could
                            // cause the activity to be destroyed when we try to commit
                            // I haven't been able to, so we have this try-catch block.
                            try {
                                getFragmentManager().beginTransaction()
                                        .replace(R.id.appFragmentContainer, new AdapterFragment())
                                        .commitAllowingStateLoss();
                            } catch (IllegalStateException e) {
                                e.printStackTrace();
                            }
                        }
                    });
                }
            };
            serviceWaitThread.start();
        }

        public void onServiceDisconnected(ComponentName className) {
            managerBinder = null;
            runningPolling = false;
            if (poller != null) poller.stop();
            poller = null;
            showAppListStatus(R.string.applist_load_failed);
        }
    };

    @Override
    public void onConfigurationChanged(Configuration newConfig) {
        super.onConfigurationChanged(newConfig);

        // If appGridAdapter is initialized, let it know about the configuration change.
        // If not, it will pick it up when it initializes.
        if (appGridAdapter != null) {
            // Update the app grid adapter to create grid items with the correct layout
            appGridAdapter.updateLayoutWithPreferences(this, PreferenceConfiguration.readPreferences(this));

            UiHelper.notifyNewRootView(this);
        }
    }

    private void startComputerUpdates() {
        // Don't start polling if we're not bound or in the foreground
        if (managerBinder == null || !inForeground || runningPolling) {
            return;
        }

        runningPolling = true;
        managerBinder.startPolling(new ComputerManagerListener() {
            @Override
            public void notifyAppListUpdated(ComputerDetails details, boolean success) {
                if (!details.uuid.equalsIgnoreCase(uuidString)) return;
                if (success && details.state != ComputerDetails.State.OFFLINE) {
                    notifyComputerUpdated(details);
                    runOnUiThread(() -> showAppListStatus(!details.hasPermission(ComputerDetails.PERMISSION_LIST) ?
                            R.string.applist_permission_required : lastAppListEmpty ? R.string.applist_empty : 0));
                } else {
                    showAppListStatus(details.state == ComputerDetails.State.OFFLINE ? R.string.library_offline :
                            details.hasPermission(ComputerDetails.PERMISSION_LIST) ?
                                    R.string.applist_load_failed : R.string.applist_permission_required);
                }
            }

            @Override
            public void notifyComputerUpdated(final ComputerDetails details) {
                // Do nothing if updates are suspended
                if (suspendGridUpdates || !inForeground || isFinishing() || isDestroyed()) {
                    return;
                }

                // Don't care about other computers
                if (!details.uuid.equalsIgnoreCase(uuidString)) {
                    return;
                }

                if (details.state == ComputerDetails.State.OFFLINE) {
                    showAppListStatus(R.string.library_offline);
                    return;
                }

                // Close immediately if the PC is no longer paired
                if (details.state == ComputerDetails.State.ONLINE && details.pairState != PairingManager.PairState.PAIRED) {
                    AppView.this.runOnUiThread(new Runnable() {
                        @Override
                        public void run() {
                            if (!inForeground || isFinishing() || isDestroyed()) {
                                return;
                            }
                            // Disable shortcuts referencing this PC for now
                            shortcutHelper.disableComputerShortcut(details,
                                    getResources().getString(R.string.scut_not_paired));

                            // Display a toast to the user and quit the activity
                            Toast.makeText(AppView.this, getResources().getText(R.string.scut_not_paired), Toast.LENGTH_SHORT).show();
                            finish();
                        }
                    });

                    return;
                }

                String rawAppList = details.rawAppList;

                if (!details.hasPermission(ComputerDetails.PERMISSION_LIST)) {
                    showAppListStatus(R.string.applist_permission_required);
                }
                else if (details.state == ComputerDetails.State.ONLINE) {
                    // The PC answered: an offline notice must not wait for the next app list poll.
                    clearOfflineStatus();
                }

                // App list is the same or empty
                if (rawAppList == null || rawAppList.equals(lastRawApplist)) {

                    // UUID changes and hidden/search-filtered apps also need their running state updated.
                    lastRunningAppId = details.runningGameId;
                    updateUiWithServerinfo(details);

                    return;
                }

                lastRunningAppId = details.runningGameId;
                lastRawApplist = rawAppList;

                try {
                    updateUiWithAppList(NvHTTP.getAppListByReader(new StringReader(rawAppList), details.caps));
                    updateUiWithServerinfo(details);

                } catch (XmlPullParserException | IOException e) {
                    e.printStackTrace();
                    showAppListStatus(R.string.applist_load_failed);
                }
            }
        });

        if (poller == null) {
            poller = managerBinder.createAppListPoller(computer);
        }
        poller.start();
    }

    private void stopComputerUpdates() {
        runningPolling = false;
        if (poller != null) {
            poller.stop();
        }

        if (managerBinder != null) {
            managerBinder.stopPolling();
        }

        if (appGridAdapter != null) {
            appGridAdapter.cancelQueuedOperations();
        }
    }

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);

        // Assume we're in the foreground when created to avoid a race
        // between binding to CMS and onResume()
        inForeground = true;

        shortcutHelper = new ShortcutHelper(this);

        UiHelper.setLocale(this);

        setContentView(R.layout.activity_app_view);
        MaterialToolbar toolbar = TopAppBar.configure(this, R.id.library_toolbar,
                getIntent().getStringExtra(NAME_EXTRA), R.menu.library);
        TopAppBar.setUpNavigation(toolbar, R.id.library_back, v -> finish());
        toolbar.setOnMenuItemClickListener(item -> {
            if (item.getItemId() == R.id.library_search_button) openLibrarySearch();
            else if (item.getItemId() == R.id.library_options) showLibraryOptions();
            return true;
        });
        librarySearch = findViewById(R.id.library_search);
        getOnBackPressedDispatcher().addCallback(this, new OnBackPressedCallback(true) {
            @Override public void handleOnBackPressed() { navigateBack(); }
        });
        librarySearch.setOnQueryTextListener(new SearchView.OnQueryTextListener() {
            @Override public boolean onQueryTextSubmit(String query) {
                librarySearch.clearFocus();
                View grid = findViewById(R.id.fragmentView);
                if (grid != null) grid.requestFocus();
                return true;
            }
            @Override public boolean onQueryTextChange(String query) {
                searchQuery = query;
                if (appGridAdapter != null) appGridAdapter.setSearchQuery(query);
                updateLibraryState();
                return true;
            }
        });

        // Allow floating expanded PiP overlays while browsing apps
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            setShouldDockBigOverlays(false);
        }

        UiHelper.notifyNewRootView(this);

        showHiddenApps = savedInstanceState != null ? savedInstanceState.getBoolean("showHiddenApps") :
                getIntent().getBooleanExtra(SHOW_HIDDEN_APPS_EXTRA, false);
        if (savedInstanceState != null) {
            searchQuery = savedInstanceState.getString("libraryQuery", "");
            librarySearch.setQuery(searchQuery, false);
            librarySearch.setVisibility(savedInstanceState.getBoolean("librarySearchVisible") ? View.VISIBLE : View.GONE);
            librarySearch.clearFocus();
        }
        uuidString = getIntent().getStringExtra(UUID_EXTRA);
        if (uuidString == null || uuidString.isEmpty()) {
            finish();
            return;
        }

        SharedPreferences hiddenAppsPrefs = getSharedPreferences(HIDDEN_APPS_PREF_FILENAME, MODE_PRIVATE);
        for (String hiddenAppIdStr : hiddenAppsPrefs.getStringSet(uuidString, new HashSet<String>())) {
            try {
                hiddenAppIds.add(Integer.parseInt(hiddenAppIdStr));
            } catch (NumberFormatException ignored) {
                // Ignore a stale or malformed saved entry without making the library unusable.
            }
        }

        String computerName = getIntent().getStringExtra(NAME_EXTRA);

        setTitle(computerName);

        // Bind to the computer manager service
        managerServiceBound = bindService(new Intent(this, ComputerManagerService.class), serviceConnection,
                Service.BIND_AUTO_CREATE);
        if (!managerServiceBound) showAppListStatus(R.string.applist_load_failed);
    }

    private void updateHiddenApps(boolean hideImmediately) {
        HashSet<String> hiddenAppIdStringSet = new HashSet<>();

        for (Integer hiddenAppId : hiddenAppIds) {
            hiddenAppIdStringSet.add(hiddenAppId.toString());
        }

        getSharedPreferences(HIDDEN_APPS_PREF_FILENAME, MODE_PRIVATE)
                .edit()
                .putStringSet(uuidString, hiddenAppIdStringSet)
                .apply();

        appGridAdapter.updateHiddenApps(hiddenAppIds, hideImmediately);
        updateLibraryState();
    }

    private void populateAppGridWithCache() {
        try {
            // Try to load from cache
            lastRawApplist = CacheHelper.readInputStreamToString(CacheHelper.openCacheFileForInput(getCacheDir(), "applist", uuidString));
            List<NvApp> applist = NvHTTP.getAppListByReader(new StringReader(lastRawApplist),
                    HostCapabilityCache.load(this, uuidString));
            updateUiWithAppList(applist);
            LimeLog.info("Loaded applist from cache");
        } catch (IOException | XmlPullParserException e) {
            if (lastRawApplist != null) {
                LimeLog.warning("Saved applist corrupted");
                e.printStackTrace();
            }
            LimeLog.info("Loading applist from the network");
            // We'll need to load from the network
            loadAppsBlocking();
        }
    }

    // Keep navigation and any cached apps usable while the host responds.
    private void loadAppsBlocking() {
        loadingApps = true;
        libraryStatus = 0;
        updateLibraryState();
    }

    private void showAppListStatus(int message) {
        runOnUiThread(() -> {
            if (!inForeground || isFinishing() || isDestroyed()) return;
            loadingApps = false;
            libraryStatus = message;
            updateLibraryState();
        });
    }

    private void clearOfflineStatus() {
        runOnUiThread(() -> {
            if (libraryStatus == R.string.library_offline) {
                showAppListStatus(lastAppListEmpty ? R.string.applist_empty : 0);
            }
        });
    }

    private void updateLibraryState() {
        if (librarySearch == null) return;
        int visible = appGridAdapter == null ? 0 : appGridAdapter.getCount();
        int total = appGridAdapter == null ? 0 : appGridAdapter.getLibraryCount();
        TextView summary = findViewById(R.id.library_summary);
        summary.setVisibility(appGridAdapter != null && !appGridAdapter.getAllApps().isEmpty() ? View.VISIBLE : View.GONE);
        summary.setText(!searchQuery.trim().isEmpty() ? getString(R.string.library_results, visible, total) :
                showHiddenApps ? getString(R.string.library_showing_hidden, visible) :
                        getResources().getQuantityString(R.plurals.library_app_count, visible, visible));
        findViewById(R.id.library_loading).setVisibility(loadingApps ? View.VISIBLE : View.GONE);

        int message = libraryStatus;
        int action = R.string.library_retry;
        View.OnClickListener retry = v -> refreshLibrary();
        if (loadingApps && visible == 0) {
            message = R.string.library_loading;
        } else if (message == 0 && visible == 0 && appGridAdapter != null) {
            if (!searchQuery.trim().isEmpty()) {
                message = R.string.library_no_matches;
                action = R.string.library_clear_search;
                retry = v -> librarySearch.setQuery("", false);
            } else if (!appGridAdapter.getAllApps().isEmpty()) {
                message = R.string.library_all_hidden;
                action = R.string.library_show_hidden;
                retry = v -> setShowHiddenApps(true);
            }
        }
        TextView status = findViewById(R.id.app_list_status);
        status.setVisibility(message == 0 ? View.GONE : View.VISIBLE);
        if (message != 0) status.setText(message);
        findViewById(R.id.library_status_panel).setVisibility(message == 0 ? View.GONE : View.VISIBLE);
        TextView retryButton = findViewById(R.id.library_retry);
        retryButton.setText(action);
        retryButton.setOnClickListener(retry);
        retryButton.setVisibility(message != 0 && !loadingApps ? View.VISIBLE : View.GONE);
    }

    public void openLibrarySearch() {
        librarySearch.setVisibility(View.VISIBLE);
        librarySearch.setIconified(false);
        librarySearch.requestFocus();
    }

    private void refreshLibrary() {
        if (poller == null) {
            showAppListStatus(R.string.applist_load_failed);
            return;
        }
        loadAppsBlocking();
        poller.pollNow();
    }

    private void setShowHiddenApps(boolean show) {
        showHiddenApps = show;
        if (appGridAdapter != null) appGridAdapter.setShowHiddenApps(show);
        updateLibraryState();
    }

    private void showLibraryOptions() {
        if (libraryMenu != null) return;
        ActionSheet sheet = new ActionSheet(this, getString(R.string.library_options));
        libraryMenu = sheet;
        sheet.addAction(R.drawable.ic_library, getString(R.string.library_refresh)).setOnClickListener(v -> {
            sheet.dismiss();
            refreshLibrary();
        });
        sheet.addAction(R.drawable.ic_library, getString(showHiddenApps ? R.string.library_hide_hidden :
                R.string.library_show_hidden)).setOnClickListener(v -> {
            sheet.dismiss();
            setShowHiddenApps(!showHiddenApps);
        });
        sheet.setOnDismissListener(dialog -> libraryMenu = null);
        sheet.show();
    }

    @Override protected void onSaveInstanceState(Bundle state) {
        super.onSaveInstanceState(state);
        state.putString("libraryQuery", searchQuery);
        state.putBoolean("showHiddenApps", showHiddenApps);
        state.putBoolean("librarySearchVisible", librarySearch != null && librarySearch.getVisibility() == View.VISIBLE);
    }

    private void navigateBack() {
        if (librarySearch != null && librarySearch.getVisibility() == View.VISIBLE) {
            librarySearch.setQuery("", false);
            librarySearch.clearFocus();
            librarySearch.setVisibility(View.GONE);
            findViewById(R.id.library_search_button).requestFocus();
        } else {
            finish();
        }
    }

    @Override
    protected void onDestroy() {
        super.onDestroy();

        Dialog.closeDialogs(AppView.this);

        if (serviceWaitThread != null) {
            serviceWaitThread.interrupt();
        }
        if (managerServiceBound) {
            unbindService(serviceConnection);
            managerServiceBound = false;
        }
        managerBinder = null;
    }

    @Override
    protected void onResume() {
        super.onResume();

        // Display a decoder crash notification if we've returned after a crash
        UiHelper.showDecoderCrashDialog(this);

        inForeground = true;
        startComputerUpdates();
        if (appGridAdapter != null) appGridAdapter.notifyDataSetChanged();
        updateLibraryState();
    }

    @Override
    protected void onPause() {
        super.onPause();

        inForeground = false;
        if (appMenu != null) appMenu.dismiss();
        if (libraryMenu != null) libraryMenu.dismiss();
        stopComputerUpdates();
    }

    private void showAppMenu(AppObject selectedApp, View selectedView) {
        if (appMenu != null) return;
        ActionSheet sheet = new ActionSheet(this, selectedApp.app.getAppName());
        appMenu = sheet;
        suspendGridUpdates = true;
        if (selectedApp.app.getControl() != NvApp.Control.NONE) {
            addAppAction(sheet, selectedApp, selectedView, START_OR_RESUME_ID,
                    R.drawable.ic_play, selectedApp.app.getAppName().trim());
        } else if (lastRunningAppId != 0) {
            if (selectedApp.isRunning) {
                addAppAction(sheet, selectedApp, selectedView, START_OR_RESUME_ID,
                        R.drawable.ic_play, getString(R.string.applist_menu_resume));
                if (computer.hasPermission(ComputerDetails.PERMISSION_LAUNCH)) {
                    addAppAction(sheet, selectedApp, selectedView, QUIT_ID,
                            R.drawable.ic_stop, getString(R.string.applist_menu_quit));
                }
            } else if (computer.hasPermission(ComputerDetails.PERMISSION_LAUNCH)) {
                addAppAction(sheet, selectedApp, selectedView, START_WITH_QUIT,
                        R.drawable.ic_play, getString(R.string.applist_menu_quit_and_start));
            }
        }
        else if (computer.hasPermission(ComputerDetails.PERMISSION_LAUNCH)) {
            addAppAction(sheet, selectedApp, selectedView, START_OR_RESUME_ID,
                    R.drawable.ic_play, getString(R.string.library_play));
        }
        if (!selectedApp.isRunning || selectedApp.isHidden) {
            addAppAction(sheet, selectedApp, selectedView, HIDE_APP_ID, R.drawable.ic_library,
                    getString(selectedApp.isHidden ? R.string.applist_menu_show_app : R.string.applist_menu_hide_app));
        }
        addAppAction(sheet, selectedApp, selectedView, VIEW_DETAILS_ID,
                R.drawable.ic_help, getString(R.string.applist_menu_details));
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O && selectedView != null) {
            ImageView image = selectedView.findViewById(R.id.grid_image);
            if (image != null && image.getDrawable() instanceof BitmapDrawable &&
                    ((BitmapDrawable) image.getDrawable()).getBitmap() != null) {
                addAppAction(sheet, selectedApp, selectedView, CREATE_SHORTCUT_ID,
                        R.drawable.ic_add, getString(R.string.applist_menu_scut));
            }
        }
        sheet.setOnDismissListener(dialog -> {
            appMenu = null;
            suspendGridUpdates = false;
            if (poller != null && inForeground) poller.pollNow();
        });
        sheet.show();
    }

    private void addAppAction(ActionSheet sheet, AppObject app, View view, int action, int icon, String label) {
        sheet.addAction(icon, label).setOnClickListener(v -> {
            sheet.dismiss();
            onAppAction(app, view, action);
        });
    }

    private boolean onAppAction(AppObject app, View view, int action) {
        if ((action == START_OR_RESUME_ID || action == START_WITH_QUIT || action == QUIT_ID) &&
                (computer == null || managerBinder == null || computer.state == ComputerDetails.State.OFFLINE)) {
            showAppListStatus(R.string.library_offline);
            return true;
        }
        switch (action) {
            case START_WITH_QUIT:
                // Display a confirmation dialog first
                UiHelper.displayQuitConfirmationDialog(this, new Runnable() {
                    @Override
                    public void run() {
                        ServerHelper.doStart(AppView.this, app.app, computer, managerBinder);
                    }
                }, null);
                return true;

            case START_OR_RESUME_ID:
                // Resume is the same as start for us
                ServerHelper.doStart(AppView.this, app.app, computer, managerBinder);
                return true;

            case QUIT_ID:
                // Display a confirmation dialog first
                UiHelper.displayQuitConfirmationDialog(this, new Runnable() {
                    @Override
                    public void run() {
                        suspendGridUpdates = true;
                        ServerHelper.doQuit(AppView.this, computer,
                                app.app, managerBinder, new Runnable() {
                            @Override
                            public void run() {
                                // Trigger a poll immediately
                                suspendGridUpdates = false;
                                if (poller != null) {
                                    poller.pollNow();
                                }
                            }
                        });
                    }
                }, null);
                return true;

            case VIEW_DETAILS_ID:
                Dialog.displayDialog(AppView.this, getString(R.string.title_details),
                        getString(R.string.app_details, app.app.getAppName(),
                                getString(app.app.isHdrSupported() ? R.string.yes : R.string.app_hdr_unknown),
                                app.app.getAppId()), false);
                return true;

            case HIDE_APP_ID:
                if (app.isHidden) {
                    // Transitioning hidden to shown
                    hiddenAppIds.remove(app.app.getAppId());
                }
                else {
                    // Transitioning shown to hidden
                    hiddenAppIds.add(app.app.getAppId());
                }
                boolean wasHidden = app.isHidden;
                updateHiddenApps(true);
                if (!wasHidden) {
                    Snackbar.make(findViewById(R.id.appFragmentContainer), R.string.library_hidden_notice, Snackbar.LENGTH_LONG)
                            .setAction(R.string.library_undo, v -> {
                                hiddenAppIds.remove(app.app.getAppId());
                                updateHiddenApps(true);
                            }).show();
                }
                return true;

            case CREATE_SHORTCUT_ID:
                ImageView appImageView = view == null ? null : view.findViewById(R.id.grid_image);
                if (appImageView == null || !(appImageView.getDrawable() instanceof BitmapDrawable)) return true;
                Bitmap appBits = ((BitmapDrawable)appImageView.getDrawable()).getBitmap();
                if (appBits == null) return true;
                if (!shortcutHelper.createPinnedGameShortcut(computer, app.app, appBits)) {
                    Toast.makeText(AppView.this, getResources().getString(R.string.unable_to_pin_shortcut), Toast.LENGTH_LONG).show();
                }
                return true;

            default:
                return false;
        }
    }

    private void updateUiWithServerinfo(final ComputerDetails details) {
        AppView.this.runOnUiThread(new Runnable() {
            @Override
            public void run() {
                if (isFinishing() || isDestroyed()) {
                    return;
                }
                if (appGridAdapter == null) return;
                boolean updated = false;

                // Update the full catalogue so a filtered or hidden running app never keeps a stale badge.
                for (AppObject app : appGridAdapter.getAllApps()) {
                    boolean running = app.app.matchesRunningApp(details.runningGameId, details.runningGameUuid);
                    if (app.isRunning != running) {
                        app.isRunning = running;
                        updated = true;
                    }
                }

                if (updated) {
                    appGridAdapter.notifyDataSetChanged();
                }
            }
        });
    }

    private void updateUiWithAppList(final List<NvApp> appList) {
        AppView.this.runOnUiThread(new Runnable() {
            @Override
            public void run() {
                if (isFinishing() || isDestroyed()) {
                    return;
                }
                lastAppListEmpty = appList.isEmpty();
                showAppListStatus(!computer.hasPermission(ComputerDetails.PERMISSION_LIST) ?
                        R.string.applist_permission_required : appList.isEmpty() ? R.string.applist_empty : 0);
                boolean updated = false;
                List<AppObject> removedApps = appGridAdapter.getAllApps();
                Map<NvApp, AppObject> matches = matchApps(appList, removedApps);
                HashSet<Integer> remappedIds = new HashSet<>();
                HashSet<Integer> remappedHiddenIds = new HashSet<>();

                // First handle app updates and additions
                for (NvApp app : appList) {
                    AppObject existingApp = matches.get(app);
                    if (existingApp != null) {
                        if (existingApp.app.getAppId() != app.getAppId()) {
                            remappedIds.add(existingApp.app.getAppId());
                            if (existingApp.isHidden) remappedHiddenIds.add(app.getAppId());
                        }
                        // Found the app; update its properties
                        if (existingApp.app.getAppId() != app.getAppId() ||
                                !existingApp.app.getAppName().equals(app.getAppName()) ||
                                !existingApp.app.getAppUuid().equals(app.getAppUuid()) ||
                                existingApp.app.getHostIndex() != app.getHostIndex() ||
                                !existingApp.app.getArtVersion().equals(app.getArtVersion()) ||
                                existingApp.app.getRole() != app.getRole() ||
                                existingApp.app.isHdrSupported() != app.isHdrSupported()) {
                            existingApp.app = app;
                            updated = true;
                        }
                    } else {
                        // This app must be new
                        appGridAdapter.addApp(new AppObject(app));

                        // We could have a leftover shortcut from last time this PC was paired
                        // or if this app was removed then added again. Enable those shortcuts
                        // again if present.
                        shortcutHelper.enableAppShortcut(computer, app);

                        updated = true;
                    }
                }

                // Next handle app removals
                for (AppObject existingApp : removedApps) {
                    for (NvApp app : appList) {
                        if (existingApp.app.getAppId() == app.getAppId() &&
                                !existingApp.app.getAppUuid().isEmpty() && !app.getAppUuid().isEmpty() &&
                                !existingApp.app.getAppUuid().equals(app.getAppUuid())) {
                            // A different known game owns this number now; it must not inherit hiding.
                            remappedIds.add(existingApp.app.getAppId());
                            break;
                        }
                    }
                    shortcutHelper.disableAppShortcut(computer, existingApp.app, "App removed from PC");
                    appGridAdapter.removeApp(existingApp);
                    updated = true;
                }

                if (!remappedIds.isEmpty()) {
                    // Apply all ID moves together so hosts swapping IDs cannot lose a hidden choice.
                    hiddenAppIds.removeAll(remappedIds);
                    hiddenAppIds.addAll(remappedHiddenIds);
                    updateHiddenApps(true);
                } else if (updated) {
                    appGridAdapter.notifyDataSetChanged();
                }
                updateLibraryState();
            }
        });
    }

    private static Map<NvApp, AppObject> matchApps(List<NvApp> apps, List<AppObject> unmatched) {
        Map<NvApp, AppObject> matches = new HashMap<>();
        // Resolve UUIDs before legacy numbers, using the original catalogue throughout. Otherwise
        // an ID move can make a legacy item consume a known game's identity later in the same poll.
        for (int pass = 0; pass < 2; pass++) {
            for (NvApp app : apps) {
                if (matches.containsKey(app)) continue;
                for (int i = 0; i < unmatched.size(); i++) {
                    NvApp previous = unmatched.get(i).app;
                    boolean bothHaveUuid = !previous.getAppUuid().isEmpty() && !app.getAppUuid().isEmpty();
                    boolean match = pass == 0 ? bothHaveUuid && previous.getAppUuid().equals(app.getAppUuid()) :
                            !bothHaveUuid && previous.getAppId() == app.getAppId();
                    if (match) {
                        matches.put(app, unmatched.remove(i));
                        break;
                    }
                }
            }
        }
        return matches;
    }

    @Override
    public int getAdapterFragmentLayoutId() {
        return PreferenceConfiguration.readPreferences(AppView.this).smallIconMode ?
                    R.layout.app_grid_view_small : R.layout.app_grid_view;
    }

    @Override
    public void receiveAbsListView(AbsListView listView) {
        int columnDp = PreferenceConfiguration.readPreferences(this).smallIconMode ? 104 : 160;
        float fontScale = Math.max(1, getResources().getConfiguration().fontScale);
        ((android.widget.GridView) listView).setColumnWidth(Math.round(columnDp * fontScale *
                getResources().getDisplayMetrics().density));
        listView.setAdapter(appGridAdapter);
        listView.setOnItemClickListener(new OnItemClickListener() {
            @Override
            public void onItemClick(AdapterView<?> arg0, View arg1, int pos,
                                    long id) {
                AppObject app = (AppObject) appGridAdapter.getItem(pos);

                if (computer == null || managerBinder == null || computer.state == ComputerDetails.State.OFFLINE) {
                    showAppListStatus(R.string.library_offline);
                    return;
                }
                // Only open the context menu if something is running, otherwise start it
                if (lastRunningAppId != 0 && app.app.getControl() == NvApp.Control.NONE) {
                    showAppMenu(app, arg1);
                } else {
                    ServerHelper.doStart(AppView.this, app.app, computer, managerBinder);
                }
            }
        });
        listView.setOnItemLongClickListener((parent, view, position, id) -> {
            showAppMenu((AppObject) appGridAdapter.getItem(position), view);
            return true;
        });
        listView.requestFocus();
    }

    public static class AppObject {
        public NvApp app;
        public boolean isRunning;
        public boolean isHidden;

        public AppObject(NvApp app) {
            if (app == null) {
                throw new IllegalArgumentException("app must not be null");
            }
            this.app = app;
        }

        @Override
        public String toString() {
            return app.getAppName();
        }
    }
}
