package com.limelight;

import java.io.IOException;
import java.io.StringReader;
import java.util.Objects;

import com.limelight.binding.video.DecoderBenchmark;
import com.limelight.binding.crypto.AndroidCryptoProvider;
import com.limelight.computers.ComputerManagerListener;
import com.limelight.computers.HostActions;
import com.limelight.computers.ComputerManagerService;
import com.limelight.computers.HostMenu;
import com.limelight.grid.PcGridAdapter;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.http.NvHTTP;
import com.limelight.nvstream.http.PairingManager.PairState;
import com.limelight.preferences.AddComputerManually;
import com.limelight.preferences.GlPreferences;
import com.limelight.preferences.HostStreamSettings;
import com.limelight.preferences.PreferenceConfiguration;
import com.limelight.ui.AdapterFragment;
import com.limelight.ui.AdapterFragmentCallbacks;
import com.limelight.console.HomeSettings;
import com.limelight.ui.ShellBar;
import com.limelight.ui.TopAppBar;
import com.limelight.update.UpdateFlow;
import com.limelight.utils.CacheHelper;
import com.limelight.utils.Dialog;
import com.limelight.utils.HelpLauncher;
import com.limelight.utils.ServerHelper;
import com.limelight.utils.ShortcutHelper;
import com.limelight.utils.UiHelper;

import com.limelight.ui.NightjarActivity;
import com.limelight.ui.NavigationDialogBuilder;
import android.app.Service;
import android.content.ComponentName;
import android.content.Intent;
import android.content.ServiceConnection;
import android.content.res.Configuration;
import android.opengl.GLSurfaceView;
import android.os.Build;
import android.os.Bundle;
import android.os.IBinder;
import android.os.SystemClock;
import android.preference.PreferenceManager;
import com.limelight.ui.ActionSheet;
import android.view.View;
import android.widget.AbsListView;

import org.xmlpull.v1.XmlPullParserException;

import javax.microedition.khronos.egl.EGLConfig;
import javax.microedition.khronos.opengles.GL10;

public class PcView extends NightjarActivity implements AdapterFragmentCallbacks {
    private View noPcFoundLayout;
    private AbsListView pcList;
    private boolean listWasEmpty = true;
    private ActionSheet pcMenu;
    private PcGridAdapter pcGridAdapter;
    private ShortcutHelper shortcutHelper;
    private final HostActions hostActions = new HostActions(this, new HostActions.Callbacks() {
        @Override public ComputerManagerService.ComputerManagerBinder binder() { return managerBinder; }
        @Override public boolean isForeground() { return inForeground; }
        @Override public void onPaired(ComputerDetails computer) {
            if (HomeSettings.landsOnHome(PcView.this, computer)) {
                // Home is where its games are; the PCs screen steps aside unless it is the first screen.
                ShellBar.open(PcView.this, ShellBar.Destination.PCS, ShellBar.Destination.HOME);
            } else {
                doAppList(computer, true, false);
            }
        }
        @Override public void onRemoved(ComputerDetails computer) { removeFromList(computer); }
        @Override public void onRenamed(ComputerDetails computer) { pcGridAdapter.notifyDataSetChanged(); }
        @Override public void onWakeSent(ComputerDetails computer) {
            ComputerObject row = findComputer(computer.uuid);
            if (row != null) {
                // The row says "Waking…" until the PC answers or the wake window passes.
                row.wakeSentAt = SystemClock.elapsedRealtime();
                pcGridAdapter.notifyDataSetChanged();
            }
        }
        @Override public void pausePolling() { stopComputerUpdates(false); }
        @Override public void resumePolling() { startComputerUpdates(); }
    });
    private volatile ComputerManagerService.ComputerManagerBinder managerBinder;
    private boolean managerServiceBound;
    private Thread serviceWaitThread;
    private boolean freezeUpdates, runningPolling, inForeground, completeOnCreateCalled;
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

                    // Now make the binder visible
                    runOnUiThread(() -> {
                        if (!managerServiceBound || isFinishing() || isDestroyed()) {
                            return;
                        }
                        managerBinder = localBinder;
                        startComputerUpdates();
                    });

                    // Force a keypair to be generated early to avoid discovery delays
                    new AndroidCryptoProvider(getApplicationContext()).getClientCertificate();
                }
            };
            serviceWaitThread.start();
        }

        public void onServiceDisconnected(ComponentName className) {
            managerBinder = null;
            runningPolling = false;
        }
    };

    @Override
    public void onConfigurationChanged(Configuration newConfig) {
        super.onConfigurationChanged(newConfig);

        if (completeOnCreateCalled) {
            UiHelper.notifyNewRootView(this);
        }
    }

    private final static int PAIR_ID = HostMenu.PAIR;
    private final static int UNPAIR_ID = HostMenu.UNPAIR;
    private final static int WOL_ID = HostMenu.WAKE;
    private final static int DELETE_ID = HostMenu.REMOVE;
    private final static int RESUME_ID = HostMenu.RESUME;
    private final static int QUIT_ID = HostMenu.QUIT;
    private final static int LIBRARY_ID = HostMenu.LIBRARY;
    private final static int PC_SETTINGS_ID = HostMenu.PC_SETTINGS;
    private final static int OTP_PAIR_ID = HostMenu.OTP_PAIR;
    private final static int PAIRING_HELP_ID = HostMenu.PAIRING_HELP;
    private final static int RENAME_ID = HostMenu.RENAME;

    /** How long a row says "Waking…" after a wake signal while the PC has not answered. */
    static final long WAKE_WINDOW_MS = 2 * 60 * 1000;

    private void initializeViews() {
        setContentView(R.layout.activity_pc_view);

        UiHelper.notifyNewRootView(this);

        // Allow floating expanded PiP overlays while browsing PCs
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            setShouldDockBigOverlays(false);
        }

        // Set default preferences if we've never been run
        PreferenceManager.setDefaultValues(this, R.xml.preferences, false);

        TopAppBar.configure(this, R.id.home_toolbar, getString(R.string.library_computers), 0);
        ShellBar.install(this, ShellBar.Destination.PCS);
        ShellBar.installBack(this, ShellBar.Destination.PCS);
        findViewById(R.id.manuallyAddPc).setOnClickListener(v -> startActivity(new Intent(this, AddComputerManually.class)));

        getFragmentManager().beginTransaction()
            .replace(R.id.pcFragmentContainer, new AdapterFragment())
            .commitAllowingStateLoss();

        noPcFoundLayout = findViewById(R.id.no_pc_found_layout);
        findViewById(R.id.discovery_add).setOnClickListener(v -> startActivity(new Intent(this, AddComputerManually.class)));
        findViewById(R.id.discovery_help).setOnClickListener(v -> showPairingGuide());
        updateEmptyState();
        pcGridAdapter.notifyDataSetChanged();
    }

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);

        // Home is the launch screen once a host has shown its games; the list of PCs stays one tap away.
        if (savedInstanceState == null && opensHome()) {
            startActivity(new Intent(this, com.limelight.console.ConsoleHomeActivity.class));
            finish();
            return;
        }

        // Once per phone and app version: lets Automatic skip codecs that decode slower here.
        DecoderBenchmark.runIfNeeded(this);

        // Assume we're in the foreground when created to avoid a race
        // between binding to CMS and onResume()
        inForeground = true;

        // Create a GLSurfaceView to fetch GLRenderer unless we have
        // a cached result already.
        final GlPreferences glPrefs = GlPreferences.readPreferences(this);
        if (!glPrefs.savedFingerprint.equals(Build.FINGERPRINT) || glPrefs.glRenderer.isEmpty()) {
            GLSurfaceView surfaceView = new GLSurfaceView(this);
            surfaceView.setRenderer(new GLSurfaceView.Renderer() {
                @Override
                public void onSurfaceCreated(GL10 gl10, EGLConfig eglConfig) {
                    // Save the GLRenderer string so we don't need to do this next time
                    glPrefs.glRenderer = gl10.glGetString(GL10.GL_RENDERER);
                    glPrefs.savedFingerprint = Build.FINGERPRINT;
                    glPrefs.writePreferences();

                    LimeLog.info("Fetched GL Renderer: " + glPrefs.glRenderer);

                    runOnUiThread(new Runnable() {
                        @Override
                        public void run() {
                            completeOnCreate();
                        }
                    });
                }

                @Override
                public void onSurfaceChanged(GL10 gl10, int i, int i1) {
                }

                @Override
                public void onDrawFrame(GL10 gl10) {
                }
            });
            setContentView(surfaceView);
        }
        else {
            LimeLog.info("Cached GL Renderer: " + glPrefs.glRenderer);
            completeOnCreate();
        }
    }

    private boolean opensHome() {
        Intent intent = getIntent();
        if (intent == null || !Intent.ACTION_MAIN.equals(intent.getAction())
                || intent.getBooleanExtra(com.limelight.console.HomeSettings.EXTRA_STAY, false)) {
            return false;
        }
        GlPreferences glPrefs = GlPreferences.readPreferences(this);
        // Before the first GL renderer is read this screen has work to do; Home waits for the next start.
        return glPrefs.savedFingerprint.equals(Build.FINGERPRINT) && !glPrefs.glRenderer.isEmpty()
                && com.limelight.console.HomeSettings.opensOnHome(this);
    }

    private void completeOnCreate() {
        if (completeOnCreateCalled || isFinishing() || isDestroyed()) {
            return;
        }
        completeOnCreateCalled = true;

        shortcutHelper = new ShortcutHelper(this);

        UiHelper.setLocale(this);

        // Bind to the computer manager service
        managerServiceBound = bindService(new Intent(PcView.this, ComputerManagerService.class), serviceConnection,
                Service.BIND_AUTO_CREATE);

        pcGridAdapter = new PcGridAdapter(this, PreferenceConfiguration.readPreferences(this));

        initializeViews();

        // The first onResume can come before the GL renderer was read and this ran; look again now.
        if (inForeground) {
            checkForUpdates();
        }

        // The discovery screen explains setup inline; Help remains available from the toolbar.
    }

    private void showPairingGuide() {
        new NavigationDialogBuilder(this)
                .setTitle(R.string.guide_title)
                .setMessage(R.string.guide_text)
                .setPositiveButton(R.string.guide_done, null)
                .setNeutralButton(R.string.guide_troubleshooting,
                        (dialog, which) -> HelpLauncher.showHelp(PcView.this))
                .show();
    }

    private void startComputerUpdates() {
        // Only allow polling to start if we're bound to CMS, polling is not already running,
        // and our activity is in the foreground.
        if (managerBinder != null && !runningPolling && inForeground) {
            freezeUpdates = false;
            managerBinder.startPolling(new ComputerManagerListener() {
                @Override
                public void notifyComputerUpdated(final ComputerDetails details) {
                    if (!freezeUpdates) {
                        PcView.this.runOnUiThread(new Runnable() {
                            @Override
                            public void run() {
                                if (inForeground && !freezeUpdates && !isFinishing() && !isDestroyed()) {
                                    updateComputer(details);
                                }
                            }
                        });

                        // Add a launcher shortcut for this PC (off the main thread to prevent ANRs)
                        ComputerManagerService.ComputerManagerBinder binder = managerBinder;
                        if (binder != null && binder.isPersistent() && details.pairState == PairState.PAIRED) {
                            shortcutHelper.createAppViewShortcutForOnlineHost(details);
                        }
                        // Home needs a library to open on; read it once for a host that has the native API.
                        if (binder != null) {
                            com.limelight.console.ConsoleSeed.seedOnce(PcView.this, binder.getUniqueId(), details,
                                    () -> runOnUiThread(PcView.this::onLibrarySeeded));
                        }
                    }
                }
            });
            runningPolling = true;
        }
    }

    private void stopComputerUpdates(boolean wait) {
        if (managerBinder != null) {
            if (!runningPolling) {
                return;
            }

            freezeUpdates = true;

            managerBinder.stopPolling();

            if (wait) {
                managerBinder.waitForPollingStopped();
            }

            runningPolling = false;
        }
    }

    @Override
    public void onDestroy() {
        super.onDestroy();

        hostActions.destroy();
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
        if (completeOnCreateCalled) {
            // A host's library can arrive while this screen is away, which makes Home available.
            ShellBar.refresh(this);
        }
        if (!hostActions.deliverPendingResult() && !hostActions.isPairing()) {
            startComputerUpdates();
        }
        if (completeOnCreateCalled) {
            checkForUpdates();
        }
    }

    /** At most every 12 hours; the PC list is never in front during a stream. */
    protected void checkForUpdates() {
        UpdateFlow.autoCheck(this, () -> inForeground);
    }

    @Override
    protected void onPause() {
        super.onPause();

        inForeground = false;
        if (pcMenu != null) pcMenu.dismiss();
        // Pairing keeps running: people leave to type the PIN into the host's web page on this
        // phone, or the screen turns off on the way to the PC.
        stopComputerUpdates(false);
    }

    @Override
    protected void onStop() {
        super.onStop();

        Dialog.closeDialogs(PcView.this);
    }

    // The hint about X and the three dots shows once a key or button was the last input, not a touch.
    private boolean keysAreLastInput;

    @Override
    public boolean dispatchKeyEvent(android.view.KeyEvent event) {
        if (event.getAction() == android.view.KeyEvent.ACTION_DOWN) {
            setKeysAreLastInput(true);
        }
        return super.dispatchKeyEvent(event);
    }

    @Override
    public boolean dispatchTouchEvent(android.view.MotionEvent event) {
        if (event.getActionMasked() == android.view.MotionEvent.ACTION_DOWN) {
            setKeysAreLastInput(false);
        }
        return super.dispatchTouchEvent(event);
    }

    private void setKeysAreLastInput(boolean keys) {
        keysAreLastInput = keys;
        updateOptionsHint();
    }

    private void updateOptionsHint() {
        View hint = findViewById(R.id.pc_options_hint);
        if (hint != null && pcGridAdapter != null) {
            // Invisible, not gone, while a pointer is in use: the list must not resize on every input switch.
            hint.setVisibility(pcGridAdapter.getCount() == 0 ? View.GONE
                    : keysAreLastInput ? View.VISIBLE : View.INVISIBLE);
        }
    }

    /** The first library was just written: Home joins the shell bar, and with it becomes the root for Back. */
    void onLibrarySeeded() {
        if (isFinishing() || isDestroyed()) return;
        ShellBar.refresh(this);
    }

    private void updateEmptyState() {
        boolean empty = pcGridAdapter.getCount() == 0;
        updateOptionsHint();
        noPcFoundLayout.setVisibility(empty ? View.VISIBLE : View.GONE);
        findViewById(R.id.pcFragmentContainer).setVisibility(empty ? View.GONE : View.VISIBLE);
        findViewById(R.id.manuallyAddPc).setVisibility(empty ? View.GONE : View.VISIBLE);
        boolean filled = listWasEmpty && !empty;
        listWasEmpty = empty;
        if (filled && pcList != null && !pcList.isInTouchMode()) {
            // The first PC replaced the empty state, which held focus; give it to the list, not the shell bar.
            claimListFocus();
        }
    }

    private boolean focusIsLoose() {
        View focus = getCurrentFocus();
        return focus == null || isInside(focus, R.id.shell_bar) || isInside(focus, R.id.no_pc_found_layout);
    }

    private void claimListFocus() {
        if (!focusIsLoose() || pcList.requestFocus()) return;
        // The list was hidden until now, so it has not been laid out and cannot take focus yet.
        pcList.addOnLayoutChangeListener(new View.OnLayoutChangeListener() {
            @Override
            public void onLayoutChange(View v, int l, int t, int r, int b, int ol, int ot, int or, int ob) {
                v.removeOnLayoutChangeListener(this);
                if (!v.isInTouchMode() && focusIsLoose()) v.requestFocus();
            }
        });
    }

    private boolean isInside(View view, int containerId) {
        if (view.getId() == containerId) return true;
        android.view.ViewParent p = view.getParent();
        while (p != null) {
            if (p instanceof View && ((View) p).getId() == containerId) return true;
            p = p.getParent();
        }
        return false;
    }

    /**
     * The PC's menu, in this order: Open library, Resume and Quit, Pair, one-time PIN, Wake, PC settings,
     * Rename, Unpair or Pairing help, then Remove PC last. Touch reaches it by long press or the three
     * dots, a mouse by right-click, a D-pad or keyboard by Menu or Shift+F10, a controller by X.
     */
    void showComputerMenu(ComputerObject computer) {
        if (pcMenu != null) return;
        stopComputerUpdates(false);
        String game = computer.runningGame != null ? computer.runningGame.getAppName() : null;
        ActionSheet sheet = HostMenu.build(this, computer.details, computer.rowState(), game,
                action -> onComputerAction(computer, action));
        pcMenu = sheet;
        sheet.setOnDismissListener(dialog -> {
            pcMenu = null;
            startComputerUpdates();
        });
        sheet.show();
    }

    /** Sends the wake signal; tests record it instead of broadcasting a magic packet. */
    protected void doWakeOnLan(final ComputerDetails computer) {
        hostActions.wake(computer);
    }

    private ComputerObject findComputer(String uuid) {
        for (int i = 0; i < pcGridAdapter.getCount(); i++) {
            ComputerObject computer = (ComputerObject) pcGridAdapter.getItem(i);
            if (computer.details.uuid.equals(uuid)) {
                return computer;
            }
        }
        return null;
    }

    private void doAppList(ComputerDetails computer, boolean newlyPaired, boolean showHiddenGames) {
        if (computer.state == ComputerDetails.State.OFFLINE) {
            hostActions.showError(computer.displayName(), getString(R.string.error_pc_offline), null);
            return;
        }
        if (managerBinder == null) {
            hostActions.showError(computer.displayName(), getString(R.string.error_manager_not_running), null);
            return;
        }

        Intent i = new Intent(this, AppView.class);
        i.putExtra(AppView.NAME_EXTRA, computer.displayName());
        i.putExtra(AppView.UUID_EXTRA, computer.uuid);
        i.putExtra(AppView.NEW_PAIR_EXTRA, newlyPaired);
        i.putExtra(AppView.SHOW_HIDDEN_APPS_EXTRA, showHiddenGames);
        startActivity(i);
    }

    HostActions hostActions() {
        return hostActions;
    }

    private boolean onComputerAction(ComputerObject computer, int action) {
        switch (action) {
            case PAIR_ID:
                hostActions.pair(computer.details);
                return true;

            case OTP_PAIR_ID:
                hostActions.pairWithOneTimePin(computer.details);
                return true;

            case UNPAIR_ID:
                hostActions.confirmUnpair(computer.details);
                return true;

            case RENAME_ID:
                hostActions.rename(computer.details);
                return true;

            case WOL_ID:
                doWakeOnLan(computer.details);
                return true;

            case DELETE_ID:
                hostActions.confirmRemove(computer.details);
                return true;

            case LIBRARY_ID:
                doAppList(computer.details, false, false);
                return true;

            case RESUME_ID:
                if (managerBinder == null) {
                    hostActions.showError(computer.details.displayName(), getString(R.string.error_manager_not_running), null);
                    return true;
                }
                ServerHelper.doStart(this, HostMenu.runningApp(computer.runningGame, computer.details), computer.details, managerBinder);
                return true;

            case QUIT_ID:
                if (managerBinder == null) {
                    hostActions.showError(computer.details.displayName(), getString(R.string.error_manager_not_running), null);
                    return true;
                }
                UiHelper.displayQuitConfirmationDialog(this, () -> ServerHelper.doQuit(PcView.this, computer.details,
                        HostMenu.runningApp(computer.runningGame, computer.details), managerBinder, null), null);
                return true;

            case PC_SETTINGS_ID:
                HostStreamSettings.show(this, computer.details.uuid, computer.details.displayName());
                return true;

            case PAIRING_HELP_ID:
                showPairingGuide();
                return true;

            default:
                return false;
        }
    }

    /** A tap does the row's main action; a PC with nothing to do opens its menu. */
    private void onComputerTapped(ComputerObject computer) {
        PcGridAdapter.RowState state = computer.rowState();
        if (LaunchPolicy.resumesOnPcTap(PreferenceConfiguration.readPreferences(this).quickResume,
                state == PcGridAdapter.RowState.RUNNING)) {
            onComputerAction(computer, RESUME_ID);
            return;
        }
        switch (state) {
            case RUNNING:
            case READY:
                doAppList(computer.details, false, false);
                break;
            case NOT_PAIRED:
                hostActions.pair(computer.details);
                break;
            case WAKING:
            case OFFLINE:
                if (computer.details.macAddress != null) {
                    doWakeOnLan(computer.details);
                } else {
                    showComputerMenu(computer);
                }
                break;
            default:
                showComputerMenu(computer);
                break;
        }
    }

    /** The manager has removed the PC; take its row, its shortcuts and its alias off this screen. */
    private void removeFromList(ComputerDetails details) {
        for (int i = 0; i < pcGridAdapter.getCount(); i++) {
            ComputerObject computer = (ComputerObject) pcGridAdapter.getItem(i);

            if (details.uuid.equals(computer.details.uuid)) {
                // Disable or delete shortcuts referencing this PC
                shortcutHelper.disableComputerShortcut(details,
                        getResources().getString(R.string.scut_deleted_pc));

                pcGridAdapter.removeComputer(computer);
                pcGridAdapter.notifyDataSetChanged();

                if (pcGridAdapter.getCount() == 0) {
                    // Show the "Discovery in progress" view
                    updateEmptyState();
                }

                break;
            }
        }
    }
    
    private void updateComputer(ComputerDetails details) {
        ComputerObject existingEntry = null;

        for (int i = 0; i < pcGridAdapter.getCount(); i++) {
            ComputerObject computer = (ComputerObject) pcGridAdapter.getItem(i);

            // Check if this is the same computer
            if (details.uuid.equals(computer.details.uuid)) {
                existingEntry = computer;
                break;
            }
        }

        if (existingEntry != null) {
            // Replace the information in the existing entry
            existingEntry.details = details;
        }
        else {
            // Add a new entry
            existingEntry = new ComputerObject(details);
            pcGridAdapter.addComputer(existingEntry);

            // Remove the "Discovery in progress" view
            updateEmptyState();
        }
        if (details.state == ComputerDetails.State.ONLINE) {
            existingEntry.wakeSentAt = 0;
            com.limelight.update.HostUpdateChecker.check(this, details.caps, pcGridAdapter::notifyDataSetChanged);
        }
        findRunningGame(existingEntry);

        // Notify the view that the data has changed
        pcGridAdapter.notifyDataSetChanged();
    }

    /** Looks the running game up in the PC's app list, off the main thread, whenever it changes. */
    private void findRunningGame(ComputerObject computer) {
        ComputerDetails details = computer.details;
        String key = details.hasRunningGame() ? details.runningGameId + "/" + details.runningGameUuid : null;
        if (Objects.equals(key, computer.runningGameKey)) {
            return;
        }
        computer.runningGameKey = key;
        computer.runningGame = null;
        if (key == null) {
            return;
        }
        String uuid = details.uuid;
        String rawAppList = details.rawAppList;
        HostCapabilities caps = details.caps;
        ComputerDetails running = new ComputerDetails();
        running.runningGameId = details.runningGameId;
        running.runningGameUuid = details.runningGameUuid;
        new Thread(() -> {
            NvApp game = null;
            try {
                String list = rawAppList != null ? rawAppList : CacheHelper.readInputStreamToString(
                        CacheHelper.openCacheFileForInput(getCacheDir(), "applist", uuid));
                game = PcGridAdapter.runningApp(NvHTTP.getAppListByReader(new StringReader(list), caps), running);
            } catch (IOException | XmlPullParserException e) {
                // No app list yet: the row says a game is running without naming it.
            }
            final NvApp found = game;
            runOnUiThread(() -> {
                if (found != null && key.equals(computer.runningGameKey) && !isFinishing() && !isDestroyed()) {
                    computer.runningGame = found;
                    pcGridAdapter.notifyDataSetChanged();
                }
            });
        }, "Running game").start();
    }

    @Override
    public int getAdapterFragmentLayoutId() {
        return R.layout.pc_grid_view;
    }

    @Override
    public void receiveAbsListView(AbsListView listView) {
        pcList = listView;
        listView.setAdapter(pcGridAdapter);
        listView.setOnItemClickListener((parent, view, position, id) ->
                onComputerTapped((ComputerObject) pcGridAdapter.getItem(position)));
        listView.setOnItemLongClickListener((parent, view, position, id) -> {
            showComputerMenu((ComputerObject) pcGridAdapter.getItem(position));
            return true;
        });
        if (pcGridAdapter.getCount() == 0) findViewById(R.id.discovery_add).requestFocus();
        else if (!listView.isInTouchMode()) listView.requestFocus(); // content first, not the shell bar
    }

    public static class ComputerObject {
        public ComputerDetails details;
        /** The running game from the PC's app list, once it has been found there. */
        public NvApp runningGame;
        String runningGameKey;
        /** When this device last sent the PC a wake signal, in elapsed realtime; 0 for never. */
        public long wakeSentAt;

        public ComputerObject(ComputerDetails details) {
            if (details == null) {
                throw new IllegalArgumentException("details must not be null");
            }
            this.details = details;
        }

        public boolean isWaking() {
            return wakeSentAt != 0 && SystemClock.elapsedRealtime() - wakeSentAt < WAKE_WINDOW_MS;
        }

        public PcGridAdapter.RowState rowState() {
            return PcGridAdapter.rowState(details, isWaking());
        }

        @Override
        public String toString() {
            return details.displayName();
        }
    }
}
