package com.limelight;

import java.io.IOException;
import java.io.StringReader;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;
import java.util.Objects;

import com.limelight.binding.video.DecoderBenchmark;
import com.limelight.binding.crypto.AndroidCryptoProvider;
import com.limelight.computers.ComputerManagerListener;
import com.limelight.computers.HostActions;
import com.limelight.computers.ComputerManagerService;
import com.limelight.computers.HostMenu;
import com.limelight.files.FileTransferActivity;
import com.limelight.grid.PcGridAdapter;
import com.limelight.nativeapi.NativePcPower;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.http.NvHTTP;
import com.limelight.nvstream.http.PairingManager.PairState;
import com.limelight.nvstream.wol.WakeTargets;
import com.limelight.preferences.AddComputerManually;
import com.limelight.preferences.GlPreferences;
import com.limelight.preferences.HostStreamSettings;
import com.limelight.preferences.PreferenceConfiguration;
import com.limelight.ui.AdapterFragment;
import com.limelight.ui.AdapterFragmentCallbacks;
import com.limelight.console.HomeSettings;
import com.limelight.ui.GlassForm;
import com.limelight.ui.InputSource;
import com.limelight.ui.ShellBar;
import com.limelight.ui.TopAppBar;
import com.limelight.update.UpdateFlow;
import com.limelight.utils.CacheHelper;
import com.limelight.utils.Dialog;
import com.limelight.utils.HelpLauncher;
import com.limelight.utils.LaunchSource;
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

import androidx.annotation.VisibleForTesting;

import org.xmlpull.v1.XmlPullParserException;

import javax.microedition.khronos.egl.EGLConfig;
import javax.microedition.khronos.opengles.GL10;

public class PcView extends NightjarActivity implements AdapterFragmentCallbacks {
    private View noPcFoundLayout;
    private AbsListView pcList;
    private boolean listWasEmpty = true;
    /** Resumed and not yet back in the window's focus: the first focus after a stream or another screen. */
    private boolean resumedFocusPending;
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
        @Override public void onWakeEnded(ComputerDetails computer) {
            ComputerObject row = findComputer(computer.uuid);
            // An answer, a timeout or a failed send: the row no longer says "Waking…".
            if (row != null && row.wakeSentAt != 0) {
                row.wakeSentAt = 0;
                pcGridAdapter.notifyDataSetChanged();
            }
            // A form change waited for the wake. Not at once: a wake moved to another PC ends this one first, and a
            // timeout's error dialog comes right after (its dismissal gives the window focus back, which asks again).
            followFormSoon();
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
    private final static int SLEEP_ID = HostMenu.SLEEP;
    private final static int SHARED_FOLDERS_ID = HostMenu.SHARED_FOLDERS;
    private final static int FILES_ID = HostMenu.FILES;
    private final static int COOP_ID = HostMenu.COOP;
    private final static int MIC_ONLY_ID = HostMenu.MIC_ONLY;
    private final static int CONNECTION_ADDRESS_ID = HostMenu.CONNECTION_ADDRESS;

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
        findViewById(R.id.manuallyAddPc).setOnClickListener(v -> offerAddPc());
        setUpGlass();

        getFragmentManager().beginTransaction()
            .replace(R.id.pcFragmentContainer, new AdapterFragment())
            .commitAllowingStateLoss();

        noPcFoundLayout = findViewById(R.id.no_pc_found_layout);
        findViewById(R.id.discovery_add).setOnClickListener(v -> offerAddPc());
        findViewById(R.id.discovery_help).setOnClickListener(v -> showPairingGuide());
        findViewById(R.id.pc_list_help).setOnClickListener(v -> showPairingGuide());
        updateEmptyState();
        pcGridAdapter.notifyDataSetChanged();
    }

    /** Type an address or scan the code a host shows. A device with no camera has only the first, so + goes straight to it. */
    private void offerAddPc() {
        Intent add = new Intent(this, AddComputerManually.class);
        if (!getPackageManager().hasSystemFeature(android.content.pm.PackageManager.FEATURE_CAMERA_ANY)) {
            startActivity(add);
            return;
        }
        if (addPcSheet != null) return;
        com.limelight.ui.ActionSheet sheet = new com.limelight.ui.ActionSheet(this, getString(R.string.title_add_pc));
        addPcSheet = sheet;
        sheet.addAction(R.drawable.ic_add, getString(R.string.title_add_pc)).setOnClickListener(v -> {
            sheet.dismiss();
            startActivity(add);
        });
        sheet.addAction(R.drawable.ic_glass_grid, getString(R.string.addpc_scan_code)).setOnClickListener(v -> {
            sheet.dismiss();
            startActivity(new Intent(this, AddComputerManually.class).putExtra(AddComputerManually.EXTRA_SCAN, true));
        });
        sheet.setOnDismissListener(dialog -> addPcSheet = null);
        sheet.show();
    }

    private com.limelight.ui.ActionSheet addPcSheet;

    /** Its chrome follows the form: a rotation into another form lays it out again. */
    @Override
    protected boolean followsForm() {
        return true;
    }

    /** A pairing, a wake or a failure's dialog keeps this screen until it is done; it then follows the form. */
    @Override
    protected boolean holdsForm() {
        return hostActions.isPairing() || hostActions.isWaking() || hostActions.isShowingError();
    }

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);

        // Home is the launch screen once a host has shown its games; the list of PCs stays one tap away.
        Intent openPlay = com.limelight.console.OpenPlay.atEntry(this, savedInstanceState);
        if (savedInstanceState == null && opensHome()) {
            Intent home = new Intent(this, com.limelight.console.ConsoleHomeActivity.class);
            if (openPlay != null) {
                // The game's page over Home: Cancel, a failed start and the stream's end all land there.
                startActivities(new Intent[] {home, openPlay});
            } else {
                startActivity(home);
            }
            finish();
            return;
        }
        openPlayPage = openPlay;

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

    /** "When Nightjar opens": the game's page, opened once this screen is set up. */
    private Intent openPlayPage;

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

        if (openPlayPage != null) {
            startActivity(openPlayPage);
            openPlayPage = null;
        }
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
            // Another screen may have removed or disabled a shortcut since: the first poll publishes each again.
            shortcutHelper.forgetPublished();
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

        if (chrome != null) chrome.stop();
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
        resumedFocusPending = true;
        if (completeOnCreateCalled) {
            // A host's library can arrive while this screen is away, which makes Home available.
            ShellBar.refresh(this);
        }
        // A PC removed from its settings page is gone from the manager but still in this list.
        dropRemovedComputers();
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

    // What the keys do on the selected PC shows once a key or button was the last input, not a touch; a TV has only keys.
    private boolean keysAreLastInput;
    private InputSource hintSource;
    private int hintVendor;
    private com.limelight.console.PageChrome chrome;

    /** The sky, the glass and where everything goes on this form factor: the list stands under the chrome and over the tab bar. */
    private void setUpGlass() {
        com.limelight.ui.GlassHost host = findViewById(R.id.glass_host);
        GlassForm form = GlassForm.of(this);
        View title = findViewById(R.id.pc_title);
        View add = findViewById(R.id.manuallyAddPc);
        android.widget.TextView focusTitle = findViewById(R.id.pc_focus_title);
        if (form == GlassForm.PHONE) {
            // The keys' line floats over the tab bar.
            View hint = findViewById(R.id.pc_options_hint);
            ((android.view.ViewGroup) hint.getParent()).removeView(hint);
            host.addView(hint, new android.widget.FrameLayout.LayoutParams(android.view.ViewGroup.LayoutParams.WRAP_CONTENT,
                    android.view.ViewGroup.LayoutParams.WRAP_CONTENT, android.view.Gravity.BOTTOM | android.view.Gravity.CENTER_HORIZONTAL));
        } else {
            // The tabs lead the row and the + closes it; the title below follows the selection.
            title.setVisibility(View.INVISIBLE);
            ((android.widget.LinearLayout.LayoutParams) title.getLayoutParams()).height = 0;
            focusTitle.setVisibility(View.VISIBLE);
            boolean television = form == GlassForm.TELEVISION;
            focusTitle.setTextSize(android.util.TypedValue.COMPLEX_UNIT_PX, getResources().getDimension(R.dimen.pcs_hero_title));
            ((android.widget.LinearLayout.LayoutParams) focusTitle.getLayoutParams()).topMargin =
                    Math.round(getResources().getDisplayMetrics().density * (television ? 26 : 10));
        }
        ((android.widget.LinearLayout.LayoutParams) add.getLayoutParams()).setMarginStart(getResources().getDimensionPixelSize(R.dimen.space_s));
        hintSource = InputSource.last() != null ? InputSource.last() : com.limelight.ui.StreamMenuHint.hasGamepad() ? InputSource.PAD : InputSource.REMOTE;
        keysAreLastInput = form == GlassForm.TELEVISION;
        fitFirstRun();
        com.limelight.ui.NightSkyView sky = findViewById(R.id.pc_sky);
        host.addOnLayoutChangeListener((v, l, t, r, b, ol, ot, or, ob) -> {
            if (r - l != or - ol || b - t != ob - ot) host.setAmbient(sky.bake(r - l, b - t), false, 0);
        });
        chrome = new com.limelight.console.PageChrome(this, host, R.id.pc_tabs, R.id.pc_top_row, R.id.pc_bottom_row, R.id.pc_foot_fade, this::layOutUnderChrome);
    }

    /** The orbit takes what the page has: wide on a phone, and over the word and the action on a short screen. */
    private void fitFirstRun() {
        View page = findViewById(R.id.no_pc_found_layout);
        View orbit = findViewById(R.id.first_run_orbit);
        android.view.ViewGroup column = findViewById(R.id.first_run);
        page.addOnLayoutChangeListener((v, l, t, r, b, ol, ot, or, ob) -> {
            if (column.getHeight() == 0) return;
            int width = v.getWidth() - v.getPaddingLeft() - v.getPaddingRight();
            int height = v.getHeight() - v.getPaddingTop() - v.getPaddingBottom();
            int others = column.getChildAt(1).getHeight() + column.getChildAt(2).getHeight() + column.getChildAt(3).getHeight()
                    + 2 * getResources().getDimensionPixelSize(R.dimen.space_l) + column.getPaddingBottom();
            int size = Math.max(0, Math.min(width, height - others));
            if (size != orbit.getLayoutParams().height) {
                orbit.getLayoutParams().width = size;
                orbit.getLayoutParams().height = size;
                v.post(orbit::requestLayout);
            }
        });
    }

    /** Lays the content out edge to edge, inside what the chrome and the window leave. */
    private void layOutUnderChrome(int left, int top, int right, int bottom) {
        GlassForm form = GlassForm.of(this);
        android.content.res.Resources res = getResources();
        float density = res.getDisplayMetrics().density;
        ((com.limelight.ui.NightSkyView) findViewById(R.id.pc_sky)).setSafeTop(top);
        View row = findViewById(R.id.pc_top_row);
        View frame = findViewById(R.id.pc_list_frame);
        android.widget.FrameLayout.LayoutParams params = (android.widget.FrameLayout.LayoutParams) frame.getLayoutParams();
        // First run's last action keeps its ring clear of the screen's edge, the gesture bar and a phone's tabs.
        findViewById(R.id.first_run).setPadding(0, 0, 0, bottom + res.getDimensionPixelSize(R.dimen.space_l)
                + (form == GlassForm.PHONE ? res.getDimensionPixelSize(R.dimen.nj_size_tab_bar_height) : 0));
        if (form == GlassForm.PHONE) {
            int edge = res.getDimensionPixelSize(R.dimen.glass_edge);
            int rowTop = top + Math.round(39 * density);
            row.setPaddingRelative(left + Math.round(20 * density), rowTop, right + edge, 0);
            params.topMargin = rowTop + res.getDimensionPixelSize(R.dimen.glass_capsule) + Math.round(12 * density);
            params.leftMargin = left;
            params.rightMargin = right;
            View bar = findViewById(R.id.pc_bottom_row);
            bar.setPadding(left + res.getDimensionPixelSize(R.dimen.space_m), 0, right + res.getDimensionPixelSize(R.dimen.space_m),
                    bottom + res.getDimensionPixelSize(R.dimen.space_s));
            listFoot = bottom + res.getDimensionPixelSize(R.dimen.nj_size_tab_bar_height) + 3 * res.getDimensionPixelSize(R.dimen.space_m);
            View fade = findViewById(R.id.pc_foot_fade);
            android.view.ViewGroup.LayoutParams fadeSize = fade.getLayoutParams();
            fadeSize.height = res.getDimensionPixelSize(R.dimen.home_foot_fade) + bottom;
            fade.setLayoutParams(fadeSize);
            View hint = findViewById(R.id.pc_options_hint);
            ((android.widget.FrameLayout.LayoutParams) hint.getLayoutParams()).bottomMargin = bottom
                    + res.getDimensionPixelSize(R.dimen.nj_size_tab_bar_height) + res.getDimensionPixelSize(R.dimen.space_m);
        } else {
            boolean television = form == GlassForm.TELEVISION;
            int side = res.getDimensionPixelSize(R.dimen.shell_overscan_horizontal);
            int inset = television ? 0 : Math.round(24 * density);
            // The capsule stands where it stands on Home; only a handheld keeps a gap above the safe area.
            int rowTop = television ? top : top + res.getDimensionPixelSize(R.dimen.space_m);
            row.setPaddingRelative(left + inset, rowTop, right + inset, 0);
            View focusTitle = findViewById(R.id.pc_focus_title);
            focusTitle.setPaddingRelative(left + inset, 0, right + inset + Math.round(density * (television ? 300 : 200)), 0);
            View hint = findViewById(R.id.pc_options_hint);
            hint.setPaddingRelative(left + inset, hint.getPaddingTop(), 0, hint.getPaddingBottom());
            params.topMargin = top + res.getDimensionPixelSize(R.dimen.pcs_list_top);
            // A card pads itself with its glow: the card itself starts under the title, the glow past it.
            int glow = res.getDimensionPixelSize(R.dimen.tile_glow);
            params.leftMargin = left + side + inset - glow;
            params.rightMargin = right + side + inset - glow;
            ((com.limelight.ui.ContentFrame) frame).setMaxContentWidth(Integer.MAX_VALUE);
            listFoot = bottom;
        }
        frame.setLayoutParams(params);
        applyListFoot();
    }

    /** What the list keeps free under its last row. */
    private int listFoot;

    private void applyListFoot() {
        if (pcList != null && listFoot != pcList.getPaddingBottom()) {
            pcList.setPadding(pcList.getPaddingLeft(), pcList.getPaddingTop(), pcList.getPaddingRight(), listFoot);
        }
    }

    /** The selected PC's name and what the keys do on it. */
    private void showSelection() {
        if (pcList == null) return;
        Object item = pcList.getSelectedItem();
        ComputerObject computer = item instanceof ComputerObject ? (ComputerObject) item : null;
        android.widget.TextView title = findViewById(R.id.pc_focus_title);
        if (title.getVisibility() == View.VISIBLE) {
            CharSequence name = computer != null ? computer.details.displayName() : getString(R.string.library_computers);
            if (!name.toString().contentEquals(title.getText())) title.setText(name);
        }
        android.widget.TextView hint = findViewById(R.id.pc_options_hint);
        if (hintSource == null) return;
        CharSequence line = computer == null ? null : com.limelight.console.HomeHints.line(this, hintSource, hintVendor,
                goWord(computer), getString(R.string.console_hint_options));
        hint.setText(line != null ? line : "");
    }

    /** What OK does on this PC, which is what a tap does: the word must not promise anything else. */
    private CharSequence goWord(ComputerObject computer) {
        PcGridAdapter.RowState state = computer.rowState();
        if (LaunchPolicy.resumesOnPcTap(PreferenceConfiguration.readPreferences(this).quickResume, state == PcGridAdapter.RowState.RUNNING)) {
            return getString(R.string.applist_menu_resume);
        }
        if (state == PcGridAdapter.RowState.RUNNING) return getString(R.string.console_hint_open);
        int word = PcGridAdapter.actionWord(this, computer);
        return word == 0 ? null : getString(word);
    }

    @Override
    public boolean dispatchKeyEvent(android.view.KeyEvent event) {
        if (event.getAction() == android.view.KeyEvent.ACTION_DOWN) {
            InputSource source = InputSource.of(event);
            if (source != null) {
                InputSource.remember(source);
                hintSource = source;
                hintVendor = InputSource.vendorOf(event);
            }
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
        showSelection();
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
        // With no PC there is no selection to name, and the selected tab already names the page.
        if (GlassForm.of(this) != GlassForm.PHONE) {
            findViewById(R.id.pc_focus_title).setVisibility(empty ? View.GONE : View.VISIBLE);
            showSelection();
        }
        ((com.limelight.ui.OrbitView) findViewById(R.id.first_run_orbit)).setMoving(empty);
        com.limelight.ui.NightSkyView sky = findViewById(R.id.pc_sky);
        if (sky.isMoonShown() == empty) {
            sky.setMoonShown(!empty);
            com.limelight.ui.GlassHost host = findViewById(R.id.glass_host);
            if (host.getWidth() > 0) host.setAmbient(sky.bake(host.getWidth(), host.getHeight()), false, 0);
        }
        findViewById(R.id.pcFragmentContainer).setVisibility(empty ? View.GONE : View.VISIBLE);
        findViewById(R.id.manuallyAddPc).setVisibility(empty ? View.GONE : View.VISIBLE);
        if (GlassForm.of(this) != GlassForm.PHONE) ShellBar.linkTopRow(this, R.id.manuallyAddPc);
        placeListFooter();
        boolean filled = listWasEmpty && !empty;
        listWasEmpty = empty;
        if (filled && pcList != null && !pcList.isInTouchMode()) {
            // The first PC replaced the empty state, which held focus; give it to the list, not the shell bar.
            claimListFocus();
        }
    }

    /**
     * Puts the footer just under the last PC, as long as every PC shows and the footer fits above the tab bar; otherwise it
     * waits, invisible. It fills the screen a few PCs leave empty with what a new user needs next: the list is still
     * looking, and how to connect a PC that does not appear. A TV has none: its tiles sit centred on the screen.
     * <p>It is placed by its top margin, not a translation, which focus search ignores; while it shows, Down from the
     * last row goes through its link.
     */
    @VisibleForTesting
    void placeListFooter() {
        View footer = findViewById(R.id.pc_list_footer);
        View container = findViewById(R.id.pcFragmentContainer);
        if (footer == null || pcList == null || pcGridAdapter == null) return;
        boolean television = (getResources().getConfiguration().uiMode & Configuration.UI_MODE_TYPE_MASK)
                == Configuration.UI_MODE_TYPE_TELEVISION;
        int count = pcGridAdapter.getCount();
        boolean show = false;
        if (!television && count > 0 && container.getVisibility() == View.VISIBLE && pcList.getFirstVisiblePosition() == 0
                && pcList.getChildCount() == count) {
            View last = pcList.getChildAt(count - 1);
            int top = container.getTop() + pcList.getTop() + last.getBottom()
                    + getResources().getDimensionPixelSize(R.dimen.space_l);
            int room = ((View) footer.getParent()).getHeight() - listFoot;
            android.view.ViewGroup.MarginLayoutParams params = (android.view.ViewGroup.MarginLayoutParams) footer.getLayoutParams();
            if (params.topMargin != top) {
                params.topMargin = top;
                footer.setLayoutParams(params);
            }
            show = top + footer.getHeight() <= room;
        }
        int visibility = show ? View.VISIBLE : View.INVISIBLE;
        if (footer.getVisibility() != visibility) footer.setVisibility(visibility);
        linkListFooter(show);
    }

    /** With the footer shown its link sits under the PCs, for the D-pad and for a screen reader. */
    private void linkListFooter(boolean shown) {
        View help = findViewById(R.id.pc_list_help);
        pcList.setNextFocusDownId(shown ? R.id.pc_list_help : View.NO_ID);
        help.setNextFocusUpId(R.id.fragmentView);
        // Android 5.0 has no traversal order: a screen reader there reads the views in layout order, footer last.
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.LOLLIPOP_MR1) {
            help.setAccessibilityTraversalAfter(R.id.fragmentView);
        }
    }

    private boolean focusIsLoose() {
        View focus = getCurrentFocus();
        return focus == null || isInside(focus, R.id.shell_bar) || isInside(focus, R.id.no_pc_found_layout);
    }

    @Override
    public void onWindowFocusChanged(boolean hasFocus) {
        super.onWindowFocusChanged(hasFocus);
        if (!hasFocus || !resumedFocusPending) return;
        resumedFocusPending = false;
        // Back from a stream a pad or a remote drove (its menu's Disconnect), the PC that runs the game takes the ring, so A
        // acts at once. This window can still be in the touch mode its last tap left: the keys of the screen before end it.
        if (pcList == null || pcGridAdapter == null || pcGridAdapter.getCount() == 0) return;
        boolean touch = pcList.isInTouchMode();
        InputSource last = InputSource.last();
        if (touch && (last == null || last == InputSource.NONE)) return;
        boolean ringShown = !touch && pcList.hasFocus() && pcList.getSelectedItemPosition() != android.widget.AdapterView.INVALID_POSITION;
        if (ringShown || (!pcList.hasFocus() && !focusIsLoose())) return;
        // Out of touch mode first: a list in touch mode keeps no selection.
        if (touch) pcList.requestFocusFromTouch();
        for (int i = 0; i < pcGridAdapter.getCount(); i++) {
            Object item = pcGridAdapter.getItem(i);
            if (item instanceof ComputerObject && ((ComputerObject) item).rowState() == PcGridAdapter.RowState.RUNNING) {
                pcList.setSelection(i);
                break;
            }
        }
        if (!touch) claimListFocus();
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
     * The PC's menu, in this order: Open library, Resume and Quit, Join co-op, Shared folders, Pair, one-time PIN, Wake
     * or Sleep, Files, PC settings, Rename, Unpair or Pairing help, then Remove PC last. Touch reaches it by long press
     * or the three dots, a mouse by right-click, a D-pad or keyboard by Menu or Shift+F10, a controller by X.
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
            // Pair from the menu stopped the polling, which would race the pairing; it restarts once that ends.
            if (!hostActions.isPairing()) startComputerUpdates();
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

            case CONNECTION_ADDRESS_ID:
                hostActions.chooseAddress(computer.details);
                return true;

            case WOL_ID:
                doWakeOnLan(computer.details);
                return true;

            case SLEEP_ID:
                hostActions.sleep(computer.details);
                return true;

            case HostMenu.RESTART:
                hostActions.pcPower(computer.details, NativePcPower.Action.RESTART);
                return true;

            case HostMenu.SHUT_DOWN:
                hostActions.pcPower(computer.details, NativePcPower.Action.SHUT_DOWN);
                return true;

            case HostMenu.UPDATE_HOST:
                hostActions.updateHost(computer.details);
                return true;

            case HostMenu.PAIR_ANOTHER:
                hostActions.pairAnother(computer.details);
                return true;

            case COOP_ID:
                hostActions.joinCoop(computer.details);
                return true;

            case MIC_ONLY_ID:
                hostActions.micOnly(computer.details);
                return true;

            case FILES_ID:
                startActivity(FileTransferActivity.forPc(this, computer.details.uuid));
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
                ServerHelper.doStart(this, HostMenu.runningApp(computer.runningGame, computer.details), computer.details, managerBinder,
                        LaunchSource.PC_RESUME);
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

            case SHARED_FOLDERS_ID:
                FolderBrowserActivity.open(this, computer.details);
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
                if (WakeTargets.hasMac(this, computer.details)) {
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

    /** Takes the rows of PCs the manager no longer knows off the list. */
    void dropRemovedComputers() {
        ComputerManagerService.ComputerManagerBinder binder = managerBinder;
        if (binder == null) {
            return;
        }
        List<ComputerDetails> gone = new ArrayList<>();
        for (int i = 0; i < pcGridAdapter.getCount(); i++) {
            ComputerDetails details = ((ComputerObject) pcGridAdapter.getItem(i)).details;
            if (binder.getComputer(details.uuid) == null) {
                gone.add(details);
            }
        }
        for (ComputerDetails details : gone) {
            removeFromList(details);
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
        // The first saved app list of a PC without the native API makes Home available.
        View home = details.rawAppList != null ? findViewById(R.id.shell_home) : null;
        if (home != null && home.getVisibility() != View.VISIBLE) ShellBar.refresh(this);

        // Notify the view that the data has changed. Every poll reports its PC, changed or not, and a refresh sorts
        // and binds every row, so only news asks for one. A running game's cover that was not on disk is looked for
        // again by a later bind, so the grid keeps refreshing while one is missing.
        if (existingEntry.changedSinceDrawn() || pcGridAdapter.awaitsCover()) {
            pcGridAdapter.notifyDataSetChanged();
        }
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
        return GlassForm.of(this) == GlassForm.PHONE ? R.layout.pc_grid_view : R.layout.pc_grid_cards;
    }

    @Override
    public void receiveAbsListView(AbsListView listView) {
        pcList = listView;
        listView.setAdapter(pcGridAdapter);
        applyListFoot();
        pcGridAdapter.setActions(new PcGridAdapter.Actions() {
            @Override public void onResume(ComputerObject computer) { onComputerAction(computer, RESUME_ID); }
            @Override public void onMain(ComputerObject computer) { onComputerTapped(computer); }
        });
        listView.setOnItemSelectedListener(new android.widget.AdapterView.OnItemSelectedListener() {
            @Override public void onItemSelected(android.widget.AdapterView<?> parent, View view, int position, long id) { showSelection(); }
            @Override public void onNothingSelected(android.widget.AdapterView<?> parent) { showSelection(); }
        });
        listView.setOnItemClickListener((parent, view, position, id) ->
                onComputerTapped((ComputerObject) pcGridAdapter.getItem(position)));
        listView.setOnItemLongClickListener((parent, view, position, id) -> {
            showComputerMenu((ComputerObject) pcGridAdapter.getItem(position));
            return true;
        });
        if (listView instanceof com.limelight.ui.AdaptiveGridView) {
            ((com.limelight.ui.AdaptiveGridView) listView).setEndFocusTarget(findViewById(R.id.manuallyAddPc));
        }
        listView.addOnLayoutChangeListener((v, l, t, r, b, ol, ot, or, ob) -> placeListFooter());
        listView.setOnScrollListener(new AbsListView.OnScrollListener() {
            @Override public void onScrollStateChanged(AbsListView view, int state) { }
            @Override public void onScroll(AbsListView view, int first, int visible, int total) { placeListFooter(); }
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
        /** What a poll last had the grid draw this row from: every field of the PC, and whether it was waking. */
        private Object[] drawnFields;
        private boolean drawnWaking;

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

        /**
         * Whether the row has news since a poll last had the grid draw it, which counts as drawn from now on. It
         * compares every field of the PC ({@link ComputerDetails#fields}), not only the ones the row is known to
         * read, and the waking state, which ends by the clock with no poll saying so.
         */
        boolean changedSinceDrawn() {
            Object[] fields = details.fields();
            boolean waking = isWaking();
            if (waking == drawnWaking && Arrays.deepEquals(fields, drawnFields)) {
                return false;
            }
            drawnFields = fields;
            drawnWaking = waking;
            return true;
        }

        @Override
        public String toString() {
            return details.displayName();
        }
    }
}
