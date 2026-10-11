package com.limelight;

import java.io.FileNotFoundException;
import java.io.IOException;
import java.net.UnknownHostException;

import com.limelight.binding.video.DecoderBenchmark;
import com.limelight.binding.PlatformBinding;
import com.limelight.binding.crypto.AndroidCryptoProvider;
import com.limelight.computers.ComputerManagerListener;
import com.limelight.computers.ComputerManagerService;
import com.limelight.grid.PcGridAdapter;
import com.limelight.grid.assets.DiskAssetLoader;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.http.NvHTTP;
import com.limelight.nvstream.http.PairingManager;
import com.limelight.nvstream.http.PairingManager.PairState;
import com.limelight.nvstream.wol.WakeOnLanSender;
import com.limelight.preferences.AddComputerManually;
import com.limelight.preferences.GlPreferences;
import com.limelight.preferences.HostDisplaySettings;
import com.limelight.preferences.HostStreamSettings;
import com.limelight.preferences.PreferenceConfiguration;
import com.limelight.preferences.StreamSettings;
import com.limelight.ui.AdapterFragment;
import com.limelight.ui.AdapterFragmentCallbacks;
import com.limelight.utils.Dialog;
import com.limelight.utils.FrontendExporter;
import com.limelight.utils.HelpLauncher;
import com.limelight.utils.HostDetailsDialog;
import com.limelight.utils.PyroWaveBandwidthTest;
import com.limelight.utils.ServerHelper;
import com.limelight.utils.ShortcutHelper;
import com.limelight.utils.UiHelper;

import androidx.appcompat.app.AppCompatActivity;
import android.app.ActivityManager;
import androidx.appcompat.app.AlertDialog;
import com.google.android.material.dialog.MaterialAlertDialogBuilder;
import android.app.Service;
import android.content.ActivityNotFoundException;
import android.content.ComponentName;
import android.content.Intent;
import android.content.ServiceConnection;
import android.content.res.Configuration;
import android.net.Uri;
import android.opengl.GLSurfaceView;
import android.os.Build;
import android.os.Bundle;
import android.os.IBinder;
import android.preference.PreferenceManager;
import android.text.InputFilter;
import android.text.InputType;
import com.limelight.ui.ActionSheet;
import com.google.android.material.appbar.MaterialToolbar;
import android.view.View;
import android.view.WindowManager;
import android.view.View.OnClickListener;
import android.widget.AbsListView;
import android.widget.AdapterView;
import android.widget.AdapterView.OnItemClickListener;
import android.widget.EditText;
import android.widget.TextView;
import android.widget.Toast;

import org.xmlpull.v1.XmlPullParserException;

import javax.microedition.khronos.egl.EGLConfig;
import javax.microedition.khronos.opengles.GL10;

public class PcView extends AppCompatActivity implements AdapterFragmentCallbacks {
    private View noPcFoundLayout;
    private ActionSheet pcMenu;
    private PcGridAdapter pcGridAdapter;
    private ShortcutHelper shortcutHelper;
    private PyroWaveBandwidthTest bandwidthTest;
    private NvHTTP pairingRequest;
    private Thread pairingThread;
    private volatile int pairingGeneration;
    private AlertDialog pairingDialog;
    private ComputerDetails exportComputer;
    private Uri exportRomsTree;
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
        }
    };

    @Override
    public void onConfigurationChanged(Configuration newConfig) {
        super.onConfigurationChanged(newConfig);

        if (completeOnCreateCalled) {
            UiHelper.notifyNewRootView(this);
        }
    }

    private final static int PAIR_ID = 2;
    private final static int UNPAIR_ID = 3;
    private final static int WOL_ID = 4;
    private final static int DELETE_ID = 5;
    private final static int RESUME_ID = 6;
    private final static int QUIT_ID = 7;
    private final static int VIEW_DETAILS_ID = 8;
    private final static int FULL_APP_LIST_ID = 9;
    private final static int GAMESTREAM_EOL_ID = 11;
    private final static int HOST_SETTINGS_ID = 12;
    private final static int BANDWIDTH_PROBE_ID = 14;
    private final static int OTP_PAIR_ID = 13;
    private final static int FRONTEND_EXPORT_ID = 15;
    private final static int HOST_DISPLAY_ID = 16;

    private final static int PICK_ROMS_REQUEST = 1;
    private final static int PICK_ESDE_REQUEST = 2;
    private final static String FIRST_RUN_PREFS = "FirstRun";
    private final static String PAIRING_GUIDE_SHOWN = "pairing_guide_shown";

    private void initializeViews() {
        setContentView(R.layout.activity_pc_view);

        UiHelper.notifyNewRootView(this);

        // Allow floating expanded PiP overlays while browsing PCs
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            setShouldDockBigOverlays(false);
        }

        // Set default preferences if we've never been run
        PreferenceManager.setDefaultValues(this, R.xml.preferences, false);

        // Set the correct layout for the PC grid
        pcGridAdapter.updateLayoutWithPreferences(this, PreferenceConfiguration.readPreferences(this));

        MaterialToolbar toolbar = findViewById(R.id.home_toolbar);
        toolbar.setOnMenuItemClickListener(item -> {
            if (item.getItemId() == R.id.settingsButton) {
                startActivity(new Intent(this, StreamSettings.class));
            } else if (item.getItemId() == R.id.helpButton) {
                showPairingGuide();
            }
            return true;
        });
        toolbar.getMenu().findItem(R.id.helpButton).setVisible(!getPackageManager().hasSystemFeature("amazon.hardware.fire_tv"));
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

        // Once per phone and app version: lets Automatic skip codecs that decode slower here.
        DecoderBenchmark.runIfNeeded(this);

        if (savedInstanceState != null && savedInstanceState.containsKey("exportComputerUuid")) {
            exportComputer = new ComputerDetails();
            exportComputer.uuid = savedInstanceState.getString("exportComputerUuid");
            exportComputer.name = savedInstanceState.getString("exportComputerName");
            String romsTree = savedInstanceState.getString("exportRomsTree");
            exportRomsTree = romsTree != null ? Uri.parse(romsTree) : null;
        }

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

        // Explain adding and pairing once; the help button shows it again
        if (!getSharedPreferences(FIRST_RUN_PREFS, MODE_PRIVATE).getBoolean(PAIRING_GUIDE_SHOWN, false)) {
            getSharedPreferences(FIRST_RUN_PREFS, MODE_PRIVATE).edit().putBoolean(PAIRING_GUIDE_SHOWN, true).apply();
            showPairingGuide();
        }
    }

    private void showPairingGuide() {
        new MaterialAlertDialogBuilder(this)
                .setTitle(R.string.guide_title)
                .setMessage(R.string.guide_text)
                .setPositiveButton(R.string.guide_done, null)
                .setNeutralButton(R.string.guide_troubleshooting,
                        (dialog, which) -> HelpLauncher.launchTroubleshooting(PcView.this))
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

    private void startFrontendExport(ComputerDetails computer) {
        if (new FrontendExporter(this, computer).loadApps().isEmpty()) {
            Toast.makeText(this, R.string.frontend_export_no_apps, Toast.LENGTH_LONG).show();
            return;
        }
        exportComputer = computer;
        exportRomsTree = null;
        new MaterialAlertDialogBuilder(this)
                .setTitle(R.string.frontend_export_roms_title)
                .setMessage(R.string.frontend_export_roms_message)
                .setPositiveButton(R.string.frontend_export_pick, (dialog, which) -> pickFolder(PICK_ROMS_REQUEST))
                .setNegativeButton(android.R.string.cancel, null)
                .show();
    }

    private void pickFolder(int request) {
        try {
            startActivityForResult(new Intent(Intent.ACTION_OPEN_DOCUMENT_TREE), request);
        } catch (ActivityNotFoundException e) {
            Toast.makeText(this, R.string.frontend_export_no_picker, Toast.LENGTH_LONG).show();
        }
    }

    @Override
    protected void onActivityResult(int requestCode, int resultCode, Intent data) {
        super.onActivityResult(requestCode, resultCode, data);
        Uri picked = resultCode == RESULT_OK && data != null ? data.getData() : null;
        if (exportComputer == null) {
            return;
        }
        if (requestCode == PICK_ROMS_REQUEST) {
            if (picked == null) {
                exportComputer = null;
                return;
            }
            exportRomsTree = picked;
            new MaterialAlertDialogBuilder(this)
                    .setTitle(R.string.frontend_export_esde_title)
                    .setMessage(R.string.frontend_export_esde_message)
                    .setPositiveButton(R.string.frontend_export_pick, (dialog, which) -> pickFolder(PICK_ESDE_REQUEST))
                    .setNegativeButton(R.string.frontend_export_skip, (dialog, which) -> runFrontendExport(null))
                    .setOnCancelListener(dialog -> runFrontendExport(null))
                    .show();
        } else if (requestCode == PICK_ESDE_REQUEST && exportRomsTree != null) {
            runFrontendExport(picked);
        }
    }

    private void runFrontendExport(final Uri esdeTree) {
        final ComputerDetails computer = exportComputer;
        final Uri romsTree = exportRomsTree;
        exportComputer = null;
        exportRomsTree = null;
        if (computer == null || romsTree == null) {
            return;
        }
        Toast.makeText(this, R.string.frontend_export_working, Toast.LENGTH_SHORT).show();
        new Thread(() -> {
            String message;
            try {
                FrontendExporter exporter = new FrontendExporter(PcView.this, computer);
                int count = exporter.export(exporter.loadApps(), romsTree, esdeTree);
                message = getResources().getQuantityString(R.plurals.frontend_export_done, count, count);
            } catch (IOException | RuntimeException e) {
                LimeLog.warning("Frontend export failed: " + e);
                message = getString(R.string.frontend_export_failed);
            }
            final String result = message;
            runOnUiThread(() -> Toast.makeText(PcView.this, result, Toast.LENGTH_LONG).show());
        }, "Frontend export").start();
    }

    @Override
    protected void onSaveInstanceState(Bundle outState) {
        super.onSaveInstanceState(outState);
        if (exportComputer != null) {
            outState.putString("exportComputerUuid", exportComputer.uuid);
            outState.putString("exportComputerName", exportComputer.name);
            outState.putString("exportRomsTree", exportRomsTree != null ? exportRomsTree.toString() : null);
        }
    }

    @Override
    public void onDestroy() {
        super.onDestroy();

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
        if (pcMenu != null) pcMenu.dismiss();
        cancelPairing();
        if (bandwidthTest != null) {
            bandwidthTest.cancel();
            bandwidthTest = null;
        }
        stopComputerUpdates(false);
    }

    @Override
    protected void onStop() {
        super.onStop();

        Dialog.closeDialogs(PcView.this);
    }

    private void updateEmptyState() {
        boolean empty = pcGridAdapter.getCount() == 0;
        noPcFoundLayout.setVisibility(empty ? View.VISIBLE : View.GONE);
        findViewById(R.id.pcFragmentContainer).setVisibility(empty ? View.GONE : View.VISIBLE);
        findViewById(R.id.manuallyAddPc).setVisibility(empty ? View.GONE : View.VISIBLE);
    }

    private void showComputerMenu(ComputerObject computer) {
        if (pcMenu != null) return;
        stopComputerUpdates(false);
        ActionSheet sheet = new ActionSheet(this, computer.details.name);
        pcMenu = sheet;
        boolean online = computer.details.state == ComputerDetails.State.ONLINE;
        boolean paired = computer.details.pairState == PairState.PAIRED;
        if (online && paired && computer.details.runningGameId != 0) {
            sheet.addSection(R.string.settings_section_video);
            addPcAction(sheet, computer, RESUME_ID, R.drawable.ic_play, R.string.applist_menu_resume);
            addPcAction(sheet, computer, QUIT_ID, R.drawable.ic_stop, R.string.applist_menu_quit);
        }
        sheet.addSection(R.string.pc_sheet_pairing);
        if (online && !paired) {
            addPcAction(sheet, computer, PAIR_ID, R.drawable.ic_lock, R.string.pcview_menu_pair_pc);
            if (computer.details.rustHostVersion != null || computer.details.permission != -1) {
                addPcAction(sheet, computer, OTP_PAIR_ID, R.drawable.ic_keyboard, R.string.pair_otp_title);
            }
        }
        sheet.addAction(R.drawable.ic_help, getString(R.string.discovery_help)).setOnClickListener(v -> {
            sheet.dismiss();
            showPairingGuide();
        });
        sheet.addSection(R.string.pc_sheet_this_pc);
        addPcAction(sheet, computer, HOST_SETTINGS_ID, R.drawable.ic_settings, R.string.host_profile_menu);
        if (!computer.details.nvidiaServer) {
            addPcAction(sheet, computer, HOST_DISPLAY_ID, R.drawable.ic_computer, R.string.host_display_menu);
        }
        addPcAction(sheet, computer, BANDWIDTH_PROBE_ID, R.drawable.ic_network, R.string.connection_test_title);
        addPcAction(sheet, computer, WOL_ID, R.drawable.ic_power, R.string.pcview_menu_send_wol);
        if (online && paired) {
            addPcAction(sheet, computer, FULL_APP_LIST_ID, R.drawable.ic_library, R.string.pcview_menu_app_list);
            addPcAction(sheet, computer, FRONTEND_EXPORT_ID, R.drawable.ic_library, R.string.frontend_export_menu);
        }
        if (computer.details.nvidiaServer) {
            addPcAction(sheet, computer, GAMESTREAM_EOL_ID, R.drawable.ic_help, R.string.pcview_menu_eol);
        }
        addPcAction(sheet, computer, VIEW_DETAILS_ID, R.drawable.ic_help, R.string.pcview_menu_details);
        sheet.addDivider();
        addPcAction(sheet, computer, DELETE_ID, R.drawable.ic_delete, R.string.pcview_menu_delete_pc);
        sheet.setOnDismissListener(dialog -> {
            pcMenu = null;
            startComputerUpdates();
        });
        sheet.show();
    }

    private void addPcAction(ActionSheet sheet, ComputerObject computer, int action, int icon, int label) {
        sheet.addAction(icon, getString(label)).setOnClickListener(v -> {
            sheet.dismiss();
            onComputerAction(computer, action);
        });
    }

    private void doPair(final ComputerDetails computer) {
        doPair(computer, null, null);
    }

    private void showOneTimePinDialog(final ComputerDetails computer) {
        View fields = getLayoutInflater().inflate(R.layout.dialog_pair_otp, null);
        EditText pin = fields.findViewById(R.id.pair_pin);
        pin.setInputType(InputType.TYPE_CLASS_NUMBER | InputType.TYPE_NUMBER_VARIATION_PASSWORD);
        pin.setFilters(new InputFilter[] {new InputFilter.LengthFilter(4)});
        pin.setSaveEnabled(false);
        EditText passphrase = fields.findViewById(R.id.pair_passphrase);
        passphrase.setInputType(InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_VARIATION_PASSWORD);
        passphrase.setSingleLine(true);
        passphrase.setSaveEnabled(false);
        AlertDialog dialog = new MaterialAlertDialogBuilder(this)
                .setTitle(R.string.pair_otp_title)
                .setView(fields)
                .setNegativeButton(android.R.string.cancel, null)
                .setPositiveButton(R.string.pcview_menu_pair_pc, null)
                .create();
        dialog.setOnShowListener(ignored -> dialog.getButton(AlertDialog.BUTTON_POSITIVE).setOnClickListener(view -> {
            String pinValue = pin.getText().toString();
            String passphraseValue = passphrase.getText().toString();
            if (!pinValue.matches("[0-9]{4}")) {
                pin.setError(getString(R.string.pair_otp_invalid_pin));
                return;
            }
            if (passphraseValue.codePointCount(0, passphraseValue.length()) < 4) {
                passphrase.setError(getString(R.string.pair_otp_invalid_passphrase));
                return;
            }
            dialog.dismiss();
            doPair(computer, pinValue, passphraseValue);
        }));
        dialog.setOnDismissListener(ignored -> {
            pin.getText().clear();
            passphrase.getText().clear();
        });
        dialog.getWindow().setSoftInputMode(WindowManager.LayoutParams.SOFT_INPUT_ADJUST_RESIZE);
        dialog.show();
    }

    private synchronized void cancelPairing() {
        pairingGeneration++;
        if (pairingRequest != null) {
            pairingRequest.cancelPendingRequests();
            pairingRequest = null;
            LimeLog.info("Pairing cancelled");
        }
        if (pairingThread != null) {
            pairingThread.interrupt();
            pairingThread = null;
        }
        if (pairingDialog != null) {
            pairingDialog.dismiss();
            pairingDialog = null;
        }
    }

    private void doPair(final ComputerDetails computer, final String oneTimePin, final String passphrase) {
        if (computer.state == ComputerDetails.State.OFFLINE || computer.activeAddress == null) {
            Toast.makeText(this, R.string.pair_pc_offline, Toast.LENGTH_LONG).show();
            return;
        }
        final ComputerManagerService.ComputerManagerBinder binder = managerBinder;
        if (binder == null) {
            Toast.makeText(this, R.string.error_manager_not_running, Toast.LENGTH_LONG).show();
            return;
        }
        cancelPairing();
        final int generation = pairingGeneration;
        stopComputerUpdates(false);
        LimeLog.info("Pairing started");
        pairingThread = new Thread(() -> {
            String message = null;
            boolean success = false;
            try {
                binder.waitForPollingStopped();
                NvHTTP request = new NvHTTP(ServerHelper.getCurrentAddressFromComputer(computer),
                        computer.httpsPort, binder.getUniqueId(), computer.serverCert,
                        PlatformBinding.getCryptoProvider(getApplicationContext()));
                synchronized (PcView.this) {
                    if (generation != pairingGeneration) {
                        request.cancelPendingRequests();
                        return;
                    }
                    pairingRequest = request;
                }
                if (request.getPairState() == PairState.PAIRED) {
                    success = true;
                } else {
                    String pin = oneTimePin == null ? PairingManager.generatePinString() : oneTimePin;
                    runOnUiThread(() -> {
                        if (generation != pairingGeneration || !inForeground || isFinishing() || isDestroyed()) {
                            return;
                        }
                        android.text.SpannableStringBuilder pairingText = new android.text.SpannableStringBuilder();
                        if (oneTimePin == null) {
                            pairingText.append(getString(R.string.pair_pairing_msg)).append("\n\n");
                            int pinStart = pairingText.length();
                            pairingText.append(pin);
                            pairingText.setSpan(new android.text.style.RelativeSizeSpan(2f), pinStart, pairingText.length(), 0);
                            pairingText.setSpan(new android.text.style.StyleSpan(android.graphics.Typeface.BOLD), pinStart, pairingText.length(), 0);
                            pairingText.append("\n\n").append(getString(R.string.pair_pairing_help));
                        } else {
                            pairingText.append(getString(R.string.pairing));
                        }
                        pairingDialog = new MaterialAlertDialogBuilder(this).setTitle(R.string.pair_pairing_title)
                                .setMessage(pairingText)
                                .setNegativeButton(android.R.string.cancel, (dialog, which) -> {
                                    cancelPairing();
                                    startComputerUpdates();
                                }).setOnCancelListener(dialog -> {
                                    cancelPairing();
                                    startComputerUpdates();
                                }).create();
                        pairingDialog.show();
                    });
                    PairingManager pairing = request.getPairingManager();
                    PairState state = pairing.pair(request.getServerInfo(true), pin, passphrase);
                    if (state == PairState.PAIRED) {
                        ComputerDetails stored = binder.getComputer(computer.uuid);
                        if (stored != null) {
                            stored.serverCert = pairing.getPairedCert();
                            binder.invalidateStateForComputer(computer.uuid);
                            success = true;
                        } else {
                            message = getString(R.string.pair_fail);
                        }
                    } else {
                        message = getString(state == PairState.PIN_WRONG ?
                                (oneTimePin == null ? R.string.pair_incorrect_pin : R.string.pair_otp_incorrect_pin) :
                                state == PairState.ALREADY_IN_PROGRESS ? R.string.pair_already_in_progress :
                                        computer.runningGameId != 0 ? R.string.pair_pc_ingame : R.string.pair_fail);
                    }
                }
            } catch (IOException | XmlPullParserException e) {
                message = getString(R.string.pair_connection_fix);
            }
            final boolean paired = success;
            final String failure = message;
            runOnUiThread(() -> {
                if (generation != pairingGeneration || !inForeground || isFinishing() || isDestroyed()) {
                    return;
                }
                pairingRequest = null;
                pairingThread = null;
                if (pairingDialog != null) {
                    pairingDialog.dismiss();
                    pairingDialog = null;
                }
                LimeLog.info(paired ? "Pairing succeeded" : "Pairing failed");
                if (paired) {
                    doAppList(computer, true, false);
                } else {
                    startComputerUpdates();
                    new MaterialAlertDialogBuilder(this).setTitle(R.string.pair_pairing_title)
                            .setMessage(failure).setNegativeButton(android.R.string.cancel, null)
                            .setPositiveButton(R.string.pair_try_again, (dialog, which) -> {
                                if (oneTimePin == null) {
                                    doPair(computer);
                                } else {
                                    showOneTimePinDialog(computer);
                                }
                            }).show();
                }
            });
        }, "Pairing");
        pairingThread.start();
    }

    private void doWakeOnLan(final ComputerDetails computer) {
        if (computer.state == ComputerDetails.State.ONLINE) {
            Toast.makeText(PcView.this, getResources().getString(R.string.wol_pc_online), Toast.LENGTH_SHORT).show();
            return;
        }

        if (computer.macAddress == null) {
            Toast.makeText(PcView.this, getResources().getString(R.string.wol_no_mac), Toast.LENGTH_SHORT).show();
            return;
        }

        new Thread(new Runnable() {
            @Override
            public void run() {
                String message;
                try {
                    WakeOnLanSender.sendWolPacket(computer);
                    message = getResources().getString(R.string.wol_waking_msg);
                } catch (IOException e) {
                    message = getResources().getString(R.string.wol_fail);
                }

                final String toastMessage = message;
                runOnUiThread(new Runnable() {
                    @Override
                    public void run() {
                        Toast.makeText(PcView.this, toastMessage, Toast.LENGTH_LONG).show();
                    }
                });
            }
        }).start();
    }

    private void doUnpair(final ComputerDetails computer) {
        if (computer.state == ComputerDetails.State.OFFLINE || computer.activeAddress == null) {
            Toast.makeText(PcView.this, getResources().getString(R.string.error_pc_offline), Toast.LENGTH_SHORT).show();
            return;
        }
        if (managerBinder == null) {
            Toast.makeText(PcView.this, getResources().getString(R.string.error_manager_not_running), Toast.LENGTH_LONG).show();
            return;
        }

        final ComputerManagerService.ComputerManagerBinder binder = managerBinder;
        Toast.makeText(PcView.this, getResources().getString(R.string.unpairing), Toast.LENGTH_SHORT).show();
        new Thread(new Runnable() {
            @Override
            public void run() {
                NvHTTP httpConn;
                String message;
                try {
                    httpConn = new NvHTTP(ServerHelper.getCurrentAddressFromComputer(computer),
                            computer.httpsPort, binder.getUniqueId(), computer.serverCert,
                            PlatformBinding.getCryptoProvider(PcView.this));
                    if (httpConn.getPairState() == PairingManager.PairState.PAIRED) {
                        httpConn.unpair();
                        if (httpConn.getPairState() == PairingManager.PairState.NOT_PAIRED) {
                            message = getResources().getString(R.string.unpair_success);
                        }
                        else {
                            message = getResources().getString(R.string.unpair_fail);
                        }
                    }
                    else {
                        message = getResources().getString(R.string.unpair_error);
                    }
                } catch (UnknownHostException e) {
                    message = getResources().getString(R.string.error_unknown_host);
                } catch (FileNotFoundException e) {
                    message = getResources().getString(R.string.error_404);
                } catch (XmlPullParserException | IOException e) {
                    message = e.getMessage();
                    e.printStackTrace();
                }

                final String toastMessage = message;
                runOnUiThread(new Runnable() {
                    @Override
                    public void run() {
                        Toast.makeText(PcView.this, toastMessage, Toast.LENGTH_LONG).show();
                    }
                });
            }
        }).start();
    }

    private void doAppList(ComputerDetails computer, boolean newlyPaired, boolean showHiddenGames) {
        if (computer.state == ComputerDetails.State.OFFLINE) {
            Toast.makeText(PcView.this, getResources().getString(R.string.error_pc_offline), Toast.LENGTH_SHORT).show();
            return;
        }
        if (managerBinder == null) {
            Toast.makeText(PcView.this, getResources().getString(R.string.error_manager_not_running), Toast.LENGTH_LONG).show();
            return;
        }

        Intent i = new Intent(this, AppView.class);
        i.putExtra(AppView.NAME_EXTRA, computer.name);
        i.putExtra(AppView.UUID_EXTRA, computer.uuid);
        i.putExtra(AppView.NEW_PAIR_EXTRA, newlyPaired);
        i.putExtra(AppView.SHOW_HIDDEN_APPS_EXTRA, showHiddenGames);
        startActivity(i);
    }

    private boolean onComputerAction(ComputerObject computer, int action) {
        switch (action) {
            case PAIR_ID:
                doPair(computer.details);
                return true;

            case OTP_PAIR_ID:
                showOneTimePinDialog(computer.details);
                return true;

            case UNPAIR_ID:
                doUnpair(computer.details);
                return true;

            case WOL_ID:
                new MaterialAlertDialogBuilder(this).setTitle(R.string.pcview_menu_send_wol)
                        .setMessage(R.string.wol_help_text).setNegativeButton(android.R.string.cancel, null)
                        .setPositiveButton(R.string.pcview_menu_send_wol, (dialog, which) -> doWakeOnLan(computer.details)).show();
                return true;

            case DELETE_ID:
                if (ActivityManager.isUserAMonkey()) {
                    LimeLog.info("Ignoring delete PC request from monkey");
                    return true;
                }
                UiHelper.displayDeletePcConfirmationDialog(this, computer.details, new Runnable() {
                    @Override
                    public void run() {
                        if (managerBinder == null) {
                            Toast.makeText(PcView.this, getResources().getString(R.string.error_manager_not_running), Toast.LENGTH_LONG).show();
                            return;
                        }
                        removeComputer(computer.details);
                    }
                }, null);
                return true;

            case FULL_APP_LIST_ID:
                doAppList(computer.details, false, true);
                return true;

            case RESUME_ID:
                if (managerBinder == null) {
                    Toast.makeText(PcView.this, getResources().getString(R.string.error_manager_not_running), Toast.LENGTH_LONG).show();
                    return true;
                }

                ServerHelper.doStart(this, new NvApp("app", computer.details.runningGameId, false), computer.details, managerBinder);
                return true;

            case QUIT_ID:
                if (managerBinder == null) {
                    Toast.makeText(PcView.this, getResources().getString(R.string.error_manager_not_running), Toast.LENGTH_LONG).show();
                    return true;
                }

                // Display a confirmation dialog first
                UiHelper.displayQuitConfirmationDialog(this, new Runnable() {
                    @Override
                    public void run() {
                        ServerHelper.doQuit(PcView.this, computer.details,
                                new NvApp("app", 0, false), managerBinder, null);
                    }
                }, null);
                return true;

            case VIEW_DETAILS_ID:
                HostDetailsDialog.show(PcView.this, computer.details);
                return true;

            case HOST_SETTINGS_ID:
                HostStreamSettings.show(this, computer.details.uuid, computer.details.name);
                return true;

            case HOST_DISPLAY_ID:
                HostDisplaySettings.show(this, computer.details.uuid, computer.details.name);
                return true;

            case BANDWIDTH_PROBE_ID:
                if (managerBinder == null) {
                    return true;
                }
                if (managerBinder == null) {
                    Toast.makeText(this, R.string.error_manager_not_running, Toast.LENGTH_LONG).show();
                    return true;
                }
                if (bandwidthTest != null) {
                    bandwidthTest.cancel();
                }
                bandwidthTest = new PyroWaveBandwidthTest(this, computer.details, managerBinder.getUniqueId());
                bandwidthTest.show();
                return true;
            case GAMESTREAM_EOL_ID:
                HelpLauncher.launchGameStreamEolFaq(PcView.this);
                return true;

            case FRONTEND_EXPORT_ID:
                startFrontendExport(computer.details);
                return true;

            default:
                return false;
        }
    }
    
    private void removeComputer(ComputerDetails details) {
        managerBinder.removeComputer(details);

        new DiskAssetLoader(this).deleteAssetsForComputer(details.uuid);

        // Delete hidden games preference value
        getSharedPreferences(AppView.HIDDEN_APPS_PREF_FILENAME, MODE_PRIVATE)
                .edit()
                .remove(details.uuid)
                .apply();

        for (int i = 0; i < pcGridAdapter.getCount(); i++) {
            ComputerObject computer = (ComputerObject) pcGridAdapter.getItem(i);

            if (details.equals(computer.details)) {
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
            pcGridAdapter.addComputer(new ComputerObject(details));

            // Remove the "Discovery in progress" view
            updateEmptyState();
        }

        // Notify the view that the data has changed
        pcGridAdapter.notifyDataSetChanged();
    }

    @Override
    public int getAdapterFragmentLayoutId() {
        return R.layout.pc_grid_view;
    }

    @Override
    public void receiveAbsListView(AbsListView listView) {
        listView.setAdapter(pcGridAdapter);
        listView.setOnItemClickListener(new OnItemClickListener() {
            @Override
            public void onItemClick(AdapterView<?> arg0, View arg1, int pos,
                                    long id) {
                ComputerObject computer = (ComputerObject) pcGridAdapter.getItem(pos);
                if (computer.details.state == ComputerDetails.State.UNKNOWN ||
                    computer.details.state == ComputerDetails.State.OFFLINE) {
                    // Open the context menu if a PC is offline or refreshing
                    showComputerMenu(computer);
                } else if (computer.details.pairState != PairState.PAIRED) {
                    // Pair an unpaired machine by default
                    doPair(computer.details);
                } else {
                    doAppList(computer.details, false, false);
                }
            }
        });
        listView.setOnItemLongClickListener((parent, view, position, id) -> {
            showComputerMenu((ComputerObject) pcGridAdapter.getItem(position));
            return true;
        });
        if (pcGridAdapter.getCount() == 0) findViewById(R.id.discovery_add).requestFocus();
    }

    public static class ComputerObject {
        public ComputerDetails details;

        public ComputerObject(ComputerDetails details) {
            if (details == null) {
                throw new IllegalArgumentException("details must not be null");
            }
            this.details = details;
        }

        @Override
        public String toString() {
            return details.name;
        }
    }
}
