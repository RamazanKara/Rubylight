package com.limelight;

import java.io.IOException;
import java.io.StringReader;
import java.util.HashMap;
import java.util.HashSet;
import java.util.List;
import java.util.Map;
import java.util.Set;

import com.limelight.computers.ComputerManagerListener;
import com.limelight.computers.ComputerManagerService;
import com.limelight.computers.HostCapabilityCache;
import com.limelight.grid.AppGridAdapter;
import com.limelight.grid.assets.CachedAppAssetLoader;
import com.limelight.nativeapi.HiddenStore;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.http.NvHTTP;
import com.limelight.nvstream.http.PairingManager;
import com.limelight.preferences.AppQuitOnLeave;
import com.limelight.preferences.AppStreamSettings;
import com.limelight.preferences.HostStreamSettings;
import com.limelight.preferences.PreferenceConfiguration;
import com.limelight.ui.AdapterFragment;
import com.limelight.ui.AdapterFragmentCallbacks;
import com.limelight.ui.AdaptiveGridView;
import com.limelight.ui.AppTheme;
import com.limelight.ui.BackdropView;
import com.limelight.ui.LibraryAmbience;
import com.limelight.console.HomeHints;
import com.limelight.console.PageChrome;
import com.limelight.ui.GlassForm;
import com.limelight.ui.GlassHost;
import com.limelight.ui.InputSource;
import com.limelight.ui.ShellBar;
import com.limelight.utils.CacheHelper;
import com.limelight.utils.Dialog;
import com.limelight.utils.FrontendSync;
import com.limelight.utils.LaunchSource;
import com.limelight.utils.ServerHelper;
import com.limelight.utils.ShortcutHelper;
import androidx.appcompat.widget.SearchView;
import com.google.android.material.snackbar.Snackbar;
import com.limelight.utils.UiHelper;

import com.limelight.ui.NightjarActivity;
import androidx.activity.OnBackPressedCallback;
import android.app.Service;
import android.content.ComponentName;
import android.content.Intent;
import android.content.ServiceConnection;
import android.content.SharedPreferences;
import android.content.res.Configuration;
import android.graphics.Bitmap;
import android.graphics.drawable.BitmapDrawable;
import android.graphics.drawable.ColorDrawable;
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

public class AppView extends NightjarActivity implements AdapterFragmentCallbacks {
    private AppGridAdapter appGridAdapter;
    private LibraryAmbience ambience;
    private boolean enterPlayed;
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
    // The host reports a running game, possibly by its UUID alone (Apollo's input-only mode).
    private boolean hostRunningGame;
    private boolean suspendGridUpdates;
    private volatile boolean inForeground;
    private boolean showHiddenApps;
    private PageChrome chrome;
    private InputSource hintSource;
    private int hintVendor;
    /** What the grid keeps free, as the chrome leaves it: start, end, foot. */
    private int gridStart, gridEnd, gridFoot;
    private int safeBottom, safeRight, safeLeft;
    private HashSet<Integer> hiddenAppIds = new HashSet<>();
    /** What {@link #hiddenAppIds} was when last read from or written to the list. */
    private HashSet<Integer> savedHiddenAppIds = new HashSet<>();

    private final static int START_OR_RESUME_ID = 1;
    private final static int QUIT_ID = 2;
    private final static int START_INSTEAD_ID = 4;
    private final static int VIEW_DETAILS_ID = 5;
    private final static int CREATE_SHORTCUT_ID = 6;
    private final static int HIDE_APP_ID = 7;
    private final static int OPEN_ON_START_ID = 8;

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
            // No binder (as under Robolectric) leaves nothing to wait for.
            if (!(binder instanceof ComputerManagerService.ComputerManagerBinder)) return;
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
                    hostRunningGame = details.hasRunningGame();
                    updateUiWithServerinfo(details);

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
        librarySearch = findViewById(R.id.library_search);
        setUpChrome();
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
        setUpAmbience();

        showHiddenApps = savedInstanceState != null ? savedInstanceState.getBoolean("showHiddenApps") :
                getIntent().getBooleanExtra(SHOW_HIDDEN_APPS_EXTRA, false);
        if (savedInstanceState != null) {
            searchQuery = savedInstanceState.getString("libraryQuery", "");
            controlsProfile = savedInstanceState.getString("controlsProfile");
            controlsGame = savedInstanceState.getString("controlsGame");
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
        savedHiddenAppIds = new HashSet<>(hiddenAppIds);

        String computerName = getIntent().getStringExtra(NAME_EXTRA);

        setTitle(computerName);

        // Bind to the computer manager service
        managerServiceBound = bindService(new Intent(this, ComputerManagerService.class), serviceConnection,
                Service.BIND_AUTO_CREATE);
        if (!managerServiceBound) showAppListStatus(R.string.applist_load_failed);
    }

    private void updateHiddenApps(boolean hideImmediately) {
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
                showAppListStatus(lastAppListEmpty ? R.string.library_no_apps : 0);
            }
        });
    }

    /** The glass and where everything goes on this form factor: the grid stands under the chrome and over the tabs. */
    private void setUpChrome() {
        GlassHost host = findViewById(R.id.glass_host);
        GlassForm form = GlassForm.of(this);
        View back = findViewById(R.id.library_back);
        View search = findViewById(R.id.library_search_button);
        View options = findViewById(R.id.library_options);
        back.setOnClickListener(v -> finish());
        search.setOnClickListener(v -> openLibrarySearch());
        options.setOnClickListener(v -> showLibraryOptions());
        ((TextView) findViewById(R.id.library_pc_name)).setText(getIntent().getStringExtra(NAME_EXTRA));
        showDot(findViewById(R.id.library_resume_dot), true);
        showPcState(true);
        findViewById(R.id.library_resume_button).setOnClickListener(v -> {
            AppObject running = runningApp();
            if (running != null) play(running, v);
        });
        android.widget.TextView title = findViewById(R.id.library_focus_title);
        android.widget.TextView line = findViewById(R.id.library_focus_status);
        if (form == GlassForm.PHONE) {
            // The capsule follows the back button, and one search sits beside the tabs, as on Home.
            findViewById(R.id.library_space_before).setVisibility(View.GONE);
            findViewById(R.id.library_space_after).setVisibility(View.VISIBLE);
            ((android.view.ViewGroup) search.getParent()).removeView(search);
            int size = getResources().getDimensionPixelSize(R.dimen.nj_size_tab_bar_height);
            android.widget.LinearLayout.LayoutParams place = new android.widget.LinearLayout.LayoutParams(size, size);
            place.setMarginStart(Math.round(10 * getResources().getDisplayMetrics().density));
            ((android.view.ViewGroup) findViewById(R.id.library_bottom_row)).addView(search, place);
            // The keys' line floats over the foot, for a phone with a pad in hand.
            ((android.view.ViewGroup) line.getParent()).removeView(line);
            host.addView(line, new android.widget.FrameLayout.LayoutParams(android.view.ViewGroup.LayoutParams.WRAP_CONTENT,
                    getResources().getDimensionPixelSize(R.dimen.library_hint_height), android.view.Gravity.BOTTOM | android.view.Gravity.CENTER_HORIZONTAL));
        } else {
            // The tabs lead the row and name the page; there is no way back that they do not offer.
            back.setVisibility(View.GONE);
            title.setVisibility(View.VISIBLE);
            line.setVisibility(View.VISIBLE);
            ((android.widget.LinearLayout.LayoutParams) title.getLayoutParams()).topMargin =
                    Math.round(getResources().getDisplayMetrics().density * (form == GlassForm.TELEVISION ? 16 : 10));
            if (android.os.Build.VERSION.SDK_INT >= Build.VERSION_CODES.N) title.forceHasOverlappingRendering(false);
            if (form == GlassForm.HANDHELD) {
                // Alone at the foot, it is as wide as what it says.
                android.widget.FrameLayout.LayoutParams place = (android.widget.FrameLayout.LayoutParams) findViewById(R.id.library_resume).getLayoutParams();
                place.gravity = android.view.Gravity.BOTTOM | android.view.Gravity.END;
                place.width = Math.round(getResources().getDisplayMetrics().density * 320);
            }
        }
        // Before the chrome, which styles the tabs: installing them again would undo that.
        ShellBar.install(this, ShellBar.Destination.PCS);
        // Last: the chrome reports the safe area at once, and that needs everything in its place.
        chrome = new PageChrome(this, host, R.id.library_tabs, R.id.library_top_row, R.id.library_bottom_row, R.id.library_foot_fade, this::layOutUnderChrome);
        if (form != GlassForm.PHONE) ShellBar.linkTopRow(this, R.id.library_options, R.id.library_search_button);
        // This page is inside PCs: its tab goes back to the list.
        findViewById(R.id.shell_pcs).setOnClickListener(v -> finish());
        findViewById(R.id.library_chrome).addOnLayoutChangeListener((v, l, t, r, b, ol, ot, or, ob) -> placeGrid(b));
        hintSource = InputSource.last() != null ? InputSource.last() : form == GlassForm.TELEVISION
                ? (com.limelight.ui.StreamMenuHint.hasGamepad() ? InputSource.PAD : InputSource.REMOTE) : InputSource.NONE;
        host.getViewTreeObserver().addOnTouchModeChangeListener(touch -> {
            if (touch || hintSource != InputSource.NONE) return;
            onInput(InputSource.leavingTouch(com.limelight.ui.StreamMenuHint.hasGamepad(),
                    getResources().getConfiguration().keyboard == android.content.res.Configuration.KEYBOARD_QWERTY), 0);
        });
    }

    private void showDot(View view, boolean ok) {
        android.graphics.drawable.GradientDrawable dot = new android.graphics.drawable.GradientDrawable();
        dot.setShape(android.graphics.drawable.GradientDrawable.OVAL);
        dot.setColor(ok ? com.google.android.material.color.MaterialColors.getColor(view, R.attr.glassOk)
                : androidx.core.content.ContextCompat.getColor(this, R.color.nj_color_label_secondary));
        view.setBackground(dot);
    }

    /** Whether the capsule's dot says the PC is there. */
    private void showPcState(boolean online) {
        showDot(findViewById(R.id.library_pc_dot), online);
    }

    /** Lays the content out edge to edge, inside what the chrome and the window leave. */
    private void layOutUnderChrome(int left, int top, int right, int bottom) {
        GlassForm form = GlassForm.of(this);
        android.content.res.Resources res = getResources();
        float density = res.getDisplayMetrics().density;
        safeLeft = left;
        safeRight = right;
        safeBottom = bottom;
        View row = findViewById(R.id.library_top_row);
        if (form == GlassForm.PHONE) {
            int edge = res.getDimensionPixelSize(R.dimen.glass_edge);
            row.setPaddingRelative(left + edge, top + Math.round(8 * density), right + edge, 0);
            ((android.view.ViewGroup.MarginLayoutParams) librarySearch.getLayoutParams()).setMarginStart(left + edge);
            ((android.view.ViewGroup.MarginLayoutParams) librarySearch.getLayoutParams()).setMarginEnd(right + edge);
            placeSummary(left + edge, right + edge);
            gridStart = left + edge;
            gridEnd = right + edge;
            findViewById(R.id.library_bottom_row).setPadding(left + res.getDimensionPixelSize(R.dimen.space_m), 0,
                    right + res.getDimensionPixelSize(R.dimen.space_m), bottom + res.getDimensionPixelSize(R.dimen.space_s));
            View fade = findViewById(R.id.library_foot_fade);
            android.view.ViewGroup.LayoutParams fadeSize = fade.getLayoutParams();
            fadeSize.height = res.getDimensionPixelSize(R.dimen.home_foot_fade) + bottom;
            fade.setLayoutParams(fadeSize);
        } else {
            boolean television = form == GlassForm.TELEVISION;
            int side = res.getDimensionPixelSize(R.dimen.shell_overscan_horizontal) + (television ? 0 : Math.round(24 * density));
            row.setPaddingRelative(left + side, television ? top : top + res.getDimensionPixelSize(R.dimen.space_m), right + side, 0);
            android.view.ViewGroup.MarginLayoutParams title = (android.view.ViewGroup.MarginLayoutParams) findViewById(R.id.library_focus_title).getLayoutParams();
            title.setMarginStart(left + side);
            title.setMarginEnd(right + side + Math.round(density * (television ? 300 : 200)));
            android.view.ViewGroup.MarginLayoutParams line = (android.view.ViewGroup.MarginLayoutParams) findViewById(R.id.library_focus_status).getLayoutParams();
            line.setMarginStart(left + side);
            line.setMarginEnd(right + side);
            ((android.view.ViewGroup.MarginLayoutParams) librarySearch.getLayoutParams()).setMarginStart(left + side);
            ((android.view.ViewGroup.MarginLayoutParams) librarySearch.getLayoutParams()).setMarginEnd(right + side);
            placeSummary(left + side, right + side);
            gridStart = left + side;
            gridEnd = right + side;
        }
        android.widget.FrameLayout.LayoutParams loading = (android.widget.FrameLayout.LayoutParams) findViewById(R.id.library_loading).getLayoutParams();
        loading.topMargin = top;
        placeFoot();
    }

    private void placeSummary(int start, int end) {
        android.view.ViewGroup.MarginLayoutParams at = (android.view.ViewGroup.MarginLayoutParams) findViewById(R.id.library_summary).getLayoutParams();
        at.setMarginStart(start);
        at.setMarginEnd(end);
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

    /** The capsule of the running game and the keys' line above the tabs, and what the grid keeps free under its last row. */
    private void placeFoot() {
        GlassForm form = GlassForm.of(this);
        android.content.res.Resources res = getResources();
        View resume = findViewById(R.id.library_resume);
        AppObject running = form == GlassForm.TELEVISION ? null : runningApp();
        int shown = running == null ? View.GONE : View.VISIBLE;
        if (resume.getVisibility() != shown) resume.setVisibility(shown);
        if (running != null) {
            TextView name = findViewById(R.id.library_resume_name);
            if (!running.app.getAppName().contentEquals(name.getText())) name.setText(running.app.getAppName());
        }
        int gap = res.getDimensionPixelSize(R.dimen.space_s);
        int side = res.getDimensionPixelSize(R.dimen.space_m);
        int capsule = running == null ? 0 : res.getDimensionPixelSize(R.dimen.library_resume_height) + gap;
        android.widget.FrameLayout.LayoutParams place = (android.widget.FrameLayout.LayoutParams) resume.getLayoutParams();
        int foot;
        if (form == GlassForm.PHONE) {
            int bar = safeBottom + gap + res.getDimensionPixelSize(R.dimen.nj_size_tab_bar_height);
            place.leftMargin = safeLeft + side;
            place.rightMargin = safeRight + side;
            place.bottomMargin = bar + gap;
            foot = bar + capsule + 2 * side;
            View line = findViewById(R.id.library_focus_status);
            android.widget.FrameLayout.LayoutParams at = (android.widget.FrameLayout.LayoutParams) line.getLayoutParams();
            at.bottomMargin = bar + capsule + gap;
            line.setLayoutParams(at);
        } else {
            place.rightMargin = gridEnd;
            place.bottomMargin = safeBottom + side;
            foot = safeBottom + (running == null ? side : capsule + 2 * side);
            // A TV's focused poster grows and wears its ring: the grid keeps room for that and the overscan under its last
            // row, so it can scroll the card that far clear of the screen's edge.
            if (form == GlassForm.TELEVISION) {
                foot = Math.max(foot, safeBottom + res.getDimensionPixelSize(R.dimen.shell_overscan_vertical)
                        + Math.round(24 * res.getDisplayMetrics().density));
            }
        }
        resume.setLayoutParams(place);
        gridFoot = foot;
        applyGridPadding();
    }

    private void applyGridPadding() {
        View grid = findViewById(R.id.fragmentView);
        if (grid == null) return;
        int top = getResources().getDimensionPixelSize(R.dimen.app_grid_top);
        if (grid.getPaddingStart() != gridStart || grid.getPaddingEnd() != gridEnd || grid.getPaddingTop() != top || grid.getPaddingBottom() != gridFoot) {
            grid.setPaddingRelative(gridStart, top, gridEnd, gridFoot);
        }
    }

    /** The grid starts where the chrome ends, which a search field opening moves. */
    private void placeGrid(int chromeBottom) {
        View frame = findViewById(R.id.appFragmentContainer);
        android.widget.FrameLayout.LayoutParams params = (android.widget.FrameLayout.LayoutParams) frame.getLayoutParams();
        if (params.topMargin == chromeBottom) return;
        params.topMargin = chromeBottom;
        // From a layout listener: on the next turn, or the request is lost in this pass.
        frame.post(() -> frame.setLayoutParams(params));
    }

    /** The last input names the keys on the line: on the next input, with no animation. */
    private void onInput(InputSource source, int vendor) {
        if (source == null) return;
        InputSource.remember(source);
        if (source == hintSource && vendor == hintVendor) return;
        hintSource = source;
        hintVendor = vendor;
        if (ambience != null) ambience.refresh();
    }

    @Override
    public boolean dispatchTouchEvent(android.view.MotionEvent event) {
        if (event.getActionMasked() == android.view.MotionEvent.ACTION_DOWN) onInput(InputSource.of(event), 0);
        return super.dispatchTouchEvent(event);
    }

    @Override
    public boolean dispatchGenericMotionEvent(android.view.MotionEvent event) {
        onInput(InputSource.of(event), event.getDevice() == null ? 0 : event.getDevice().getVendorId());
        return super.dispatchGenericMotionEvent(event);
    }

    @Override
    public boolean dispatchKeyEvent(android.view.KeyEvent event) {
        if (event.getAction() == android.view.KeyEvent.ACTION_DOWN) onInput(InputSource.of(event), InputSource.vendorOf(event));
        return super.dispatchKeyEvent(event);
    }

    /** What the game in focus is, then what the keys do on it. What OK does is what a tap does: the word promises nothing else. */
    private CharSequence focusLine(AppObject app) {
        int state = AppGridAdapter.stateWord(app);
        CharSequence keys = HomeHints.line(this, hintSource, hintVendor,
                getString(app.isRunning ? R.string.applist_menu_resume : R.string.library_play), getString(R.string.console_hint_options));
        android.text.SpannableStringBuilder line = new android.text.SpannableStringBuilder();
        if (state != 0 && GlassForm.of(this) != GlassForm.PHONE) {
            line.append(getString(state));
            line.setSpan(new android.text.style.ForegroundColorSpan(com.google.android.material.color.MaterialColors.getColor(findViewById(R.id.library_focus_status), com.google.android.material.R.attr.colorOnSurfaceVariant)),
                    0, line.length(), android.text.Spanned.SPAN_EXCLUSIVE_EXCLUSIVE);
        }
        if (keys != null) {
            if (line.length() > 0) line.append("    ");
            line.append(keys);
        }
        return line;
    }

    /** The ambient of the game in focus behind everything; on a handheld and a TV its title and keys over the grid. */
    private void setUpAmbience() {
        BackdropView backdrop = findViewById(R.id.library_backdrop);
        GlassForm form = GlassForm.of(this);
        int canvas = form == GlassForm.TELEVISION ? AppTheme.tvCanvas(this)
                : com.google.android.material.color.MaterialColors.getColor(backdrop, R.attr.glassCanvas);
        chrome.setUpBackdrop(backdrop, canvas);
        // The window shows the canvas until the first backdrop covers it, then stops painting under it.
        getWindow().setBackgroundDrawable(new ColorDrawable(canvas));
        // The TV's safe-area padding sits on the content view, which would otherwise clip the backdrop that reaches past it.
        ((android.view.ViewGroup) findViewById(android.R.id.content)).setClipChildren(false);
        backdrop.setCanvasColor(canvas);
        backdrop.setDim(AppTheme.isHighContrast(this) ? 0.85f : 1f);
        backdrop.setOnCovered(() -> getWindow().setBackgroundDrawable(null));
        ambience = new LibraryAmbience(backdrop, form == GlassForm.PHONE ? null : findViewById(R.id.library_focus_title), findViewById(R.id.library_focus_status));
        ambience.setLine(this::focusLine);
    }

    /** The first time the grid holds games: a PC's cached list reaches the adapter before the grid exists, a fresh one after. */
    private void playEnterOnce(View grid) {
        if (enterPlayed || !(grid instanceof AdaptiveGridView)) return;
        enterPlayed = true;
        ((AdaptiveGridView) grid).playEnter();
    }

    private void updateLibraryState() {
        if (librarySearch == null) return;
        int visible = appGridAdapter == null ? 0 : appGridAdapter.getCount();
        if (ambience != null) {
            if (visible == 0) {
                ambience.clear();
            } else {
                ambience.refresh();
                playEnterOnce(findViewById(R.id.fragmentView));
            }
        }
        placeFoot();
        findViewById(R.id.library_loading).setVisibility(loadingApps ? View.VISIBLE : View.GONE);
        String summary = appGridAdapter == null ? null
                : librarySummary(getResources(), searchQuery, showHiddenApps, visible, appGridAdapter.getLibraryCount());
        TextView summaryLine = findViewById(R.id.library_summary);
        if (summary == null) {
            summaryLine.setVisibility(View.GONE);
        } else {
            if (!summary.contentEquals(summaryLine.getText())) summaryLine.setText(summary);
            summaryLine.setVisibility(View.VISIBLE);
        }

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
        if (message == R.string.library_no_apps) action = R.string.library_refresh;
        TextView status = findViewById(R.id.app_list_status);
        status.setVisibility(message == 0 ? View.GONE : View.VISIBLE);
        if (message != 0) status.setText(message);
        findViewById(R.id.library_status_panel).setVisibility(message == 0 ? View.GONE : View.VISIBLE);
        TextView retryButton = findViewById(R.id.library_retry);
        retryButton.setText(action);
        retryButton.setOnClickListener(retry);
        boolean offered = message != 0 && !loadingApps;
        retryButton.setVisibility(offered ? View.VISIBLE : View.GONE);
        // An empty grid has nothing for a remote to land on: what can be done about it takes the focus.
        View focused = getCurrentFocus();
        if (offered && visible == 0 && !retryButton.isInTouchMode() && (focused == null || focused.getId() == R.id.fragmentView)) {
            retryButton.requestFocus();
        }
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
        sheet.addAction(R.drawable.ic_refresh, getString(R.string.library_refresh)).setOnClickListener(v -> {
            sheet.dismiss();
            refreshLibrary();
        });
        sheet.addAction(showHiddenApps ? R.drawable.ic_visibility_off : R.drawable.ic_visibility,
                getString(showHiddenApps ? R.string.library_hide_hidden : R.string.library_show_hidden)).setOnClickListener(v -> {
            sheet.dismiss();
            setShowHiddenApps(!showHiddenApps);
        });
        sheet.setOnDismissListener(dialog -> libraryMenu = null);
        sheet.show();
    }

    @Override protected void onSaveInstanceState(Bundle state) {
        super.onSaveInstanceState(state);
        state.putString("libraryQuery", searchQuery);
        state.putString("controlsProfile", controlsProfile);
        state.putString("controlsGame", controlsGame);
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
        if (chrome != null) chrome.stop();

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

    /** The game the host reports as running, if the library lists it. */
    private AppObject runningApp() {
        if (appGridAdapter == null) return null;
        for (AppObject app : appGridAdapter.getAllApps()) {
            if (app.isRunning) return app;
        }
        return null;
    }

    /** True when the host itself asks before it replaces the running game, so the client does not quit first. */
    private boolean hostConfirmsReplacement() {
        return computer != null && computer.caps.has(HostCapabilities.Feature.HOST_CONFIRMS_REPLACEMENT);
    }

    private void showAppMenu(AppObject selectedApp, View selectedView) {
        if (appMenu != null) return;
        ActionSheet sheet = new ActionSheet(this, selectedApp.app.getAppName());
        appMenu = sheet;
        suspendGridUpdates = true;
        boolean canLaunch = computer.hasPermission(ComputerDetails.PERMISSION_LAUNCH);
        if (selectedApp.app.getControl() != NvApp.Control.NONE) {
            addAppAction(sheet, selectedApp, selectedView, START_OR_RESUME_ID,
                    R.drawable.ic_play, selectedApp.app.getAppName().trim());
        } else if (selectedApp.isRunning) {
            addAppAction(sheet, selectedApp, selectedView, START_OR_RESUME_ID,
                    R.drawable.ic_play, getString(R.string.applist_menu_resume));
            if (canLaunch) {
                addAppAction(sheet, selectedApp, selectedView, QUIT_ID,
                        R.drawable.ic_stop, getString(R.string.applist_menu_quit));
            }
        } else if (hostRunningGame && canLaunch) {
            AppObject running = runningApp();
            if (hostConfirmsReplacement()) {
                // The host asks before it closes the running game; nothing is quit from here.
                CharSequence note = running != null ? getString(R.string.library_host_asks_named, running.app.getAppName()) :
                        getString(R.string.library_host_asks);
                sheet.addAction(R.drawable.ic_play, getString(R.string.library_play), note).setOnClickListener(v -> {
                    sheet.dismiss();
                    onAppAction(selectedApp, selectedView, START_OR_RESUME_ID);
                });
            } else {
                addAppAction(sheet, selectedApp, selectedView, START_INSTEAD_ID, R.drawable.ic_play, running != null ?
                        getString(R.string.library_quit_named_and_play, running.app.getAppName()) :
                        getString(R.string.library_quit_and_play));
            }
        } else if (canLaunch) {
            addAppAction(sheet, selectedApp, selectedView, START_OR_RESUME_ID,
                    R.drawable.ic_play, getString(R.string.library_play));
        }
        addSmallStreamInputOnlyAction(sheet, selectedApp);
        if (LaunchPolicy.offersQuitOnLeave(canLaunch, selectedApp.app.getControl(), selectedApp.app.getRole())) {
            addQuitOnLeaveAction(sheet, selectedApp);
        }
        if (selectedApp.app.getControl() == NvApp.Control.NONE) {
            addGameSettingsAction(sheet, selectedApp);
        }
        if (selectedApp.app.getRole() != NvApp.Role.REMOTE_MONITOR) {
            sheet.addAction(R.drawable.ic_touch_controls, getString(R.string.library_import_controls),
                    getString(R.string.library_import_controls_note)).setOnClickListener(v -> {
                sheet.dismiss();
                controlsProfile = com.limelight.binding.input.TouchLayout.profileKey(uuidString,
                        selectedApp.app.getAppUuid(), selectedApp.app.getAppId());
                controlsGame = selectedApp.app.getAppName();
                try {
                    controlsPicker.launch(new String[] {"*/*"});
                } catch (android.content.ActivityNotFoundException e) {
                    controlsProfile = null;
                    Toast.makeText(this, R.string.library_import_controls_error, Toast.LENGTH_LONG).show();
                }
            });
        }
        if (!selectedApp.isRunning || selectedApp.isHidden) {
            addAppAction(sheet, selectedApp, selectedView, HIDE_APP_ID,
                    selectedApp.isHidden ? R.drawable.ic_visibility : R.drawable.ic_visibility_off,
                    getString(selectedApp.isHidden ? R.string.applist_menu_show_app : R.string.applist_menu_hide_app));
        }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            addAppAction(sheet, selectedApp, selectedView, CREATE_SHORTCUT_ID,
                    R.drawable.ic_add, getString(R.string.applist_menu_scut));
        }
        addAppAction(sheet, selectedApp, selectedView, OPEN_ON_START_ID, R.drawable.ic_play,
                com.limelight.console.OpenPlay.menuLabel(this, computer.uuid, selectedApp.app.getAppId()));
        addAppAction(sheet, selectedApp, selectedView, VIEW_DETAILS_ID,
                R.drawable.ic_info, getString(R.string.applist_menu_details));
        sheet.setOnDismissListener(dialog -> {
            appMenu = null;
            suspendGridUpdates = false;
            if (poller != null && inForeground) poller.pollNow();
        });
        sheet.show();
    }

    // A Nightjar layout, or an Artemis keyboard layout and/or Custom Keys file, for the game the menu was opened on.
    // Kept across recreation while the picker is open.
    private String controlsProfile, controlsGame;
    private final androidx.activity.result.ActivityResultLauncher<String[]> controlsPicker = registerForActivityResult(
            new androidx.activity.result.contract.ActivityResultContracts.OpenMultipleDocuments(), this::importControls);

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
            UiHelper.showDialog(this, new com.google.android.material.dialog.MaterialAlertDialogBuilder(this)
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
     * Input-only on a PC whose host has no Input-only mode of its own, when the setting is on: a
     * small stream this device does not show. It joins the running game or starts one on an idle
     * PC, and never replaces a game that runs.
     */
    private void addSmallStreamInputOnlyAction(ActionSheet sheet, AppObject app) {
        if (app.app.getControl() != NvApp.Control.NONE ||
                !InputOnlyPolicy.keepsRunningGame(hostRunningGame, app.isRunning)) {
            return;
        }
        boolean hostListsInputOnly = false;
        for (AppObject listed : appGridAdapter.getAllApps()) {
            if (listed.app.getRole() == NvApp.Role.INPUT_ONLY) {
                hostListsInputOnly = true;
                break;
            }
        }
        if (InputOnlyPolicy.mode(PreferenceConfiguration.readPreferences(this).inputOnlyAnyHost, computer.caps,
                hostListsInputOnly) != InputOnlyPolicy.Mode.SMALL_STREAM) {
            return;
        }
        int blocker = InputOnlyPolicy.smallStreamBlocker(computer);
        com.google.android.material.button.MaterialButton action = sheet.addAction(R.drawable.ic_input_only,
                getString(R.string.input_only_start), blocker != 0 ? getString(blocker) :
                        getString(R.string.input_only_small_stream_note, InputOnlyPolicy.estimatedKbps() / 1000f,
                                InputOnlyPolicy.estimatedMegabytesPerHour()));
        action.setEnabled(blocker == 0);
        action.setOnClickListener(v -> {
            sheet.dismiss();
            if (managerBinder == null || computer.state == ComputerDetails.State.OFFLINE) {
                showAppListStatus(R.string.library_offline);
                return;
            }
            ServerHelper.doStart(this, app.app, computer, managerBinder, true, LaunchSource.LIBRARY_INPUT_ONLY);
        });
    }

    /** Turns quit on leave on or off for this game in place; the sheet stays open and says what leaving now does. */
    private void addQuitOnLeaveAction(ActionSheet sheet, AppObject app) {
        String appUuid = app.app.getAppUuid();
        int appId = app.app.getAppId();
        com.google.android.material.button.MaterialButton action = sheet.addAction(R.drawable.ic_power,
                getString(R.string.library_quit_on_leave),
                quitOnLeaveNote(AppQuitOnLeave.isEnabled(this, uuidString, appUuid, appId)));
        action.setOnClickListener(v -> {
            boolean enabled = !AppQuitOnLeave.isEnabled(this, uuidString, appUuid, appId);
            AppQuitOnLeave.setEnabled(this, uuidString, appUuid, appId, enabled);
            sheet.setActionText(action, getString(R.string.library_quit_on_leave), quitOnLeaveNote(enabled));
            action.announceForAccessibility(quitOnLeaveNote(enabled));
        });
    }

    /** Opens this game's own stream values; the note says whether it has any. */
    private void addGameSettingsAction(ActionSheet sheet, AppObject app) {
        String appUuid = app.app.getAppUuid();
        int appId = app.app.getAppId();
        int own = AppStreamSettings.load(this, uuidString, appUuid, appId).count();
        sheet.addAction(R.drawable.ic_tune, getString(R.string.game_settings_action), own == 0 ?
                getString(R.string.game_settings_note_none) :
                getResources().getQuantityString(R.plurals.game_settings_note_own, own, own)).setOnClickListener(v -> {
            sheet.dismiss();
            HostStreamSettings.showGame(this, uuidString, getIntent().getStringExtra(NAME_EXTRA), appUuid, appId,
                    app.app.getAppName());
        });
    }

    private String quitOnLeaveNote(boolean enabled) {
        return getString(enabled ? R.string.library_quit_on_leave_on : R.string.library_quit_on_leave_off);
    }

    private void addAppAction(ActionSheet sheet, AppObject app, View view, int action, int icon, String label) {
        sheet.addAction(icon, label).setOnClickListener(v -> {
            sheet.dismiss();
            onAppAction(app, view, action);
        });
    }

    /**
     * Starts a game while another runs. A host that confirms replacement asks the player itself;
     * elsewhere the client quits the running game first, so it asks here.
     */
    private void startInsteadOfRunning(AppObject app) {
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

    private boolean onAppAction(AppObject app, View view, int action) {
        if ((action == START_OR_RESUME_ID || action == START_INSTEAD_ID || action == QUIT_ID) &&
                (computer == null || managerBinder == null || computer.state == ComputerDetails.State.OFFLINE)) {
            showAppListStatus(R.string.library_offline);
            return true;
        }
        switch (action) {
            case START_INSTEAD_ID:
                startInsteadOfRunning(app);
                return true;

            case START_OR_RESUME_ID:
                // Resume is the same as start for us
                ServerHelper.doStart(AppView.this, app.app, computer, managerBinder, LaunchSource.LIBRARY_MENU);
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

            case OPEN_ON_START_ID:
                com.limelight.console.OpenPlay.toggle(this, computer.uuid, computer.displayName(), app.app.getAppId(), app.app.getAppUuid(), app.app.getAppName());
                return true;

            case CREATE_SHORTCUT_ID:
                // The game's artwork when it has some; the PC icon otherwise.
                ImageView appImageView = view == null ? null : view.findViewById(R.id.grid_image);
                Bitmap appBits = appImageView != null && CachedAppAssetLoader.hasArtwork(appImageView) ?
                        ((BitmapDrawable) appImageView.getDrawable()).getBitmap() : null;
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
                    updateLibraryState();
                }
                showPcState(details.state != ComputerDetails.State.OFFLINE);
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
                // The first saved list of a PC without the native API makes Home available.
                ShellBar.refresh(AppView.this);
                showAppListStatus(!computer.hasPermission(ComputerDetails.PERMISSION_LIST) ?
                        R.string.applist_permission_required : appList.isEmpty() ? R.string.library_no_apps : 0);
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
        // A recreated activity gets its grid fragment back before the service connects and the adapter exists;
        // the connection adds a fresh fragment once it does.
        if (appGridAdapter == null) return;
        applyGridPadding();
        if (GlassForm.of(this) == GlassForm.PHONE) {
            listView.setNextFocusUpId(R.id.library_back);
            if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.LOLLIPOP_MR1) {
                listView.setAccessibilityTraversalAfter(R.id.library_back);
            }
        }
        listView.setAdapter(appGridAdapter);
        if (ambience != null) ambience.bind(listView, appGridAdapter);
        if (appGridAdapter.getCount() > 0) playEnterOnce(listView);
        listView.setOnItemClickListener(new OnItemClickListener() {
            @Override
            public void onItemClick(AdapterView<?> arg0, View arg1, int pos,
                                    long id) {
                AppObject app = (AppObject) appGridAdapter.getItem(pos);

                play(app, arg1);
            }
        });
        listView.setOnItemLongClickListener((parent, view, position, id) -> {
            showAppMenu((AppObject) appGridAdapter.getItem(position), view);
            return true;
        });
        listView.requestFocus();
    }

    /** A tap plays or resumes; another game running first needs the player's go-ahead. */
    private void play(AppObject app, View from) {
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
