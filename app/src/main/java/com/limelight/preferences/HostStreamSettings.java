package com.limelight.preferences;

import android.app.Activity;
import android.app.Service;
import android.content.ActivityNotFoundException;
import android.content.ComponentName;
import android.content.Intent;
import android.content.ServiceConnection;
import android.content.pm.ShortcutManager;
import android.net.Uri;
import android.os.Build;
import android.os.Bundle;
import android.os.IBinder;
import android.preference.Preference;
import android.preference.PreferenceCategory;
import android.preference.PreferenceFragment;
import android.preference.PreferenceScreen;
import android.text.InputType;
import android.util.DisplayMetrics;
import android.view.LayoutInflater;
import android.view.View;
import android.view.ViewGroup;
import android.widget.CompoundButton;
import android.widget.EditText;
import android.widget.LinearLayout;
import android.widget.ListView;
import android.widget.Toast;
import android.widget.TextView;
import android.widget.ScrollView;

import androidx.appcompat.app.AlertDialog;
import com.limelight.ui.NightjarActivity;
import androidx.activity.OnBackPressedCallback;
import com.google.android.material.appbar.MaterialToolbar;
import com.limelight.ui.TopAppBar;
import com.google.android.material.checkbox.MaterialCheckBox;
import com.google.android.material.dialog.MaterialAlertDialogBuilder;
import com.limelight.ui.NavigationDialogBuilder;
import com.google.android.material.textfield.TextInputEditText;
import com.google.android.material.textfield.TextInputLayout;
import com.limelight.LimeLog;
import com.limelight.R;
import com.limelight.RemoteSetupActivity;
import com.limelight.binding.video.UpscalingPolicy;
import com.limelight.computers.ComputerManagerService;
import com.limelight.computers.HostActions;
import com.limelight.computers.HostCapabilityCache;
import com.limelight.computers.HostNames;
import com.limelight.computers.IdentityManager;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.http.PairingManager;
import com.limelight.nvstream.wol.WakeOnLanSender;
import com.limelight.ui.HostFeatureGate;
import com.google.android.material.slider.Slider;
import com.limelight.utils.FrontendExporter;
import com.limelight.utils.FrontendLauncher;
import com.limelight.utils.FrontendExportStore;
import com.limelight.utils.HelpLauncher;
import com.limelight.utils.HostDetailsDialog;
import com.limelight.utils.PyroWaveBandwidthTest;
import com.limelight.utils.PyroWavePaceCalibration;
import com.limelight.utils.PyroWavePaceStore;
import com.limelight.utils.ServerHelper;
import com.limelight.utils.ShortcutHelper;
import com.limelight.utils.UiHelper;

import java.io.IOException;

/**
 * One PC's settings, titled with its name: its own stream values, its display, its connection
 * and its host. Each stream value is either this PC's own or "Same as all PCs", which follows the
 * main Stream settings and the profile in use, so a change there reaches every PC that doesn't set
 * it. Save keeps the stream values; the other rows act at once.
 *
 * The same stream rows edit a named profile's values and one game's values (see {@link Mode});
 * a value not set there comes from the layers below it ({@link StreamLayers}).
 */
public class HostStreamSettings extends NightjarActivity {
    private static final int PICK_ROMS_REQUEST = 1;
    private static final int PICK_ESDE_REQUEST = 2;
    static final String EXTRA_MODE = "mode";
    static final String EXTRA_PROFILE_ID = "profile_id";
    static final String EXTRA_APP_UUID = "app_uuid";
    static final String EXTRA_APP_ID = "app_id";
    static final String EXTRA_APP_NAME = "app_name";

    /** Whose values this screen edits. */
    enum Mode { PC, PROFILE, GAME }

    private Mode mode = Mode.PC;
    private String profileId, appUuid, appName;
    private int appId;
    // The layers below the one edited here, and where each of their values comes from.
    private StreamLayers.Result below;
    // The values below the ones edited here ("all PCs" on a PC), and the values edited on top of them.
    private PreferenceConfiguration global;
    private HostStreamProfile overrides;
    private String hostUuid;
    // What this PC last showed it can do; rows it can never use are left out.
    private HostCapabilities caps = HostCapabilities.UNKNOWN;
    // The PC as the computer manager last saw it; only its name and saved capabilities until it answers.
    ComputerDetails computer;
    private boolean managerBound;
    // The computer manager once bound; Rename, Unpair and Remove go through it.
    private ComputerManagerService.ComputerManagerBinder manager;
    private boolean foreground;
    private final HostActions hostActions = new HostActions(this, new HostActions.Callbacks() {
        @Override public ComputerManagerService.ComputerManagerBinder binder() { return manager; }
        @Override public boolean isForeground() { return foreground; }
        @Override public void onPaired(ComputerDetails details) { }
        @Override public void onRemoved(ComputerDetails details) {
            // Nothing is left to set for a PC that is gone.
            finish();
        }
        @Override public void onRenamed(ComputerDetails details) {
            showName();
        }
        @Override public void onWakeSent(ComputerDetails details) { }
        @Override public void pausePolling() { }
        @Override public void resumePolling() { }
    });
    private PyroWaveBandwidthTest connectionTest;
    private PyroWavePaceCalibration paceCalibration;
    private Uri exportRomsTree;
    private boolean exporting;
    private boolean exportRunning;
    private FrontendLauncher exportLauncher = FrontendLauncher.ES_DE;
    private String savedProfile;
    private String editorKey;
    private AlertDialog editorDialog;
    private Bundle restoredEditor;
    private String restoredEditorKey;

    private final ServiceConnection managerConnection = new ServiceConnection() {
        @Override
        public void onServiceConnected(ComponentName name, IBinder binder) {
            if (!(binder instanceof ComputerManagerService.ComputerManagerBinder)) return;
            ComputerManagerService.ComputerManagerBinder bound = (ComputerManagerService.ComputerManagerBinder) binder;
            manager = bound;
            new Thread(() -> {
                if (!bound.waitForReady()) return;
                ComputerDetails known = bound.getComputer(hostUuid);
                if (known == null) return;
                ComputerDetails copy = new ComputerDetails(known);
                runOnUiThread(() -> {
                    if (!isFinishing() && !isDestroyed()) showComputer(copy);
                });
            }, "PC settings").start();
        }

        @Override
        public void onServiceDisconnected(ComponentName name) {
        }
    };

    public static void show(Activity activity, String hostUuid, String hostName) {
        activity.startActivity(new Intent(activity, HostStreamSettings.class)
                .putExtra("host_uuid", hostUuid).putExtra("host_name", hostName));
    }

    /** A named profile's values. */
    public static void showProfile(Activity activity, String profileId) {
        activity.startActivity(new Intent(activity, HostStreamSettings.class)
                .putExtra(EXTRA_MODE, Mode.PROFILE.name()).putExtra(EXTRA_PROFILE_ID, profileId));
    }

    /** One game's values on one PC. */
    public static void showGame(Activity activity, String hostUuid, String hostName, String appUuid, int appId,
                                String appName) {
        activity.startActivity(new Intent(activity, HostStreamSettings.class)
                .putExtra(EXTRA_MODE, Mode.GAME.name()).putExtra("host_uuid", hostUuid).putExtra("host_name", hostName)
                .putExtra(EXTRA_APP_UUID, appUuid).putExtra(EXTRA_APP_ID, appId).putExtra(EXTRA_APP_NAME, appName));
    }

    @Override
    protected void onCreate(Bundle state) {
        super.onCreate(state);
        UiHelper.setLocale(this);
        try {
            String requested = getIntent().getStringExtra(EXTRA_MODE);
            mode = requested == null ? Mode.PC : Mode.valueOf(requested);
        } catch (IllegalArgumentException e) {
            finish();
            return;
        }
        hostUuid = getIntent().getStringExtra("host_uuid");
        profileId = getIntent().getStringExtra(EXTRA_PROFILE_ID);
        appUuid = getIntent().getStringExtra(EXTRA_APP_UUID);
        appId = getIntent().getIntExtra(EXTRA_APP_ID, 0);
        appName = getIntent().getStringExtra(EXTRA_APP_NAME);
        StreamProfiles.Snapshot profiles = StreamProfiles.load(this);
        if (mode == Mode.PROFILE ? !profiles.exists(profileId) : hostUuid == null) {
            finish();
            return;
        }
        caps = mode == Mode.PROFILE ? HostCapabilities.UNKNOWN : HostCapabilityCache.load(this, hostUuid);
        computer = new ComputerDetails();
        computer.uuid = hostUuid;
        computer.name = getIntent().getStringExtra("host_name");
        computer.alias = HostNames.get(this, hostUuid);
        computer.caps = caps;
        PreferenceConfiguration.migratePcProfilesToOverrides(this);
        StreamLayers.Input input = PreferenceConfiguration.layerInput(this, PreferenceConfiguration.readPreferences(this),
                hostUuid, appUuid, appId);
        // A stream's mobile data limit is not a stored value; these rows show what is stored.
        input.mobileLimitKbps = 0;
        if (mode == Mode.PROFILE) {
            input.profiles = StreamProfiles.Snapshot.EMPTY;
        } else {
            StreamNetwork.fill(this, input);
        }
        if (mode != Mode.GAME) input.pcValues = null;
        input.gameValues = null;
        below = StreamLayers.resolve(input);
        global = baseConfig();
        overrides = loadOwn(profiles);
        savedProfile = overrides.serialize();
        if (state != null && state.getString("draft") != null) {
            try {
                overrides = HostStreamProfile.deserialize(state.getString("draft"));
            } catch (IllegalArgumentException ignored) {
                // Keep the saved values.
            }
            restoredEditorKey = state.getString("editor_key");
            restoredEditor = state.getBundle("editor_state");
            exporting = state.getBoolean("exporting");
            String launcher = state.getString("export_launcher");
            if (launcher != null) exportLauncher = FrontendLauncher.valueOf(launcher);
            String romsTree = state.getString("export_roms_tree");
            exportRomsTree = romsTree != null ? Uri.parse(romsTree) : null;
        }
        setContentView(R.layout.activity_host_stream_settings);
        // The PC is the page: its name is the title, the host software and version the subtitle.
        // A profile or a game names the page the same way.
        String title = mode == Mode.PROFILE ? profiles.name(profileId) : mode == Mode.GAME ? appName : computer.displayName();
        setTitle(title);
        MaterialToolbar toolbar = TopAppBar.configure(this, R.id.host_profile_toolbar, title, R.menu.host_profile);
        toolbar.setSubtitle(mode == Mode.PROFILE ? getString(R.string.profiles_subtitle) :
                mode == Mode.GAME ? getString(R.string.game_settings_subtitle, computer.displayName()) : caps.familyLabel(this));
        TopAppBar.setUpNavigation(toolbar, R.id.host_profile_back, v -> leaveSettings());
        getOnBackPressedDispatcher().addCallback(this, new OnBackPressedCallback(true) {
            @Override
            public void handleOnBackPressed() {
                leaveSettings();
            }
        });
        toolbar.setOnMenuItemClickListener(item -> {
            new NavigationDialogBuilder(this).setTitle(mode == Mode.PROFILE ? R.string.profiles_title :
                            mode == Mode.GAME ? R.string.game_settings_title : R.string.pc_settings_title)
                    .setMessage(mode == Mode.PROFILE ? R.string.profile_settings_explanation :
                            mode == Mode.GAME ? R.string.game_settings_explanation : R.string.host_profile_explanation)
                    .setPositiveButton(android.R.string.ok, null).show();
            return true;
        });
        findViewById(R.id.host_profile_save).setOnClickListener(v -> {
            saveOwn();
            Toast.makeText(this, mode != Mode.PC ? R.string.layers_saved :
                    overrides.isEmpty() ? R.string.host_profile_reset_done : R.string.host_profile_saved,
                    Toast.LENGTH_SHORT).show();
            finish();
        });
        updateSaveState();
        if (state == null) {
            getFragmentManager().beginTransaction().replace(R.id.host_profile_settings, new SettingsFragment()).commit();
        }
        UiHelper.notifyNewRootView(this);
        if (mode == Mode.PC) {
            managerBound = bindService(new Intent(this, ComputerManagerService.class), managerConnection, Service.BIND_AUTO_CREATE);
        }
    }

    /** The stored values edited here; unreadable ones are reported and start empty. */
    private HostStreamProfile loadOwn(StreamProfiles.Snapshot profiles) {
        String raw = mode == Mode.PROFILE ? profiles.rawValues(profileId) : mode == Mode.GAME ?
                AppStreamSettings.raw(this, hostUuid, appUuid, appId) : HostStreamProfile.raw(this, hostUuid);
        if (raw == null) return new HostStreamProfile();
        try {
            return HostStreamProfile.deserialize(raw);
        } catch (IllegalArgumentException e) {
            Toast.makeText(this, mode == Mode.PROFILE ?
                    getString(R.string.layers_problem_profile_invalid, profiles.name(profileId)) :
                    getString(mode == Mode.GAME ? R.string.layers_problem_game_invalid : R.string.layers_problem_pc_invalid),
                    Toast.LENGTH_LONG).show();
            return new HostStreamProfile();
        }
    }

    private void saveOwn() {
        switch (mode) {
            case PROFILE: StreamProfiles.saveValues(this, profileId, overrides); break;
            case GAME: AppStreamSettings.save(this, hostUuid, appUuid, appId, overrides); break;
            default: overrides.save(this, hostUuid); break;
        }
    }

    /** The main settings with the layers below this one on top: what a value not set here uses. */
    private PreferenceConfiguration baseConfig() {
        PreferenceConfiguration config = PreferenceConfiguration.readPreferences(this);
        applyWithScreen(below.values, config);
        return config;
    }

    /**
     * What the Quality preset sets: more than the PC's bitrate, or the main settings' when the PC has
     * none. A network profile is left out, so the preset reads the same on every network.
     */
    private int qualityBitrate() {
        PreferenceConfiguration main = PreferenceConfiguration.readPreferences(this);
        HostStreamProfile pc = null;
        String raw = HostStreamProfile.raw(this, hostUuid);
        if (raw != null) {
            try {
                pc = HostStreamProfile.deserialize(raw);
            } catch (IllegalArgumentException ignored) {
                // An unreadable PC layer is reported where it is edited; the main settings serve as base.
            }
        }
        return GameProfilePreset.qualityBitrate(GameProfilePreset.baseBitrate(main.bitrate, pc));
    }

    private void applyWithScreen(HostStreamProfile values, PreferenceConfiguration config) {
        boolean followedScreen = config.resolutionFollowsScreen;
        values.applyTo(config);
        if (config.resolutionFollowsScreen && !followedScreen) {
            String nativeResolution = PreferenceConfiguration.nativeResolution(this);
            if (nativeResolution != null) {
                config.width = PreferenceConfiguration.getWidthFromResolutionString(nativeResolution);
                config.height = PreferenceConfiguration.getHeightFromResolutionString(nativeResolution);
            }
        }
        if (config.vrr) config.framePacing = PreferenceConfiguration.FRAME_PACING_MIN_LATENCY;
    }

    @Override
    protected void onResume() {
        super.onResume();
        foreground = true;
    }

    @Override
    protected void onPause() {
        super.onPause();
        foreground = false;
        if (paceCalibration != null) {
            paceCalibration.cancel();
            paceCalibration = null;
        }
        if (connectionTest != null) {
            connectionTest.cancel();
            connectionTest = null;
        }
    }

    @Override
    protected void onDestroy() {
        super.onDestroy();
        hostActions.destroy();
        if (managerBound) {
            unbindService(managerConnection);
            managerBound = false;
        }
    }

    /** The PC as the computer manager knows it: its state, addresses and what its host reported last. */
    void showComputer(ComputerDetails details) {
        details.alias = HostNames.get(this, hostUuid);
        computer = details;
        showName();
        if (details.caps.isKnown()) {
            caps = details.caps;
            ((MaterialToolbar) findViewById(R.id.host_profile_toolbar)).setSubtitle(caps.familyLabel(this));
        }
        SettingsFragment fragment = (SettingsFragment) getFragmentManager().findFragmentById(R.id.host_profile_settings);
        if (fragment != null && fragment.activity != null) {
            fragment.showActions();
        }
    }

    /** The title is the name this device gives the PC: the alias while one is set, else the PC's own name. */
    private void showName() {
        if (mode == Mode.PROFILE) return;
        MaterialToolbar toolbar = findViewById(R.id.host_profile_toolbar);
        if (mode == Mode.GAME) {
            toolbar.setSubtitle(getString(R.string.game_settings_subtitle, computer.displayName()));
        } else {
            setTitle(computer.displayName());
            toolbar.setTitle(computer.displayName());
        }
    }

    private boolean isOnline() {
        return computer.state == ComputerDetails.State.ONLINE && computer.activeAddress != null;
    }

    private void startConnectionTest() {
        if (connectionTest != null) {
            connectionTest.cancel();
        }
        connectionTest = new PyroWaveBandwidthTest(this, computer, new IdentityManager(this).getUniqueId());
        connectionTest.show();
    }

    private void startPaceCalibration() {
        if (paceCalibration != null) {
            paceCalibration.cancel();
        }
        PreferenceConfiguration config = effective();
        paceCalibration = new PyroWavePaceCalibration(this, computer, new IdentityManager(this).getUniqueId(),
                config.bitrate, config.fps, () -> {
                    SettingsFragment fragment = (SettingsFragment) getFragmentManager().findFragmentById(R.id.host_profile_settings);
                    if (fragment != null && fragment.activity != null) {
                        fragment.showActions();
                    }
                });
        paceCalibration.show();
    }

    void showHostAdministration() {
        okhttp3.HttpUrl url = ServerHelper.getHostSettingsUrl(computer);
        if (url == null) {
            return;
        }
        AlertDialog dialog = new NavigationDialogBuilder(this)
                .setTitle(R.string.host_administration_menu)
                .setMessage(getString(R.string.host_administration_help, url.toString()))
                .setNegativeButton(android.R.string.cancel, null)
                .setPositiveButton(R.string.host_administration_open, (prompt, which) -> {
                    try {
                        startActivity(new Intent(Intent.ACTION_VIEW, Uri.parse(url.toString())));
                    } catch (ActivityNotFoundException | SecurityException e) {
                        Toast.makeText(this, R.string.host_administration_no_browser, Toast.LENGTH_LONG).show();
                    }
                }).show();
        ((TextView) dialog.findViewById(android.R.id.message)).setTextIsSelectable(true);
    }

    private void showWakeHelp() {
        MaterialAlertDialogBuilder dialog = new NavigationDialogBuilder(this).setTitle(R.string.wol_help_title)
                .setMessage(R.string.wol_help_text);
        if (computer.state != ComputerDetails.State.ONLINE && computer.macAddress != null) {
            dialog.setNegativeButton(android.R.string.cancel, null)
                    .setPositiveButton(R.string.pcview_menu_send_wol, (prompt, which) -> wake());
        } else {
            dialog.setPositiveButton(android.R.string.ok, null);
        }
        UiHelper.showDialog(this, dialog.create());
    }

    private void wake() {
        ComputerDetails target = computer;
        new Thread(() -> {
            boolean sent;
            try {
                WakeOnLanSender.sendWolPacket(target);
                sent = true;
            } catch (IOException e) {
                sent = false;
            }
            boolean woken = sent;
            runOnUiThread(() -> Toast.makeText(this, woken ? R.string.wol_waking_msg : R.string.wol_fail,
                    Toast.LENGTH_LONG).show());
        }, "Wake on LAN").start();
    }

    private void pinShortcut() {
        if (!new ShortcutHelper(this).createPinnedPcShortcut(computer)) {
            Toast.makeText(this, R.string.unable_to_pin_shortcut, Toast.LENGTH_LONG).show();
        }
    }

    private boolean canPinShortcuts() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) {
            return false;
        }
        ShortcutManager shortcuts = getSystemService(ShortcutManager.class);
        return shortcuts != null && shortcuts.isRequestPinShortcutSupported();
    }

    private void startFrontendExport() {
        if (exportRunning) return;
        FrontendLauncher[] launchers = FrontendLauncher.values();
        String[] labels = new String[launchers.length];
        for (int i = 0; i < labels.length; i++) labels[i] = launchers[i].label;
        new NavigationDialogBuilder(this)
                .setTitle(R.string.frontend_export_menu)
                .setItems(labels, (dialog, which) -> {
                    exportLauncher = launchers[which];
                    showFrontendActions();
                }).show();
    }

    private FrontendExportStore frontendStore() {
        return new FrontendExportStore(this, computer.uuid, exportLauncher);
    }

    private void showFrontendActions() {
        FrontendExportStore store = frontendStore();
        if (store.romsTree() == null) {
            chooseFrontendFolder();
            return;
        }
        new NavigationDialogBuilder(this)
                .setTitle(exportLauncher.label)
                .setItems(new String[] {getString(R.string.frontend_export_update),
                        getString(R.string.frontend_export_pick), getString(R.string.frontend_setup_title)},
                        (dialog, which) -> {
                            if (which == 0) {
                                exportRomsTree = store.romsTree();
                                runFrontendExport(store.esdeTree());
                            } else if (which == 1) chooseFrontendFolder();
                            else showFrontendSetup(null);
                        }).show();
    }

    private void chooseFrontendFolder() {
        if (new FrontendExporter(this, computer).loadApps().isEmpty()) {
            Toast.makeText(this, R.string.frontend_export_no_apps, Toast.LENGTH_LONG).show();
            return;
        }
        exporting = true;
        exportRomsTree = null;
        new NavigationDialogBuilder(this)
                .setTitle(R.string.frontend_export_roms_title)
                .setMessage(R.string.frontend_export_roms_message)
                .setPositiveButton(R.string.frontend_export_pick, (dialog, which) -> pickFolder(PICK_ROMS_REQUEST))
                .setNegativeButton(android.R.string.cancel, (dialog, which) -> exporting = false)
                .setOnCancelListener(dialog -> exporting = false)
                .show();
    }

    private void pickFolder(int request) {
        try {
            startActivityForResult(new Intent(Intent.ACTION_OPEN_DOCUMENT_TREE).addFlags(
                    Intent.FLAG_GRANT_READ_URI_PERMISSION | Intent.FLAG_GRANT_WRITE_URI_PERMISSION |
                            Intent.FLAG_GRANT_PERSISTABLE_URI_PERMISSION), request);
        } catch (ActivityNotFoundException e) {
            exporting = false;
            Toast.makeText(this, R.string.frontend_export_no_picker, Toast.LENGTH_LONG).show();
        }
    }

    @Override
    protected void onActivityResult(int requestCode, int resultCode, Intent data) {
        super.onActivityResult(requestCode, resultCode, data);
        Uri picked = resultCode == RESULT_OK && data != null ? data.getData() : null;
        if (!exporting) {
            return;
        }
        if (requestCode != PICK_ROMS_REQUEST && requestCode != PICK_ESDE_REQUEST) return;
        if (picked != null) {
            try {
                int access = Intent.FLAG_GRANT_READ_URI_PERMISSION | Intent.FLAG_GRANT_WRITE_URI_PERMISSION;
                if ((data.getFlags() & access) == access) {
                    getContentResolver().takePersistableUriPermission(picked, access);
                }
            } catch (SecurityException e) {
                // Some providers only grant this visit. The export still works; a later update
                // explains the lost access and lets the user pick the folder again.
                LimeLog.info("Frontend folder has no persistent access");
            }
        }
        if (requestCode == PICK_ROMS_REQUEST) {
            if (picked == null) {
                exporting = false;
                return;
            }
            exportRomsTree = picked;
            if (exportLauncher != FrontendLauncher.ES_DE) {
                runFrontendExport(null);
                return;
            }
            new NavigationDialogBuilder(this)
                    .setTitle(R.string.frontend_export_esde_title)
                    .setMessage(getString(R.string.frontend_export_esde_message, getString(R.string.app_name)))
                    .setPositiveButton(R.string.frontend_export_pick, (dialog, which) -> pickFolder(PICK_ESDE_REQUEST))
                    .setNegativeButton(R.string.frontend_export_skip, (dialog, which) -> runFrontendExport(null))
                    .setOnCancelListener(dialog -> runFrontendExport(null))
                    .show();
        } else if (requestCode == PICK_ESDE_REQUEST && exportRomsTree != null) {
            runFrontendExport(picked);
        }
    }

    private void runFrontendExport(final Uri esdeTree) {
        if (exportRunning) return;
        final ComputerDetails target = computer;
        final Uri romsTree = exportRomsTree;
        final FrontendLauncher launcher = exportLauncher;
        final FrontendExportStore store = frontendStore();
        exporting = false;
        exportRomsTree = null;
        if (romsTree == null) {
            return;
        }
        exportRunning = true;
        Toast.makeText(this, R.string.frontend_export_working, Toast.LENGTH_SHORT).show();
        new Thread(() -> {
            String message;
            boolean succeeded = false;
            try {
                FrontendExporter exporter = new FrontendExporter(HostStreamSettings.this, target);
                java.util.List<com.limelight.nvstream.http.NvApp> apps = exporter.loadApps();
                if (apps.isEmpty()) message = getString(R.string.frontend_export_no_apps);
                else {
                    int count = exporter.export(apps, romsTree, esdeTree);
                    store.save(romsTree, esdeTree);
                    succeeded = true;
                    message = getResources().getQuantityString(R.plurals.frontend_export_done, count, count);
                }
            } catch (IOException | RuntimeException e) {
                LimeLog.warning("Frontend export failed: " + e);
                message = getString(R.string.frontend_export_failed);
            }
            final String result = message;
            final boolean success = succeeded;
            runOnUiThread(() -> {
                exportRunning = false;
                if (isFinishing() || isDestroyed()) return;
                exportLauncher = launcher;
                if (success) showFrontendSetup(result);
                else new NavigationDialogBuilder(this).setTitle(launcher.label).setMessage(result)
                        .setPositiveButton(R.string.frontend_export_pick, (dialog, which) -> chooseFrontendFolder())
                        .setNegativeButton(android.R.string.cancel, null).show();
            });
        }, "Frontend export").start();
    }

    private void showFrontendSetup(String result) {
        String command = exportLauncher.command(getPackageName());
        String message = (result == null ? "" : result + "\n\n") +
                getString(exportLauncher.instructions, getString(R.string.app_name));
        if (command != null) message += "\n\n" + command;
        MaterialAlertDialogBuilder dialog = new NavigationDialogBuilder(this)
                .setTitle(exportLauncher.label).setMessage(message)
                .setPositiveButton(android.R.string.ok, null);
        if (command != null) {
            dialog.setNeutralButton(R.string.frontend_setup_copy, (ignored, which) -> {
                android.content.ClipboardManager clipboard = (android.content.ClipboardManager)
                        getSystemService(CLIPBOARD_SERVICE);
                clipboard.setPrimaryClip(android.content.ClipData.newPlainText(exportLauncher.label, command));
            });
        }
        dialog.show();
    }

    private void leaveSettings() {
        if (savedProfile.equals(overrides.serialize())) {
            finish();
            return;
        }
        UiHelper.showDialog(this, new NavigationDialogBuilder(this).setTitle(R.string.host_profile_unsaved_title)
                .setMessage(mode == Mode.PC ? R.string.host_profile_unsaved_message : R.string.layers_unsaved_message)
                .setPositiveButton(R.string.host_profile_save, (dialog, which) -> {
                    saveOwn();
                    finish();
                })
                .setNegativeButton(R.string.host_profile_discard, (dialog, which) -> finish())
                .setNeutralButton(android.R.string.cancel, null).create());
    }

    private void updateSaveState() {
        boolean changed = !savedProfile.equals(overrides.serialize());
        findViewById(R.id.host_profile_save).setEnabled(changed);
        ((TextView) findViewById(R.id.host_profile_status)).setText(mode != Mode.PC ?
                (changed ? R.string.layers_unsaved_context : R.string.layers_saved_context) : changed ?
                R.string.settings_profile_unsaved_context : R.string.settings_profile_saved_context);
    }

    /** What the next stream uses: the layers below with the values edited here on top. */
    private PreferenceConfiguration effective() {
        PreferenceConfiguration config = baseConfig();
        applyWithScreen(overrides, config);
        return config;
    }

    @Override
    protected void onSaveInstanceState(Bundle state) {
        super.onSaveInstanceState(state);
        if (overrides != null) state.putString("draft", overrides.serialize());
        state.putBoolean("exporting", exporting);
        state.putString("export_launcher", exportLauncher.name());
        if (exportRomsTree != null) state.putString("export_roms_tree", exportRomsTree.toString());
        if (editorDialog != null && editorDialog.isShowing()) {
            state.putString("editor_key", editorKey);
            state.putBundle("editor_state", editorDialog.onSaveInstanceState());
        }
    }

    public static class SettingsFragment extends PreferenceFragment {
        private HostStreamSettings activity;
        private PreferenceCategory connection;
        private PreferenceCategory host;

        @Override
        public void onActivityCreated(Bundle state) {
            super.onActivityCreated(state);
            activity = (HostStreamSettings) getActivity();
            PreferenceScreen screen = getPreferenceManager().createPreferenceScreen(activity);
            setPreferenceScreen(screen);
            PreferenceCategory stream = category(screen, R.string.settings_section_video);
            if (activity.mode == Mode.GAME) {
                row(stream, "preset", R.string.game_preset_title, this::showPreset);
            }
            row(stream, "resolution", R.string.title_resolution_list, this::showResolution);
            row(stream, "refresh", R.string.title_fps_list, () -> showNumber("refresh", R.string.host_profile_refresh));
            row(stream, "bitrate", R.string.title_seekbar_bitrate, () -> showNumber("bitrate", R.string.host_profile_bitrate));
            row(stream, "codec", R.string.title_video_format, this::showCodec);
            row(stream, "upscaling", R.string.title_upscaling, this::showUpscaling);
            row(stream, "sharpness", R.string.title_upscaling_sharpness, this::showSharpness);
            row(stream, "hdr", R.string.title_enable_hdr, () -> showSwitch("hdr", R.string.title_enable_hdr));
            row(stream, "vrr", R.string.title_vrr, () -> showSwitch("vrr", R.string.title_vrr));
            row(stream, "pacing", R.string.title_frame_pacing, this::showPacing);
            row(stream, "yuv444", R.string.title_yuv444, () -> showSwitch("yuv444", R.string.title_yuv444));
            row(stream, "full_range", R.string.title_full_range, () -> showSwitch("full_range", R.string.title_full_range));
            boolean pc = activity.mode == Mode.PC;
            int reset = pc ? R.string.host_profile_reset : R.string.layers_reset;
            row(stream, "reset", reset, () -> UiHelper.showDialog(activity,
                    new NavigationDialogBuilder(activity).setTitle(reset)
                            .setMessage(pc ? R.string.settings_profile_reset_message : R.string.layers_reset_message)
                            .setPositiveButton(reset, (dialog, which) -> {
                                activity.overrides = new HostStreamProfile();
                                updateSummaries();
                            }).setNegativeButton(android.R.string.cancel, null).create()));

            // The picker for hosts with display modes, a virtual display switch for hosts with one;
            // neither for hosts that can never use them.
            // A profile serves every PC, so it leaves host displays alone; a game has no display mode of its own.
            boolean displayModes = pc && gate(HostCapabilities.Feature.HOST_DISPLAY_MODES).isVisible();
            boolean virtualDisplay = activity.mode != Mode.PROFILE && gate(HostCapabilities.Feature.VIRTUAL_DISPLAY).isVisible();
            if (displayModes || virtualDisplay) {
                PreferenceCategory display = category(screen, R.string.pc_settings_display);
                if (displayModes) {
                    // Saved separately, so the display choice also applies when profile edits are discarded.
                    row(display, "host_display", R.string.host_display_menu, this::showDisplay);
                }
                if (virtualDisplay) {
                    row(display, "virtual", R.string.title_virtual_display, () -> showSwitch("virtual", R.string.title_virtual_display));
                    row(display, "scale", R.string.title_virtual_display_scale, () -> showNumber("scale", R.string.host_profile_scale));
                }
            }
            Preference footer = new Preference(activity);
            footer.setLayoutResource(R.layout.settings_preference);
            footer.setSummary(pc ? R.string.host_profile_footer : R.string.layers_footer);
            footer.setSelectable(false);
            screen.addPreference(footer);

            if (pc) {
                connection = category(screen, R.string.pc_settings_connection);
                host = category(screen, R.string.pc_settings_host);
                showActions();
            }
            updateSummaries();
        }

        private PreferenceCategory category(PreferenceScreen screen, int title) {
            PreferenceCategory category = new PreferenceCategory(activity);
            category.setTitle(title);
            category.setLayoutResource(R.layout.settings_category);
            screen.addPreference(category);
            return category;
        }

        private Preference row(PreferenceCategory category, String key, int title, Runnable onClick) {
            Preference preference = new Preference(activity);
            preference.setKey(key);
            preference.setTitle(title);
            preference.setLayoutResource(R.layout.settings_preference);
            preference.setPersistent(false);
            preference.setOnPreferenceClickListener(pref -> {
                onClick.run();
                return true;
            });
            category.addPreference(preference);
            return preference;
        }

        /**
         * The connection and host rows, which depend on whether the PC answers. They act at once;
         * Save only concerns the stream values above them.
         */
        void showActions() {
            if (connection == null) return;
            HostStreamSettings screen = activity;
            ComputerDetails pc = screen.computer;
            boolean online = screen.isOnline();
            boolean unpaired = pc.pairState == PairingManager.PairState.NOT_PAIRED;
            String needsOnline = getString(R.string.pc_settings_needs_online);

            connection.removeAll();
            Preference test = row(connection, "connection_test", R.string.connection_test_title, screen::startConnectionTest);
            test.setSummary(getString(pc.caps.mayHave(HostCapabilities.Feature.PYROWAVE_BANDWIDTH_PROBE) ?
                    R.string.pc_settings_test_speed : R.string.pc_settings_test_delay) + (online ? "" : "\n" + needsOnline));
            test.setEnabled(online);
            PyroWavePaceStore.Calibration pace = PyroWavePaceStore.get(screen, pc.uuid);
            if (pc.supportsPyroWavePaceCalibration() || pace != null) {
                Preference calibrate = row(connection, "pace_calibration", R.string.pace_calibration_title,
                        screen::startPaceCalibration);
                calibrate.setSummary((pace != null ? getString(R.string.pace_calibration_saved, pace.paceMbps) :
                        getString(R.string.pace_calibration_summary)) + (online ? "" : "\n" + needsOnline));
                calibrate.setEnabled(online);
            }
            row(connection, "remote_setup", R.string.remote_setup_title, () -> startActivity(
                    new Intent(screen, RemoteSetupActivity.class).putExtra(RemoteSetupActivity.EXTRA_PC_UUID, screen.hostUuid)))
                    .setSummary(R.string.pc_settings_remote_summary);
            row(connection, "wake", R.string.pc_settings_wake, screen::showWakeHelp).setSummary(pc.macAddress != null ?
                    R.string.pc_settings_wake_ready : R.string.pc_settings_wake_unknown);

            host.removeAll();
            HostFeatureGate webUi = gate(HostCapabilities.Feature.WEB_UI);
            if (webUi.isVisible()) {
                Preference web = row(host, "web_ui", R.string.host_administration_menu, screen::showHostAdministration);
                okhttp3.HttpUrl url = ServerHelper.getHostSettingsUrl(pc);
                web.setSummary(url != null ? url.toString() : needsOnline);
                web.setEnabled(url != null);
                applyGate(web, HostCapabilities.Feature.WEB_UI);
            }
            row(host, "details", R.string.pcview_menu_details, () -> HostDetailsDialog.show(screen, screen.computer))
                    .setSummary(R.string.pc_settings_details_summary);
            if (!unpaired) {
                row(host, "frontend_export", R.string.frontend_export_menu, screen::startFrontendExport)
                        .setSummary(R.string.pc_settings_export_summary);
                if (screen.canPinShortcuts()) {
                    row(host, "shortcut", R.string.pc_settings_shortcut, screen::pinShortcut)
                            .setSummary(R.string.pc_settings_shortcut_summary);
                }
            }
            if (pc.caps.family() == HostCapabilities.Family.GAMESTREAM) {
                row(host, "gamestream_eol", R.string.pcview_menu_eol, () -> HelpLauncher.launchGameStreamEolFaq(screen));
            }
            // The same three actions as the PC's options on the list and on Home.
            row(host, "rename", R.string.host_menu_rename, () -> screen.hostActions.rename(screen.computer));
            if (online && pc.pairState == PairingManager.PairState.PAIRED) {
                row(host, "unpair", R.string.pcview_menu_unpair_pc, () -> screen.hostActions.confirmUnpair(screen.computer));
            }
            row(host, "remove", R.string.pcview_menu_delete_pc, () -> screen.hostActions.confirmRemove(screen.computer));
        }

        private String onOff(boolean value) {
            return getString(value ? R.string.host_profile_on : R.string.host_profile_off);
        }

        /** The value, marked with where it comes from when it is not set here. */
        private String summary(HostStreamProfile.Field field, boolean own, String value) {
            if (own) return value;
            StreamLayers.Source source = activity.below.source(field);
            if (activity.mode == Mode.PC && source == StreamLayers.Source.MAIN) {
                return getString(R.string.host_profile_follows, value);
            }
            return getString(R.string.layers_value_from, value, StreamLayersText.source(activity, activity.below, source));
        }

        /** The "not set here" choice, with the value it then has. */
        private String sameAs(String value) {
            return getString(activity.mode == Mode.PC ? R.string.host_profile_same_as_all_value :
                    R.string.layers_not_set_value, value);
        }

        private static HostStreamProfile.Field switchField(String key) {
            switch (key) {
                case "hdr": return HostStreamProfile.Field.HDR;
                case "vrr": return HostStreamProfile.Field.VRR;
                case "virtual": return HostStreamProfile.Field.VIRTUAL_DISPLAY;
                case "yuv444": return HostStreamProfile.Field.YUV444;
                default: return HostStreamProfile.Field.FULL_RANGE;
            }
        }

        private String pacingText(PreferenceConfiguration config) {
            String[] names = getResources().getStringArray(R.array.video_frame_pacing_names);
            return names[Math.max(0, Math.min(names.length - 1, config.framePacing))];
        }

        private String resolutionText(PreferenceConfiguration config) {
            return config.resolutionFollowsScreen ? getString(R.string.host_profile_native) :
                    config.width + " × " + config.height;
        }

        private String refreshText(PreferenceConfiguration config) {
            return HostStreamProfile.formatRefreshRate(config.launchRefreshRateX100 == 0 ?
                    config.fps * 100 : config.launchRefreshRateX100) + " " + getString(R.string.fps_suffix_fps);
        }

        private String upscalingText(PreferenceConfiguration config) {
            String upscaling = getResources().getStringArray(R.array.upscaling_names)[config.upscalingMode.ordinal()];
            if (config.upscalingMode != UpscalingPolicy.Mode.OFF) {
                if (config.enableHdr) upscaling += "\n" + getString(R.string.upscaling_hdr);
                else if (config.videoFormat == PreferenceConfiguration.FormatOption.FORCE_PYROWAVE) {
                    upscaling += "\n" + getString(R.string.upscaling_pyrowave);
                }
            }
            return upscaling;
        }

        private boolean switchValue(PreferenceConfiguration config, String key) {
            switch (key) {
                case "hdr": return config.enableHdr;
                case "vrr": return config.vrr;
                case "virtual": return config.virtualDisplay;
                case "yuv444": return config.enableYuv444;
                default: return config.fullRange;
            }
        }

        private Boolean switchOverride(String key) {
            HostStreamProfile own = activity.overrides;
            switch (key) {
                case "hdr": return own.hdr;
                case "vrr": return own.vrr;
                case "virtual": return own.virtualDisplay;
                case "yuv444": return own.yuv444;
                default: return own.fullRange;
            }
        }

        private void setSwitchOverride(String key, Boolean value) {
            HostStreamProfile own = activity.overrides;
            switch (key) {
                case "hdr": own.hdr = value; break;
                case "vrr": own.vrr = value; break;
                case "virtual": own.virtualDisplay = value; break;
                case "yuv444": own.yuv444 = value; break;
                default: own.fullRange = value; break;
            }
        }

        private void updateSummaries() {
            PreferenceConfiguration config = activity.effective();
            HostStreamProfile own = activity.overrides;
            findPreference("resolution").setSummary(summary(HostStreamProfile.Field.RESOLUTION, own.nativeResolution != null, resolutionText(config)) +
                    "\n" + getString(R.string.summary_resolution_list));
            findPreference("refresh").setSummary(summary(HostStreamProfile.Field.REFRESH, own.refreshRateX100 != null, refreshText(config)));
            findPreference("bitrate").setSummary(summary(HostStreamProfile.Field.BITRATE, own.bitrate != null,
                    HostStreamProfile.formatBitrateMbps(config.bitrate) + " Mbps"));
            String[] codecs = getResources().getStringArray(R.array.host_profile_codecs);
            String codec = summary(HostStreamProfile.Field.CODEC, own.codec != null, codecs[config.videoFormat.ordinal()]);
            if (config.videoFormat == PreferenceConfiguration.FormatOption.FORCE_PYROWAVE &&
                    !activity.caps.mayHave(HostCapabilities.Feature.PYROWAVE)) {
                codec += "\n" + getString(R.string.host_pyrowave_unavailable,
                        codecs[PreferenceConfiguration.codecWithoutPyroWave(activity.global.videoFormat).ordinal()]);
            }
            findPreference("codec").setSummary(codec);
            findPreference("upscaling").setSummary(summary(HostStreamProfile.Field.UPSCALING, own.upscalingMode != null, upscalingText(config)));
            findPreference("sharpness").setSummary(summary(HostStreamProfile.Field.SHARPNESS, own.upscalingSharpness != null,
                    getString(R.string.upscaling_sharpness_value, config.upscalingSharpness)));
            findPreference("sharpness").setEnabled(config.upscalingMode == UpscalingPolicy.Mode.FSR1 ||
                    config.upscalingMode == UpscalingPolicy.Mode.SGSR1);
            for (String key : new String[] {"hdr", "vrr", "yuv444", "full_range"}) {
                findPreference(key).setSummary(summary(switchField(key), switchOverride(key) != null, onOff(switchValue(config, key))));
            }
            String pacing = summary(HostStreamProfile.Field.PACING, own.pacing != null, pacingText(config));
            findPreference("pacing").setSummary(config.vrr ? pacing + "\n" + getString(R.string.layers_pacing_vrr) : pacing);
            findPreference("vrr").setSummary(findPreference("vrr").getSummary() +
                    "\n" + getString(R.string.summary_vrr));
            Preference virtual = findPreference("virtual");
            Preference scale = findPreference("scale");
            if (virtual != null) {
                virtual.setSummary(summary(HostStreamProfile.Field.VIRTUAL_DISPLAY, switchOverride("virtual") != null, onOff(switchValue(config, "virtual"))));
                scale.setSummary(summary(HostStreamProfile.Field.RENDER_SCALE, own.renderScale != null, config.virtualDisplayScale + "%") +
                        "\n" + getString(R.string.summary_virtual_display_scale));
            }
            Preference preset = findPreference("preset");
            if (preset != null) {
                GameProfilePreset current = GameProfilePreset.of(own, activity.qualityBitrate());
                preset.setSummary(getString(current.title) + "\n" + getString(current.summary));
            }
            findPreference("reset").setEnabled(!own.isEmpty());
            boolean pc = activity.mode == Mode.PC;
            findPreference("reset").setSummary(own.isEmpty() ?
                    getString(pc ? R.string.host_profile_status_global : R.string.layers_status_empty) :
                    getString(pc ? R.string.host_profile_status_custom : R.string.layers_status_custom));
            Preference display = findPreference("host_display");
            // Only a host with display modes receives the choice; elsewhere the switch decides.
            HostDisplayChoice displayChoice = pc && activity.caps.mayHave(HostCapabilities.Feature.HOST_DISPLAY_MODES) ?
                    HostDisplayChoice.load(activity, activity.hostUuid) : HostDisplayChoice.HOST_DEFAULT;
            if (virtual != null) {
                virtual.setEnabled(displayChoice == HostDisplayChoice.HOST_DEFAULT);
                if (displayChoice != HostDisplayChoice.HOST_DEFAULT) {
                    virtual.setSummary(getString(R.string.host_display_controls_virtual, getString(displayChoice.label)));
                }
                applyGate(virtual, HostCapabilities.Feature.VIRTUAL_DISPLAY);
                applyGate(scale, HostCapabilities.Feature.VIRTUAL_DISPLAY);
            }
            if (display != null) {
                display.setSummary(getString(displayChoice.label) + "\n" + getString(R.string.settings_profile_display_immediate));
                applyGate(display, HostCapabilities.Feature.HOST_DISPLAY_MODES);
            }
            activity.updateSaveState();
        }

        private HostFeatureGate gate(HostCapabilities.Feature feature) {
            return HostFeatureGate.of(activity.caps, feature);
        }

        /** Disables a row the host can't use yet and adds why, or notes that it applies if supported. */
        private void applyGate(Preference row, HostCapabilities.Feature feature) {
            HostFeatureGate gate = gate(feature);
            if (!gate.isEnabled()) {
                row.setEnabled(false);
            }
            row.setSummary(gate.withNote(activity, row.getSummary()));
        }

        private void showSwitch(String key, int title) {
            boolean allPcs = switchValue(activity.global, key);
            Boolean own = switchOverride(key);
            CharSequence[] items = {
                    sameAs(onOff(allPcs)),
                    getString(R.string.host_profile_on),
                    getString(R.string.host_profile_off)};
            int checked = own == null ? 0 : own ? 1 : 2;
            new NavigationDialogBuilder(activity).setTitle(title)
                    .setSingleChoiceItems(items, checked, (dialog, which) -> {
                        setSwitchOverride(key, which == 0 ? null : which == 1);
                        updateSummaries();
                        dialog.dismiss();
                    }).setNegativeButton(android.R.string.cancel, null).show();
        }

        /** Competitive, Smoothness and Quality each set only the values they name; Custom changes nothing. */
        private void showPreset() {
            GameProfilePreset[] presets = GameProfilePreset.values();
            int quality = activity.qualityBitrate();
            CharSequence[] items = new CharSequence[presets.length];
            for (int i = 0; i < presets.length; i++) {
                items[i] = getString(presets[i].title) + "\n" + getString(presets[i].summary);
            }
            new NavigationDialogBuilder(activity).setTitle(R.string.game_preset_title)
                    .setSingleChoiceItems(items, GameProfilePreset.of(activity.overrides, quality).ordinal(),
                            (dialog, which) -> {
                                presets[which].apply(activity.overrides, quality);
                                updateSummaries();
                                dialog.dismiss();
                            }).setNegativeButton(android.R.string.cancel, null).show();
        }

        private void showPacing() {
            String[] names = getResources().getStringArray(R.array.video_frame_pacing_names);
            String[] values = getResources().getStringArray(R.array.video_frame_pacing_values);
            CharSequence[] items = new CharSequence[names.length + 1];
            items[0] = sameAs(pacingText(activity.global));
            System.arraycopy(names, 0, items, 1, names.length);
            String own = activity.overrides.pacing;
            int checked = own == null ? 0 : java.util.Arrays.asList(values).indexOf(own) + 1;
            new NavigationDialogBuilder(activity).setTitle(R.string.title_frame_pacing)
                    .setSingleChoiceItems(items, checked, (dialog, which) -> {
                        activity.overrides.pacing = which == 0 ? null : values[which - 1];
                        updateSummaries();
                        dialog.dismiss();
                    }).setNegativeButton(android.R.string.cancel, null).show();
        }

        private void showCodec() {
            String[] codecs = getResources().getStringArray(R.array.host_profile_codecs);
            // PyroWave is the last codec; a host that can never stream it doesn't offer it.
            int offered = gate(HostCapabilities.Feature.PYROWAVE).isVisible() ? codecs.length :
                    PreferenceConfiguration.FormatOption.FORCE_PYROWAVE.ordinal();
            CharSequence[] items = new CharSequence[offered + 1];
            items[0] = sameAs(codecs[activity.global.videoFormat.ordinal()]);
            System.arraycopy(codecs, 0, items, 1, offered);
            PreferenceConfiguration.FormatOption own = activity.overrides.codec;
            int checked = own == null ? 0 : own.ordinal() < offered ? own.ordinal() + 1 : -1;
            new NavigationDialogBuilder(activity).setTitle(R.string.title_video_format)
                    .setSingleChoiceItems(items, checked, (dialog, which) -> {
                        activity.overrides.codec = which == 0 ? null :
                                PreferenceConfiguration.FormatOption.values()[which - 1];
                        updateSummaries();
                        dialog.dismiss();
                    }).setNegativeButton(android.R.string.cancel, null).show();
        }

        private void showUpscaling() {
            String[] modes = getResources().getStringArray(R.array.upscaling_names);
            CharSequence[] items = new CharSequence[modes.length + 1];
            items[0] = sameAs(modes[activity.global.upscalingMode.ordinal()]);
            System.arraycopy(modes, 0, items, 1, modes.length);
            UpscalingPolicy.Mode own = activity.overrides.upscalingMode;
            AlertDialog picker = new NavigationDialogBuilder(activity).setTitle(R.string.title_upscaling)
                    .setSingleChoiceItems(items, own == null ? 0 : own.ordinal() + 1, (dialog, which) -> {
                        activity.overrides.upscalingMode = which == 0 ? null : UpscalingPolicy.Mode.values()[which - 1];
                        updateSummaries();
                        dialog.dismiss();
                    }).setNeutralButton(R.string.help, null)
                    .setNegativeButton(android.R.string.cancel, null).create();
            UiHelper.showDialog(activity, picker);
            picker.getButton(AlertDialog.BUTTON_NEUTRAL).setOnClickListener(view ->
                    UiHelper.showDialog(activity, new NavigationDialogBuilder(activity).setTitle(R.string.title_upscaling)
                            .setMessage(R.string.summary_upscaling).setPositiveButton(android.R.string.ok, null).create()));
        }

        private LinearLayout form() {
            LinearLayout form = new LinearLayout(activity);
            form.setOrientation(LinearLayout.VERTICAL);
            int padding = Math.round(24 * getResources().getDisplayMetrics().density);
            form.setPadding(padding, padding / 3, padding, 0);
            return form;
        }

        private ScrollView scrollable(LinearLayout form) {
            ScrollView scroll = new ScrollView(activity);
            scroll.setFillViewport(true);
            scroll.setClipToPadding(false);
            scroll.addView(form);
            return scroll;
        }

        private void showDisplay() {
            AlertDialog picker = HostDisplaySettings.create(activity, activity.hostUuid,
                    activity.computer.displayName(), this::updateSummaries);
            showEditor("display", picker);
            HostDisplaySettings.bindHelp(activity, picker);
        }

        private void showEditor(String key, AlertDialog dialog) {
            activity.editorKey = key;
            activity.editorDialog = dialog;
            UiHelper.showDialog(activity, dialog);
            if (key.equals(activity.restoredEditorKey) && activity.restoredEditor != null) {
                dialog.onRestoreInstanceState(activity.restoredEditor);
                activity.restoredEditor = null;
                activity.restoredEditorKey = null;
            }
        }

        @Override
        public void onResume() {
            super.onResume();
            String key = activity.restoredEditorKey;
            if (key == null) return;
            switch (key) {
                case "display": showDisplay(); break;
                case "resolution": showResolution(); break;
                case "sharpness": showSharpness(); break;
                case "refresh": showNumber(key, R.string.host_profile_refresh); break;
                case "bitrate": showNumber(key, R.string.host_profile_bitrate); break;
                case "scale": showNumber(key, R.string.host_profile_scale); break;
                default: activity.restoredEditorKey = null; break;
            }
        }

        /** The "Same as all PCs" box on top of a value dialog; checked, it disables {@code fields}. */
        private MaterialCheckBox sameAsAll(LinearLayout form, String allPcsValue, boolean checked, View... fields) {
            MaterialCheckBox box = new MaterialCheckBox(activity);
            box.setId(R.id.host_profile_follow_all);
            box.setText(sameAs(allPcsValue));
            box.setChecked(checked);
            form.addView(box);
            CompoundButton.OnCheckedChangeListener listener = (button, isChecked) -> {
                for (View field : fields) field.setEnabled(!isChecked);
            };
            box.setOnCheckedChangeListener(listener);
            return box;
        }

        private static void enable(boolean enabled, View... views) {
            for (View view : views) view.setEnabled(enabled);
        }

        private EditText field(LinearLayout form, int title, String value, boolean decimal) {
            TextInputLayout container = new TextInputLayout(activity);
            container.setHint(title);
            EditText field = new TextInputEditText(container.getContext());
            field.setId(form.findViewById(R.id.host_profile_field_primary) == null ?
                    R.id.host_profile_field_primary : R.id.host_profile_field_secondary);
            field.setContentDescription(getString(title));
            field.setInputType(InputType.TYPE_CLASS_NUMBER | (decimal ? InputType.TYPE_NUMBER_FLAG_DECIMAL : 0));
            field.setSingleLine(true);
            field.setSelectAllOnFocus(true);
            field.setMinHeight(Math.round(56 * getResources().getDisplayMetrics().density));
            field.setText(value);
            container.addView(field);
            form.addView(container);
            return field;
        }

        // Inline under the field (a TextInputLayout error), so it stays visible and screen readers announce it.
        private void showError(EditText field, String error) {
            View parent = (View) field.getParent();
            while (!(parent instanceof TextInputLayout)) parent = (View) parent.getParent();
            ((TextInputLayout) parent).setError(error);
        }

        private void showSharpness() {
            PreferenceConfiguration config = activity.effective();
            LinearLayout form = form();
            TextView value = new TextView(activity);
            Slider slider = new Slider(activity);
            slider.setId(R.id.host_profile_sharpness);
            MaterialCheckBox allPcs = sameAsAll(form, getString(R.string.upscaling_sharpness_value,
                    activity.global.upscalingSharpness), activity.overrides.upscalingSharpness == null, slider);
            value.setText(getString(R.string.upscaling_sharpness_value, config.upscalingSharpness));
            value.setTextAppearance(activity, R.style.TextAppearance_Nightjar_ScreenTitle);
            form.addView(value);
            slider.setContentDescription(getString(R.string.title_upscaling_sharpness));
            slider.setValueFrom(0);
            slider.setValueTo(100);
            slider.setStepSize(1);
            slider.setValue(config.upscalingSharpness);
            slider.setEnabled(!allPcs.isChecked());
            slider.addOnChangeListener((view, strength, fromUser) ->
                    value.setText(getString(R.string.upscaling_sharpness_value, Math.round(strength))));
            form.addView(slider);
            showEditor("sharpness", new NavigationDialogBuilder(activity).setTitle(R.string.title_upscaling_sharpness).setView(scrollable(form))
                    .setPositiveButton(android.R.string.ok, (dialog, which) -> {
                        activity.overrides.upscalingSharpness = allPcs.isChecked() ? null : Math.round(slider.getValue());
                        updateSummaries();
                    }).setNegativeButton(android.R.string.cancel, null).create());
        }

        private void showResolution() {
            PreferenceConfiguration config = activity.effective();
            HostStreamProfile own = activity.overrides;
            LinearLayout form = form();
            MaterialCheckBox nativeChoice = new MaterialCheckBox(activity);
            nativeChoice.setId(R.id.host_profile_follow_screen);
            nativeChoice.setText(R.string.host_profile_native);
            nativeChoice.setChecked(config.resolutionFollowsScreen);
            EditText[] size = new EditText[2];
            MaterialCheckBox allPcs = sameAsAll(form, resolutionText(activity.global), own.nativeResolution == null,
                    nativeChoice);
            form.addView(nativeChoice);
            size[0] = field(form, R.string.host_profile_width, Integer.toString(config.width), false);
            size[1] = field(form, R.string.host_profile_height, Integer.toString(config.height), false);
            Runnable refresh = () -> {
                nativeChoice.setEnabled(!allPcs.isChecked());
                enable(!allPcs.isChecked() && !nativeChoice.isChecked(), size);
            };
            allPcs.setOnCheckedChangeListener((button, checked) -> refresh.run());
            nativeChoice.setOnCheckedChangeListener((button, checked) -> refresh.run());
            refresh.run();
            AlertDialog dialog = new NavigationDialogBuilder(activity).setTitle(R.string.title_resolution_list)
                    .setView(scrollable(form)).setPositiveButton(android.R.string.ok, null)
                    .setNegativeButton(android.R.string.cancel, null)
                    .setNeutralButton(R.string.host_profile_match_screen, null).create();
            dialog.setOnShowListener(ignored -> {
                dialog.getButton(AlertDialog.BUTTON_NEUTRAL).setOnClickListener(v -> {
                    allPcs.setChecked(false);
                    nativeChoice.setChecked(false);
                    // The panel's own full size, as a follow-screen stream gets it. Since Android 12 the
                    // activity's real metrics can be its window's bounds, which leave out a camera
                    // cutout the app is kept clear of, or a split screen's other half; such a size
                    // then streamed with a strip beside the camera.
                    String panel = PreferenceConfiguration.nativeResolution(activity);
                    if (panel == null) {
                        DisplayMetrics metrics = new DisplayMetrics();
                        activity.getWindowManager().getDefaultDisplay().getRealMetrics(metrics);
                        panel = Math.max(metrics.widthPixels, metrics.heightPixels) + "x" +
                                Math.min(metrics.widthPixels, metrics.heightPixels);
                    }
                    size[0].setText(Integer.toString(PreferenceConfiguration.getWidthFromResolutionString(panel)));
                    size[1].setText(Integer.toString(PreferenceConfiguration.getHeightFromResolutionString(panel)));
                });
                dialog.getButton(AlertDialog.BUTTON_POSITIVE).setOnClickListener(v -> {
                    if (allPcs.isChecked() || nativeChoice.isChecked()) {
                        own.nativeResolution = allPcs.isChecked() ? null : Boolean.TRUE;
                        own.width = own.height = null;
                        updateSummaries();
                        dialog.dismiss();
                        return;
                    }
                    Integer w = readNumber(size[0], 64, 16384);
                    Integer h = readNumber(size[1], 64, 16384);
                    if (w != null && h != null) {
                        own.nativeResolution = false;
                        own.width = w;
                        own.height = h;
                        updateSummaries();
                        dialog.dismiss();
                    }
                });
            });
            showEditor("resolution", dialog);
        }

        private void showNumber(String key, int title) {
            PreferenceConfiguration config = activity.effective();
            PreferenceConfiguration global = activity.global;
            HostStreamProfile own = activity.overrides;
            LinearLayout form = form();
            String value, allPcsValue;
            boolean followsAll;
            if (key.equals("refresh")) {
                value = HostStreamProfile.formatRefreshRate(config.launchRefreshRateX100 == 0 ?
                        config.fps * 100 : config.launchRefreshRateX100);
                allPcsValue = refreshText(global);
                followsAll = own.refreshRateX100 == null;
            } else if (key.equals("bitrate")) {
                value = HostStreamProfile.formatBitrateMbps(config.bitrate);
                allPcsValue = HostStreamProfile.formatBitrateMbps(global.bitrate) + " Mbps";
                followsAll = own.bitrate == null;
            } else {
                value = Integer.toString(config.virtualDisplayScale);
                allPcsValue = global.virtualDisplayScale + "%";
                followsAll = own.renderScale == null;
            }
            EditText[] field = new EditText[1];
            MaterialCheckBox allPcs = sameAsAll(form, allPcsValue, followsAll);
            field[0] = field(form, title, value, !key.equals("scale"));
            allPcs.setOnCheckedChangeListener((button, checked) -> field[0].setEnabled(!checked));
            field[0].setEnabled(!followsAll);
            MaterialAlertDialogBuilder builder = new NavigationDialogBuilder(activity).setTitle(title).setView(scrollable(form))
                    .setPositiveButton(android.R.string.ok, null).setNegativeButton(android.R.string.cancel, null);
            if (key.equals("refresh")) builder.setNeutralButton(R.string.host_profile_match_screen, null);
            AlertDialog dialog = builder.create();
            dialog.setOnShowListener(ignored -> {
                if (key.equals("refresh")) {
                    dialog.getButton(AlertDialog.BUTTON_NEUTRAL).setOnClickListener(v -> {
                        allPcs.setChecked(false);
                        field[0].setText(HostStreamProfile.formatRefreshRate(Math.round(100 *
                                com.limelight.binding.video.DisplayFrameRatePolicy.maxRefreshRate(
                                        activity.getWindowManager().getDefaultDisplay()))));
                    });
                }
                dialog.getButton(AlertDialog.BUTTON_POSITIVE).setOnClickListener(v -> {
                    showError(field[0], null);
                    boolean clear = allPcs.isChecked();
                    try {
                        if (key.equals("refresh")) {
                            own.refreshRateX100 = clear ? null : HostStreamProfile.parseRefreshRate(field[0].getText().toString());
                        } else if (key.equals("bitrate")) {
                            own.bitrate = clear ? null : HostStreamProfile.parseBitrateMbps(field[0].getText().toString());
                        } else if (clear) {
                            own.renderScale = null;
                        } else {
                            Integer scale = readNumber(field[0], 50, 200);
                            if (scale == null) return;
                            own.renderScale = scale;
                        }
                        updateSummaries();
                        dialog.dismiss();
                    } catch (IllegalArgumentException e) {
                        showError(field[0], getString(key.equals("refresh") ? R.string.host_profile_invalid_refresh : R.string.host_profile_invalid_bitrate));
                    }
                });
            });
            showEditor(key, dialog);
        }

        private Integer readNumber(EditText field, int min, int max) {
            showError(field, null);
            try {
                return HostStreamProfile.requireRange(Integer.parseInt(field.getText().toString().trim()), min, max);
            } catch (IllegalArgumentException e) {
                showError(field, getString(R.string.host_profile_invalid_number, min, max));
                return null;
            }
        }

        @Override
        public View onCreateView(LayoutInflater inflater, ViewGroup container, Bundle state) {
            View view = super.onCreateView(inflater, container, state);
            ListView list = view.findViewById(android.R.id.list);
            com.limelight.ui.UiNavigation.bindPreferencesWhenReady(list);
            list.setNextFocusDownId(R.id.host_profile_save);
            list.setDivider(null);
            list.setSelector(R.drawable.list_focus_row);
            list.setPadding(0, 0, 0, getResources().getDimensionPixelSize(R.dimen.space_s));
            return view;
        }
    }
}
