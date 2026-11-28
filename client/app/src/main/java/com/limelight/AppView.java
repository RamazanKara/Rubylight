package com.limelight;

import java.io.IOException;
import java.io.StringReader;
import java.util.HashSet;
import java.util.List;
import java.util.Set;

import com.limelight.computers.ComputerManagerListener;
import com.limelight.computers.ComputerManagerService;
import com.limelight.grid.AppGridAdapter;
import com.limelight.nativeapi.HiddenStore;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.http.NvHTTP;
import com.limelight.nvstream.http.PairingManager;
import com.limelight.preferences.PreferenceConfiguration;
import com.limelight.ui.AdapterFragment;
import com.limelight.ui.AdapterFragmentCallbacks;
import com.limelight.ui.GlassForm;
import com.limelight.ui.NightjarActivity;
import com.limelight.utils.Dialog;
import com.limelight.utils.FrontendSync;
import com.limelight.utils.LaunchSource;
import com.limelight.utils.ServerHelper;
import com.limelight.utils.ShortcutHelper;
import com.limelight.utils.UiHelper;

import androidx.activity.OnBackPressedCallback;
import android.app.Service;
import android.content.ComponentName;
import android.content.Intent;
import android.content.ServiceConnection;
import android.content.res.Configuration;
import android.os.Build;
import android.os.Bundle;
import android.os.Handler;
import android.os.IBinder;
import android.os.Looper;
import android.view.View;
import android.widget.AbsListView;
import android.widget.Toast;

import org.xmlpull.v1.XmlPullParserException;

public class AppView extends NightjarActivity implements AdapterFragmentCallbacks {
    final AppViewChrome viewChrome = new AppViewChrome(this);
    private final AppViewMenu viewMenu = new AppViewMenu(this);

    AppGridAdapter appGridAdapter;
    boolean enterPlayed;
    String uuidString;
    ShortcutHelper shortcutHelper;

    ComputerDetails computer;
    ComputerManagerService.ApplistPoller poller;
    private final Object pollerLock = new Object();
    private final Handler mainHandler = new Handler(Looper.getMainLooper());
    private static final long REFRESH_TIMEOUT_MS = 15_000L;
    private final Runnable refreshTimeout = this::onRefreshTimeout;

    private void onRefreshTimeout() {
        if (loadingApps && inForeground && !isFinishing() && !isDestroyed()) {
            showAppListStatus(computer != null && computer.state == ComputerDetails.State.OFFLINE
                    ? R.string.library_offline : R.string.applist_load_failed);
        }
    }

    void pollLibraryNow() {
        pollLibraryIfReady();
    }
    String searchQuery = "";
    boolean loadingApps = true;
    int libraryStatus;
    private boolean runningPolling;
    String lastRawApplist;
    boolean lastAppListEmpty;
    // The host reports a running game, possibly by its UUID alone (Apollo's input-only mode).
    boolean hostRunningGame;
    boolean suspendGridUpdates;
    volatile boolean inForeground;
    boolean showHiddenApps;
    HashSet<Integer> hiddenAppIds = new HashSet<>();
    /** What {@link #hiddenAppIds} was when last read from or written to the list. */
    private HashSet<Integer> savedHiddenAppIds = new HashSet<>();

    public final static String HIDDEN_APPS_PREF_FILENAME = "HiddenApps";

    public final static String NAME_EXTRA = "Name";
    public final static String UUID_EXTRA = "UUID";
    public final static String NEW_PAIR_EXTRA = "NewPair";
    public final static String SHOW_HIDDEN_APPS_EXTRA = "ShowHiddenApps";

    volatile ComputerManagerService.ComputerManagerBinder managerBinder;
    private boolean managerServiceBound;
    private Thread serviceWaitThread;
    private final ServiceConnection serviceConnection = new ServiceConnection() {
        public void onServiceConnected(ComponentName className, IBinder binder) {
            // No binder (as under Robolectric) leaves nothing to wait for.
            if (!(binder instanceof ComputerManagerService.ComputerManagerBinder)) return;
            final ComputerManagerService.ComputerManagerBinder localBinder =
                    ((ComputerManagerService.ComputerManagerBinder)binder);
            com.limelight.console.HostFeed.invalidateRecentStreams(localBinder);

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

                    runOnUiThread(() -> {
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
                    });
                }
            };
            serviceWaitThread.start();
        }

        public void onServiceDisconnected(ComponentName className) {
            managerBinder = null;
            runningPolling = false;
            stopAndClearPoller();
            showAppListStatus(R.string.applist_load_failed);
        }
    };

    // A Nightjar layout, or an Artemis keyboard layout and/or Custom Keys file, for the game the menu was opened on.
    // Kept across recreation while the picker is open.
    private String controlsProfile, controlsGame;
    private final androidx.activity.result.ActivityResultLauncher<String[]> controlsPicker = registerForActivityResult(
            new androidx.activity.result.contract.ActivityResultContracts.OpenMultipleDocuments(), this::importControls);

    @Override
    public void onConfigurationChanged(Configuration newConfig) {
        super.onConfigurationChanged(newConfig);
        if (recreatingForForm()) return;

        // If appGridAdapter is initialized, let it know about the configuration change.
        // If not, it will pick it up when it initializes.
        if (appGridAdapter != null) {
            // Update the app grid adapter to create grid items with the correct layout
            appGridAdapter.updateLayoutWithPreferences(this, PreferenceConfiguration.readPreferences(this));

            UiHelper.notifyNewRootView(this);
        }
    }

    private void stopAndClearPoller() {
        ComputerManagerService.ApplistPoller oldPoller;
        synchronized (pollerLock) {
            oldPoller = poller;
            poller = null;
        }
        if (oldPoller != null) oldPoller.stop();
    }

    void pollLibraryIfReady() {
        ComputerManagerService.ApplistPoller current;
        synchronized (pollerLock) {
            current = poller;
        }
        if (current != null && !current.needsRestart()) {
            current.pollNow();
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
                            R.string.applist_permission_required : lastAppListEmpty ? R.string.library_no_apps : 0));
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

                // Refresh our live computer snapshot (address, ports, state, caps, running game) before
                // handling state transitions so launches, cover art, and applist polling never stay stale.
                ComputerDetails previous = computer;
                boolean endpointChanged = com.limelight.nativeapi.HostEndpoint.endpointChanged(previous, details);
                boolean becameOnline = previous != null && previous.state != ComputerDetails.State.ONLINE
                        && details.state == ComputerDetails.State.ONLINE;
                ComputerDetails updated = new ComputerDetails(details);
                if (updated.rawAppList == null && previous != null) {
                    updated.rawAppList = previous.rawAppList;
                }
                computer = updated;
                if (appGridAdapter != null) {
                    appGridAdapter.updateComputer(updated);
                }
                ComputerManagerService.ComputerManagerBinder binder = managerBinder;
                if (runningPolling && binder != null && details.state == ComputerDetails.State.ONLINE) {
                    ComputerManagerService.ApplistPoller oldPoller = null;
                    synchronized (pollerLock) {
                        if (runningPolling && (poller == null || endpointChanged || becameOnline || poller.needsRestart())) {
                            oldPoller = poller;
                            poller = binder.createAppListPoller(updated);
                            poller.start();
                        }
                    }
                    if (oldPoller != null) oldPoller.stop();
                }

                if (details.state == ComputerDetails.State.OFFLINE) {
                    showAppListStatus(R.string.library_offline);
                    return;
                }

                // Close immediately if the PC is no longer paired
                if (details.state == ComputerDetails.State.ONLINE && details.pairState != PairingManager.PairState.PAIRED) {
                    AppView.this.runOnUiThread(() -> {
                        if (!inForeground || isFinishing() || isDestroyed()) {
                            return;
                        }
                        // Disable shortcuts referencing this PC for now
                        shortcutHelper.disableComputerShortcut(details,
                                getResources().getString(R.string.scut_not_paired));

                        // Display a toast to the user and quit the activity
                        Toast.makeText(AppView.this, getResources().getText(R.string.scut_not_paired), Toast.LENGTH_SHORT).show();
                        finish();
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
                    hostRunningGame = details.hasRunningGame();
                    updateUiWithServerinfo(details);
                    if (loadingApps && rawAppList != null) {
                        showAppListStatus(!details.hasPermission(ComputerDetails.PERMISSION_LIST)
                                ? R.string.applist_permission_required
                                : lastAppListEmpty ? R.string.library_no_apps : 0);
                    }

                    return;
                }

                hostRunningGame = details.hasRunningGame();
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

        ComputerManagerService.ApplistPoller created;
        synchronized (pollerLock) {
            if (!runningPolling || managerBinder == null) return;
            if (poller == null) {
                poller = managerBinder.createAppListPoller(computer);
            }
            created = poller;
        }
        if (created != null) created.start();
    }

    private void stopComputerUpdates() {
        runningPolling = false;
        mainHandler.removeCallbacks(refreshTimeout);
        ComputerManagerService.ApplistPoller current;
        synchronized (pollerLock) {
            current = poller;
        }
        if (current != null) {
            current.stop();
        }

        if (managerBinder != null) {
            managerBinder.stopPolling();
        }

        if (appGridAdapter != null) {
            appGridAdapter.cancelQueuedOperations();
        }
    }

    /** Its chrome follows the form: a rotation into another form lays it out again. */
    @Override
    protected boolean followsForm() {
        return true;
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
        viewChrome.bindSearch();
        viewChrome.setUpChrome();
        getOnBackPressedDispatcher().addCallback(this, new OnBackPressedCallback(true) {
            @Override public void handleOnBackPressed() { viewChrome.navigateBack(); }
        });

        // Allow floating expanded PiP overlays while browsing apps
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            setShouldDockBigOverlays(false);
        }

        UiHelper.notifyNewRootView(this);
        viewChrome.setUpAmbience();

        showHiddenApps = savedInstanceState != null ? savedInstanceState.getBoolean("showHiddenApps") :
                getIntent().getBooleanExtra(SHOW_HIDDEN_APPS_EXTRA, false);
        if (savedInstanceState != null) {
            searchQuery = savedInstanceState.getString("libraryQuery", "");
            controlsProfile = savedInstanceState.getString("controlsProfile");
            controlsGame = savedInstanceState.getString("controlsGame");
            viewChrome.restoreSearchState(savedInstanceState);
        }
        uuidString = getIntent().getStringExtra(UUID_EXTRA);
        if (uuidString == null || uuidString.isEmpty()) {
            finish();
            return;
        }

        hiddenAppIds.addAll(new HiddenStore.Preferences(this).of(uuidString));
        savedHiddenAppIds = new HashSet<>(hiddenAppIds);

        String computerName = getIntent().getStringExtra(NAME_EXTRA);

        setTitle(computerName);

        // Bind to the computer manager service
        managerServiceBound = bindService(new Intent(this, ComputerManagerService.class), serviceConnection,
                Service.BIND_AUTO_CREATE);
        if (!managerServiceBound) showAppListStatus(R.string.applist_load_failed);
    }

    void updateHiddenApps(boolean hideImmediately) {
        // Only what changed here: the list may have followed a renumber since this screen read it.
        HashSet<Integer> shown = new HashSet<>(savedHiddenAppIds);
        shown.removeAll(hiddenAppIds);
        HashSet<Integer> hidden = new HashSet<>(hiddenAppIds);
        hidden.removeAll(savedHiddenAppIds);
        Set<Integer> now = new HiddenStore.Preferences(this).changed(uuidString, shown, hidden);
        hiddenAppIds.clear();
        hiddenAppIds.addAll(now);
        savedHiddenAppIds = new HashSet<>(now);

        appGridAdapter.updateHiddenApps(hiddenAppIds, hideImmediately);
        updateLibraryState();
        // A hidden game leaves the PC's launcher exports too.
        FrontendSync.appListChanged(this, computer);
    }

    private void populateAppGridWithCache() {
        AppViewReconciler.populateAppGridWithCache(this);
    }

    // Keep navigation and any cached apps usable while the host responds.
    void loadAppsBlocking() {
        loadingApps = true;
        libraryStatus = 0;
        updateLibraryState();
    }

    void showAppListStatus(int message) {
        mainHandler.removeCallbacks(refreshTimeout);
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
                showAppListStatus(lastAppListEmpty ? R.string.library_no_apps : 0);
            }
        });
    }

    /**
     * The caption over the grid: a search's matches out of the games it looked through, or the count with hidden games
     * shown. Null when the grid simply shows every game that is not hidden, or shows nothing (the status panel speaks).
     */
    static String librarySummary(android.content.res.Resources res, String query, boolean showHidden, int shown, int searched) {
        if (shown == 0) return null;
        if (!query.trim().isEmpty()) return res.getQuantityString(R.plurals.library_results, searched, shown, searched);
        if (showHidden) return res.getQuantityString(R.plurals.library_showing_hidden, shown, shown);
        return null;
    }

    @Override
    public boolean dispatchTouchEvent(android.view.MotionEvent event) {
        viewChrome.onTouchEvent(event);
        return super.dispatchTouchEvent(event);
    }

    @Override
    public boolean dispatchGenericMotionEvent(android.view.MotionEvent event) {
        viewChrome.onGenericMotionEvent(event);
        return super.dispatchGenericMotionEvent(event);
    }

    @Override
    public boolean dispatchKeyEvent(android.view.KeyEvent event) {
        if (viewChrome.onKeyEvent(event)) return true;
        return super.dispatchKeyEvent(event);
    }

    void updateLibraryState() {
        viewChrome.updateLibraryState();
    }

    public void openLibrarySearch() {
        viewChrome.openLibrarySearch();
    }

    void refreshLibrary() {
        if (computer == null || computer.state == ComputerDetails.State.OFFLINE) {
            showAppListStatus(R.string.library_offline);
            return;
        }
        if (!computer.hasPermission(ComputerDetails.PERMISSION_LIST)) {
            showAppListStatus(R.string.applist_permission_required);
            return;
        }
        ComputerManagerService.ApplistPoller current;
        synchronized (pollerLock) {
            ComputerManagerService.ComputerManagerBinder binder = managerBinder;
            if (binder != null && runningPolling && (poller == null || poller.needsRestart())) {
                if (poller != null) poller.stop();
                poller = binder.createAppListPoller(computer);
                poller.start();
            }
            current = poller;
        }
        if (current == null || current.needsRestart()) {
            showAppListStatus(R.string.applist_load_failed);
            return;
        }
        loadAppsBlocking();
        mainHandler.removeCallbacks(refreshTimeout);
        mainHandler.postDelayed(refreshTimeout, REFRESH_TIMEOUT_MS);
        current.pollNow();
    }

    void setShowHiddenApps(boolean show) {
        showHiddenApps = show;
        if (appGridAdapter != null) appGridAdapter.setShowHiddenApps(show);
        updateLibraryState();
    }

    @Override protected void onSaveInstanceState(Bundle state) {
        super.onSaveInstanceState(state);
        state.putString("libraryQuery", searchQuery);
        state.putString("controlsProfile", controlsProfile);
        state.putString("controlsGame", controlsGame);
        state.putBoolean("showHiddenApps", showHiddenApps);
        viewChrome.saveSearchState(state);
    }

    @Override
    protected void onDestroy() {
        super.onDestroy();

        mainHandler.removeCallbacks(refreshTimeout);
        stopAndClearPoller();
        Dialog.closeDialogs(AppView.this);
        viewChrome.onDestroy();

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
        com.limelight.console.HostFeed.invalidateRecentStreams(managerBinder);
        if (appGridAdapter != null) appGridAdapter.notifyDataSetChanged();
        updateLibraryState();
    }

    @Override
    protected void onPause() {
        super.onPause();

        inForeground = false;
        viewMenu.dismiss();
        viewChrome.onPause();
        stopComputerUpdates();
    }

    /** The game the host reports as running, if the library lists it. */
    AppObject runningApp() {
        if (appGridAdapter == null) return null;
        for (AppObject app : appGridAdapter.getAllApps()) {
            if (app.isRunning) return app;
        }
        return null;
    }

    /** True when the host itself asks before it replaces the running game, so the client does not quit first. */
    boolean hostConfirmsReplacement() {
        return computer != null && computer.caps.has(HostCapabilities.Feature.HOST_CONFIRMS_REPLACEMENT);
    }

    void launchControlsPicker(String profile, String gameName) {
        controlsProfile = profile;
        controlsGame = gameName;
        try {
            controlsPicker.launch(new String[] {"*/*"});
        } catch (android.content.ActivityNotFoundException e) {
            controlsProfile = null;
            Toast.makeText(this, R.string.library_import_controls_error, Toast.LENGTH_LONG).show();
        }
    }

    private void importControls(List<android.net.Uri> files) {
        String profile = controlsProfile, game = controlsGame;
        controlsProfile = null;
        if (profile == null || files == null || files.isEmpty()) return;
        java.util.ArrayList<String> texts = new java.util.ArrayList<>();
        try {
            for (android.net.Uri file : files) {
                try (java.io.InputStream input = getContentResolver().openInputStream(file)) {
                    texts.add(com.limelight.binding.input.TouchLayout.readText(input));
                }
            }
        } catch (IOException | SecurityException e) {
            Toast.makeText(this, R.string.library_import_controls_error, Toast.LENGTH_LONG).show();
            return;
        }
        // Artemis stores pixels; read them against this screen held the way a stream holds it.
        android.util.DisplayMetrics screen = new android.util.DisplayMetrics();
        getWindowManager().getDefaultDisplay().getRealMetrics(screen);
        int width = Math.max(screen.widthPixels, screen.heightPixels), height = Math.min(screen.widthPixels, screen.heightPixels);
        com.limelight.binding.input.TouchLayoutImport.Result result;
        try {
            result = com.limelight.binding.input.TouchLayoutImport.importFiles(texts, width, height);
        } catch (IllegalArgumentException e) {
            UiHelper.showDialog(this, new com.limelight.ui.NavigationDialogBuilder(this)
                    .setTitle(getString(R.string.library_import_controls_title, game))
                    .setMessage(com.limelight.ui.GameTouchControls.errorMessage(this, e))
                    .setPositiveButton(android.R.string.ok, null).create());
            return;
        }
        UiHelper.showDialog(this, new com.limelight.ui.NavigationDialogBuilder(this).startOnCancel()
                .setTitle(getString(R.string.library_import_controls_title, game))
                .setMessage(com.limelight.ui.GameTouchControls.importSummary(this, result) + "\n\n" +
                        getString(R.string.library_import_controls_replace))
                .setPositiveButton(R.string.keyboard_shortcuts_replace_button, (dialog, which) -> {
                    result.layout.save(this, profile);
                    com.limelight.binding.input.TouchLayout.setEnabled(this, profile, true);
                    Toast.makeText(this, R.string.library_import_controls_saved, Toast.LENGTH_SHORT).show();
                })
                .setNegativeButton(android.R.string.cancel, null).create());
    }

    /**
     * Starts a game while another runs. A host that confirms replacement asks the player itself;
     * elsewhere the client quits the running game first, so it asks here.
     */
    void startInsteadOfRunning(AppObject app) {
        if (hostConfirmsReplacement() || !computer.hasPermission(ComputerDetails.PERMISSION_LAUNCH)) {
            ServerHelper.doStart(this, app.app, computer, managerBinder, LaunchSource.LIBRARY_REPLACE);
            return;
        }
        AppObject running = runningApp();
        UiHelper.showDialog(this, new com.limelight.ui.NavigationDialogBuilder(this).startOnCancel()
                .setMessage(running != null ?
                        getString(R.string.library_replace_message_named, running.app.getAppName(), app.app.getAppName()) :
                        getString(R.string.library_replace_message, app.app.getAppName()))
                .setPositiveButton(R.string.library_replace_confirm,
                        (dialog, which) -> ServerHelper.doStart(this, app.app, computer, managerBinder,
                                LaunchSource.LIBRARY_REPLACE))
                .setNegativeButton(android.R.string.cancel, null).create());
    }

    private void updateUiWithServerinfo(final ComputerDetails details) {
        AppViewReconciler.updateUiWithServerinfo(this, details);
    }

    void updateUiWithAppList(final List<NvApp> appList) {
        AppViewReconciler.updateUiWithAppList(this, appList);
    }

    @Override
    public int getAdapterFragmentLayoutId() {
        return PreferenceConfiguration.readPreferences(AppView.this).smallIconMode ?
                    R.layout.app_grid_view_small : R.layout.app_grid_view;
    }

    @Override
    public void receiveAbsListView(AbsListView listView) {
        // A recreated activity gets its grid fragment back before the service connects and the adapter exists;
        // the connection adds a fresh fragment once it does.
        if (appGridAdapter == null) return;
        viewChrome.applyGridPadding();
        if (GlassForm.of(this) == GlassForm.PHONE) {
            listView.setNextFocusUpId(R.id.library_back);
            listView.setAccessibilityTraversalAfter(R.id.library_back);
        }
        listView.setAdapter(appGridAdapter);
        viewChrome.bindAmbience(listView);
        if (appGridAdapter.getCount() > 0) viewChrome.playEnterOnce(listView);
        listView.setOnItemClickListener((arg0, arg1, pos, id) ->
                play((AppObject) appGridAdapter.getItem(pos), arg1));
        listView.setOnItemLongClickListener((parent, view, position, id) -> {
            viewMenu.showAppMenu((AppObject) appGridAdapter.getItem(position), view);
            return true;
        });
        listView.requestFocus();
    }

    /** A tap plays or resumes; another game running first needs the player's go-ahead. */
    void play(AppObject app, View from) {
        if (computer == null || managerBinder == null || computer.state == ComputerDetails.State.OFFLINE) {
            showAppListStatus(R.string.library_offline);
            return;
        }
        if (hostRunningGame && !app.isRunning && app.app.getControl() == NvApp.Control.NONE) {
            startInsteadOfRunning(app);
        } else {
            ServerHelper.doStart(this, app.app, computer, managerBinder, from, LaunchSource.LIBRARY_TAP);
        }
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
