package com.limelight;

import java.io.IOException;
import java.io.StringReader;
import java.util.HashMap;
import java.util.HashSet;
import java.util.List;
import java.util.Map;

import com.limelight.computers.ComputerManagerListener;
import com.limelight.computers.ComputerManagerService;
import com.limelight.grid.AppGridAdapter;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.http.NvHTTP;
import com.limelight.nvstream.http.PairingManager;
import com.limelight.preferences.PreferenceConfiguration;
import com.limelight.ui.AdapterFragment;
import com.limelight.ui.AdapterFragmentCallbacks;
import com.limelight.utils.CacheHelper;
import com.limelight.utils.Dialog;
import com.limelight.utils.ServerHelper;
import com.limelight.utils.ShortcutHelper;
import com.limelight.utils.SpinnerDialog;
import com.limelight.utils.UiHelper;

import androidx.appcompat.app.AppCompatActivity;
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
    private SpinnerDialog blockingLoadSpinner;
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
        if (managerBinder == null || !inForeground) {
            return;
        }

        managerBinder.startPolling(new ComputerManagerListener() {
            @Override
            public void notifyAppListUpdated(ComputerDetails details, boolean success) {
                if (!details.uuid.equalsIgnoreCase(uuidString)) return;
                if (success) {
                    notifyComputerUpdated(details);
                    runOnUiThread(() -> showAppListStatus(!details.hasPermission(ComputerDetails.PERMISSION_LIST) ?
                            R.string.applist_permission_required : lastAppListEmpty ? R.string.applist_empty : 0));
                } else {
                    showAppListStatus(details.hasPermission(ComputerDetails.PERMISSION_LIST) ?
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
                    // The PC is unreachable now
                    AppView.this.runOnUiThread(new Runnable() {
                        @Override
                        public void run() {
                            if (!inForeground || isFinishing() || isDestroyed()) {
                                return;
                            }
                            // Display a toast to the user and quit the activity
                            Toast.makeText(AppView.this, getResources().getText(R.string.lost_connection), Toast.LENGTH_SHORT).show();
                            finish();
                        }
                    });

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

                // App list is the same or empty
                if (rawAppList == null || rawAppList.equals(lastRawApplist)) {

                    // Let's check if the running app ID changed
                    if (details.runningGameId != lastRunningAppId) {
                        // Update the currently running game using the app ID
                        lastRunningAppId = details.runningGameId;
                        updateUiWithServerinfo(details);
                    }

                    return;
                }

                lastRunningAppId = details.runningGameId;
                lastRawApplist = rawAppList;

                try {
                    updateUiWithAppList(NvHTTP.getAppListByReader(new StringReader(rawAppList)));
                    updateUiWithServerinfo(details);

                    if (blockingLoadSpinner != null) {
                        blockingLoadSpinner.dismiss();
                        blockingLoadSpinner = null;
                    }
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
        findViewById(R.id.library_back).setOnClickListener(v -> finish());

        // Allow floating expanded PiP overlays while browsing apps
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            setShouldDockBigOverlays(false);
        }

        UiHelper.notifyNewRootView(this);

        showHiddenApps = getIntent().getBooleanExtra(SHOW_HIDDEN_APPS_EXTRA, false);
        uuidString = getIntent().getStringExtra(UUID_EXTRA);
        if (uuidString == null || uuidString.isEmpty()) {
            finish();
            return;
        }

        SharedPreferences hiddenAppsPrefs = getSharedPreferences(HIDDEN_APPS_PREF_FILENAME, MODE_PRIVATE);
        for (String hiddenAppIdStr : hiddenAppsPrefs.getStringSet(uuidString, new HashSet<String>())) {
            hiddenAppIds.add(Integer.parseInt(hiddenAppIdStr));
        }

        String computerName = getIntent().getStringExtra(NAME_EXTRA);

        TextView label = findViewById(R.id.appListText);
        setTitle(computerName);
        label.setText(computerName);

        // Bind to the computer manager service
        managerServiceBound = bindService(new Intent(this, ComputerManagerService.class), serviceConnection,
                Service.BIND_AUTO_CREATE);
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
    }

    private void populateAppGridWithCache() {
        try {
            // Try to load from cache
            lastRawApplist = CacheHelper.readInputStreamToString(CacheHelper.openCacheFileForInput(getCacheDir(), "applist", uuidString));
            List<NvApp> applist = NvHTTP.getAppListByReader(new StringReader(lastRawApplist));
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

    private void loadAppsBlocking() {
        blockingLoadSpinner = SpinnerDialog.displayDialog(this, getResources().getString(R.string.applist_refresh_title),
                getResources().getString(R.string.applist_refresh_msg), true);
    }

    private void showAppListStatus(int message) {
        runOnUiThread(() -> {
            if (!inForeground || isFinishing() || isDestroyed()) return;
            if (blockingLoadSpinner != null) {
                blockingLoadSpinner.dismiss();
                blockingLoadSpinner = null;
            }
            TextView status = findViewById(R.id.app_list_status);
            if (message != 0) status.setText(message);
            status.setVisibility(message == 0 ? View.GONE : View.VISIBLE);
        });
    }

    @Override
    protected void onDestroy() {
        super.onDestroy();

        SpinnerDialog.closeDialogs(this);
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
    }

    @Override
    protected void onPause() {
        super.onPause();

        inForeground = false;
        if (appMenu != null) appMenu.dismiss();
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
        if (!selectedApp.isRunning || selectedApp.isHidden) {
            addAppAction(sheet, selectedApp, selectedView, HIDE_APP_ID, R.drawable.ic_library,
                    getString(selectedApp.isHidden ? R.string.applist_menu_show_app : R.string.applist_menu_hide_app));
        }
        addAppAction(sheet, selectedApp, selectedView, VIEW_DETAILS_ID,
                R.drawable.ic_help, getString(R.string.applist_menu_details));
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
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
                updateHiddenApps(false);
                return true;

            case CREATE_SHORTCUT_ID:
                ImageView appImageView = view.findViewById(R.id.grid_image);
                Bitmap appBits = ((BitmapDrawable)appImageView.getDrawable()).getBitmap();
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
                boolean updated = false;

                    // Look through our current app list to tag the running app
                for (int i = 0; i < appGridAdapter.getCount(); i++) {
                    AppObject existingApp = (AppObject) appGridAdapter.getItem(i);

                    // There can only be one or zero apps running.
                    if (existingApp.isRunning &&
                            existingApp.app.matchesRunningApp(details.runningGameId, details.runningGameUuid)) {
                        // This app was running and still is, so we're done now
                        return;
                    }
                    else if (existingApp.app.matchesRunningApp(details.runningGameId, details.runningGameUuid)) {
                        // This app wasn't running but now is
                        existingApp.isRunning = true;
                        updated = true;
                    }
                    else if (existingApp.isRunning) {
                        // This app was running but now isn't
                        existingApp.isRunning = false;
                        updated = true;
                    }
                    else {
                        // This app wasn't running and still isn't
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
