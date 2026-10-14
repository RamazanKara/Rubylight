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
import androidx.appcompat.app.AppCompatActivity;
import androidx.activity.OnBackPressedCallback;
import com.google.android.material.appbar.MaterialToolbar;
import com.limelight.ui.TopAppBar;
import com.google.android.material.checkbox.MaterialCheckBox;
import com.google.android.material.dialog.MaterialAlertDialogBuilder;
import com.google.android.material.textfield.TextInputEditText;
import com.google.android.material.textfield.TextInputLayout;
import com.limelight.LimeLog;
import com.limelight.R;
import com.limelight.RemoteSetupActivity;
import com.limelight.binding.video.UpscalingPolicy;
import com.limelight.computers.ComputerManagerService;
import com.limelight.computers.HostCapabilityCache;
import com.limelight.computers.IdentityManager;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.http.PairingManager;
import com.limelight.nvstream.wol.WakeOnLanSender;
import com.limelight.ui.HostFeatureGate;
import com.google.android.material.slider.Slider;
import com.limelight.utils.FrontendExporter;
import com.limelight.utils.HelpLauncher;
import com.limelight.utils.HostDetailsDialog;
import com.limelight.utils.PyroWaveBandwidthTest;
import com.limelight.utils.ServerHelper;
import com.limelight.utils.ShortcutHelper;
import com.limelight.utils.UiHelper;

import java.io.IOException;

/**
 * One PC's settings, titled with its name: its own stream values, its display, its connection
 * and its host. Each stream value is either this PC's own or "Same as all PCs", which follows the
 * main Stream settings, so a global change reaches every PC that doesn't set it. Save keeps the
 * stream values; the other rows act at once.
 */
public final class HostStreamSettings extends AppCompatActivity {
    private static final int PICK_ROMS_REQUEST = 1;
    private static final int PICK_ESDE_REQUEST = 2;

    // The settings for all PCs, and this PC's own values on top of them.
    private PreferenceConfiguration global;
    private HostStreamProfile overrides;
    private String hostUuid;
    // What this PC last showed it can do; rows it can never use are left out.
    private HostCapabilities caps = HostCapabilities.UNKNOWN;
    // The PC as the computer manager last saw it; only its name and saved capabilities until it answers.
    ComputerDetails computer;
    private boolean managerBound;
    private PyroWaveBandwidthTest connectionTest;
    private Uri exportRomsTree;
    private boolean exporting;
    private String savedProfile;
    private String editorKey;
    private AlertDialog editorDialog;
    private Bundle restoredEditor;
    private String restoredEditorKey;

    private final ServiceConnection managerConnection = new ServiceConnection() {
        @Override
        public void onServiceConnected(ComponentName name, IBinder binder) {
            if (!(binder instanceof ComputerManagerService.ComputerManagerBinder)) return;
            ComputerManagerService.ComputerManagerBinder manager = (ComputerManagerService.ComputerManagerBinder) binder;
            new Thread(() -> {
                if (!manager.waitForReady()) return;
                ComputerDetails known = manager.getComputer(hostUuid);
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

    @Override
    protected void onCreate(Bundle state) {
        super.onCreate(state);
        UiHelper.setLocale(this);
        hostUuid = getIntent().getStringExtra("host_uuid");
        if (hostUuid == null) {
            finish();
            return;
        }
        caps = HostCapabilityCache.load(this, hostUuid);
        computer = new ComputerDetails();
        computer.uuid = hostUuid;
        computer.name = getIntent().getStringExtra("host_name");
        computer.caps = caps;
        global = PreferenceConfiguration.readPreferences(this);
        PreferenceConfiguration.migratePcProfilesToOverrides(this);
        HostStreamProfile saved = HostStreamProfile.load(this, hostUuid);
        overrides = saved == null ? new HostStreamProfile() : saved;
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
            String romsTree = state.getString("export_roms_tree");
            exportRomsTree = romsTree != null ? Uri.parse(romsTree) : null;
        }
        setContentView(R.layout.activity_host_stream_settings);
        // The PC is the page: its name is the title, the host software and version the subtitle.
        setTitle(computer.name);
        MaterialToolbar toolbar = TopAppBar.configure(this, R.id.host_profile_toolbar, computer.name, R.menu.host_profile);
        toolbar.setSubtitle(caps.familyLabel(this));
        TopAppBar.setUpNavigation(toolbar, R.id.host_profile_back, v -> leaveSettings());
        getOnBackPressedDispatcher().addCallback(this, new OnBackPressedCallback(true) {
            @Override
            public void handleOnBackPressed() {
                leaveSettings();
            }
        });
        toolbar.setOnMenuItemClickListener(item -> {
            new MaterialAlertDialogBuilder(this).setTitle(R.string.pc_settings_title)
                    .setMessage(R.string.host_profile_explanation).setPositiveButton(android.R.string.ok, null).show();
            return true;
        });
        findViewById(R.id.host_profile_save).setOnClickListener(v -> {
            overrides.save(this, hostUuid);
            Toast.makeText(this, overrides.isEmpty() ? R.string.host_profile_reset_done : R.string.host_profile_saved,
                    Toast.LENGTH_SHORT).show();
            finish();
        });
        updateSaveState();
        if (state == null) {
            getFragmentManager().beginTransaction().replace(R.id.host_profile_settings, new SettingsFragment()).commit();
        }
        UiHelper.notifyNewRootView(this);
        managerBound = bindService(new Intent(this, ComputerManagerService.class), managerConnection, Service.BIND_AUTO_CREATE);
    }

    @Override
    protected void onPause() {
        super.onPause();
        if (connectionTest != null) {
            connectionTest.cancel();
            connectionTest = null;
        }
    }

    @Override
    protected void onDestroy() {
        super.onDestroy();
        if (managerBound) {
            unbindService(managerConnection);
            managerBound = false;
        }
    }

    /** The PC as the computer manager knows it: its state, addresses and what its host reported last. */
    void showComputer(ComputerDetails details) {
        computer = details;
        if (details.caps.isKnown()) {
            caps = details.caps;
            ((MaterialToolbar) findViewById(R.id.host_profile_toolbar)).setSubtitle(caps.familyLabel(this));
        }
        SettingsFragment fragment = (SettingsFragment) getFragmentManager().findFragmentById(R.id.host_profile_settings);
        if (fragment != null && fragment.activity != null) {
            fragment.showActions();
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

    void showHostAdministration() {
        okhttp3.HttpUrl url = ServerHelper.getHostSettingsUrl(computer);
        if (url == null) {
            return;
        }
        AlertDialog dialog = new MaterialAlertDialogBuilder(this)
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
        MaterialAlertDialogBuilder dialog = new MaterialAlertDialogBuilder(this).setTitle(R.string.wol_help_title)
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
        if (new FrontendExporter(this, computer).loadApps().isEmpty()) {
            Toast.makeText(this, R.string.frontend_export_no_apps, Toast.LENGTH_LONG).show();
            return;
        }
        exporting = true;
        exportRomsTree = null;
        new MaterialAlertDialogBuilder(this)
                .setTitle(R.string.frontend_export_roms_title)
                .setMessage(R.string.frontend_export_roms_message)
                .setPositiveButton(R.string.frontend_export_pick, (dialog, which) -> pickFolder(PICK_ROMS_REQUEST))
                .setNegativeButton(android.R.string.cancel, (dialog, which) -> exporting = false)
                .show();
    }

    private void pickFolder(int request) {
        try {
            startActivityForResult(new Intent(Intent.ACTION_OPEN_DOCUMENT_TREE), request);
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
        if (requestCode == PICK_ROMS_REQUEST) {
            if (picked == null) {
                exporting = false;
                return;
            }
            exportRomsTree = picked;
            new MaterialAlertDialogBuilder(this)
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
        final ComputerDetails target = computer;
        final Uri romsTree = exportRomsTree;
        exporting = false;
        exportRomsTree = null;
        if (romsTree == null) {
            return;
        }
        Toast.makeText(this, R.string.frontend_export_working, Toast.LENGTH_SHORT).show();
        new Thread(() -> {
            String message;
            try {
                FrontendExporter exporter = new FrontendExporter(HostStreamSettings.this, target);
                int count = exporter.export(exporter.loadApps(), romsTree, esdeTree);
                message = getResources().getQuantityString(R.plurals.frontend_export_done, count, count);
            } catch (IOException | RuntimeException e) {
                LimeLog.warning("Frontend export failed: " + e);
                message = getString(R.string.frontend_export_failed);
            }
            final String result = message;
            runOnUiThread(() -> Toast.makeText(HostStreamSettings.this, result, Toast.LENGTH_LONG).show());
        }, "Frontend export").start();
    }

    private void leaveSettings() {
        if (savedProfile.equals(overrides.serialize())) {
            finish();
            return;
        }
        UiHelper.showDialog(this, new MaterialAlertDialogBuilder(this).setTitle(R.string.host_profile_unsaved_title)
                .setMessage(R.string.host_profile_unsaved_message)
                .setPositiveButton(R.string.host_profile_save, (dialog, which) -> {
                    overrides.save(this, hostUuid);
                    finish();
                })
                .setNegativeButton(R.string.host_profile_discard, (dialog, which) -> finish())
                .setNeutralButton(android.R.string.cancel, null).create());
    }

    private void updateSaveState() {
        boolean changed = !savedProfile.equals(overrides.serialize());
        findViewById(R.id.host_profile_save).setEnabled(changed);
        ((TextView) findViewById(R.id.host_profile_status)).setText(changed ?
                R.string.settings_profile_unsaved_context : R.string.settings_profile_saved_context);
    }

    /** What the next stream from this PC uses: the global settings with this PC's values on top. */
    private PreferenceConfiguration effective() {
        PreferenceConfiguration config = PreferenceConfiguration.readPreferences(this);
        boolean followedScreen = config.resolutionFollowsScreen;
        overrides.applyTo(config);
        if (config.resolutionFollowsScreen && !followedScreen) {
            String nativeResolution = PreferenceConfiguration.nativeResolution(this);
            if (nativeResolution != null) {
                config.width = PreferenceConfiguration.getWidthFromResolutionString(nativeResolution);
                config.height = PreferenceConfiguration.getHeightFromResolutionString(nativeResolution);
            }
        }
        return config;
    }

    @Override
    protected void onSaveInstanceState(Bundle state) {
        super.onSaveInstanceState(state);
        if (overrides != null) state.putString("draft", overrides.serialize());
        state.putBoolean("exporting", exporting);
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
            row(stream, "resolution", R.string.title_resolution_list, this::showResolution);
            row(stream, "refresh", R.string.title_fps_list, () -> showNumber("refresh", R.string.host_profile_refresh));
            row(stream, "bitrate", R.string.title_seekbar_bitrate, () -> showNumber("bitrate", R.string.host_profile_bitrate));
            row(stream, "codec", R.string.title_video_format, this::showCodec);
            row(stream, "upscaling", R.string.title_upscaling, this::showUpscaling);
            row(stream, "sharpness", R.string.title_upscaling_sharpness, this::showSharpness);
            row(stream, "hdr", R.string.title_enable_hdr, () -> showSwitch("hdr", R.string.title_enable_hdr));
            row(stream, "vrr", R.string.title_vrr, () -> showSwitch("vrr", R.string.title_vrr));
            row(stream, "yuv444", R.string.title_yuv444, () -> showSwitch("yuv444", R.string.title_yuv444));
            row(stream, "full_range", R.string.title_full_range, () -> showSwitch("full_range", R.string.title_full_range));
            row(stream, "reset", R.string.host_profile_reset, () -> UiHelper.showDialog(activity,
                    new MaterialAlertDialogBuilder(activity).setTitle(R.string.host_profile_reset)
                            .setMessage(R.string.settings_profile_reset_message)
                            .setPositiveButton(R.string.host_profile_reset, (dialog, which) -> {
                                activity.overrides = new HostStreamProfile();
                                updateSummaries();
                            }).setNegativeButton(android.R.string.cancel, null).create()));

            // The picker for hosts with display modes, a virtual display switch for hosts with one;
            // neither for hosts that can never use them.
            boolean displayModes = gate(HostCapabilities.Feature.HOST_DISPLAY_MODES).isVisible();
            boolean virtualDisplay = gate(HostCapabilities.Feature.VIRTUAL_DISPLAY).isVisible();
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
            footer.setSummary(R.string.host_profile_footer);
            footer.setSelectable(false);
            screen.addPreference(footer);

            connection = category(screen, R.string.pc_settings_connection);
            host = category(screen, R.string.pc_settings_host);
            showActions();
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
        }

        private String onOff(boolean value) {
            return getString(value ? R.string.host_profile_on : R.string.host_profile_off);
        }

        /** The value, marked when it comes from the settings for all PCs. */
        private String summary(boolean own, String value) {
            return own ? value : getString(R.string.host_profile_follows, value);
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
            findPreference("resolution").setSummary(summary(own.nativeResolution != null, resolutionText(config)) +
                    "\n" + getString(R.string.summary_resolution_list));
            findPreference("refresh").setSummary(summary(own.refreshRateX100 != null, refreshText(config)));
            findPreference("bitrate").setSummary(summary(own.bitrate != null,
                    HostStreamProfile.formatBitrateMbps(config.bitrate) + " Mbps"));
            String[] codecs = getResources().getStringArray(R.array.host_profile_codecs);
            String codec = summary(own.codec != null, codecs[config.videoFormat.ordinal()]);
            if (config.videoFormat == PreferenceConfiguration.FormatOption.FORCE_PYROWAVE &&
                    !activity.caps.mayHave(HostCapabilities.Feature.PYROWAVE)) {
                codec += "\n" + getString(R.string.host_pyrowave_unavailable,
                        codecs[PreferenceConfiguration.codecWithoutPyroWave(activity.global.videoFormat).ordinal()]);
            }
            findPreference("codec").setSummary(codec);
            findPreference("upscaling").setSummary(summary(own.upscalingMode != null, upscalingText(config)));
            findPreference("sharpness").setSummary(summary(own.upscalingSharpness != null,
                    getString(R.string.upscaling_sharpness_value, config.upscalingSharpness)));
            findPreference("sharpness").setEnabled(config.upscalingMode == UpscalingPolicy.Mode.FSR1 ||
                    config.upscalingMode == UpscalingPolicy.Mode.SGSR1);
            for (String key : new String[] {"hdr", "vrr", "yuv444", "full_range"}) {
                findPreference(key).setSummary(summary(switchOverride(key) != null, onOff(switchValue(config, key))));
            }
            findPreference("vrr").setSummary(findPreference("vrr").getSummary() +
                    "\n" + getString(R.string.summary_vrr));
            Preference virtual = findPreference("virtual");
            Preference scale = findPreference("scale");
            if (virtual != null) {
                virtual.setSummary(summary(switchOverride("virtual") != null, onOff(switchValue(config, "virtual"))));
                scale.setSummary(summary(own.renderScale != null, config.virtualDisplayScale + "%") +
                        "\n" + getString(R.string.summary_virtual_display_scale));
            }
            findPreference("reset").setEnabled(!own.isEmpty());
            findPreference("reset").setSummary(own.isEmpty() ? getString(R.string.host_profile_status_global) :
                    getString(R.string.host_profile_status_custom));
            Preference display = findPreference("host_display");
            // Only a host with display modes receives the choice; elsewhere the switch decides.
            HostDisplayChoice displayChoice = activity.caps.mayHave(HostCapabilities.Feature.HOST_DISPLAY_MODES) ?
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
                    getString(R.string.host_profile_same_as_all_value, onOff(allPcs)),
                    getString(R.string.host_profile_on),
                    getString(R.string.host_profile_off)};
            int checked = own == null ? 0 : own ? 1 : 2;
            new MaterialAlertDialogBuilder(activity).setTitle(title)
                    .setSingleChoiceItems(items, checked, (dialog, which) -> {
                        setSwitchOverride(key, which == 0 ? null : which == 1);
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
            items[0] = getString(R.string.host_profile_same_as_all_value, codecs[activity.global.videoFormat.ordinal()]);
            System.arraycopy(codecs, 0, items, 1, offered);
            PreferenceConfiguration.FormatOption own = activity.overrides.codec;
            int checked = own == null ? 0 : own.ordinal() < offered ? own.ordinal() + 1 : -1;
            new MaterialAlertDialogBuilder(activity).setTitle(R.string.title_video_format)
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
            items[0] = getString(R.string.host_profile_same_as_all_value, modes[activity.global.upscalingMode.ordinal()]);
            System.arraycopy(modes, 0, items, 1, modes.length);
            UpscalingPolicy.Mode own = activity.overrides.upscalingMode;
            AlertDialog picker = new MaterialAlertDialogBuilder(activity).setTitle(R.string.title_upscaling)
                    .setSingleChoiceItems(items, own == null ? 0 : own.ordinal() + 1, (dialog, which) -> {
                        activity.overrides.upscalingMode = which == 0 ? null : UpscalingPolicy.Mode.values()[which - 1];
                        updateSummaries();
                        dialog.dismiss();
                    }).setNeutralButton(R.string.help, null)
                    .setNegativeButton(android.R.string.cancel, null).create();
            UiHelper.showDialog(activity, picker);
            picker.getButton(AlertDialog.BUTTON_NEUTRAL).setOnClickListener(view ->
                    UiHelper.showDialog(activity, new MaterialAlertDialogBuilder(activity).setTitle(R.string.title_upscaling)
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
                    activity.getIntent().getStringExtra("host_name"), this::updateSummaries);
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
            box.setText(getString(R.string.host_profile_same_as_all_value, allPcsValue));
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
            showEditor("sharpness", new MaterialAlertDialogBuilder(activity).setTitle(R.string.title_upscaling_sharpness).setView(scrollable(form))
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
            AlertDialog dialog = new MaterialAlertDialogBuilder(activity).setTitle(R.string.title_resolution_list)
                    .setView(scrollable(form)).setPositiveButton(android.R.string.ok, null)
                    .setNegativeButton(android.R.string.cancel, null)
                    .setNeutralButton(R.string.host_profile_match_screen, null).create();
            dialog.setOnShowListener(ignored -> {
                dialog.getButton(AlertDialog.BUTTON_NEUTRAL).setOnClickListener(v -> {
                    allPcs.setChecked(false);
                    nativeChoice.setChecked(false);
                    DisplayMetrics metrics = new DisplayMetrics();
                    activity.getWindowManager().getDefaultDisplay().getRealMetrics(metrics);
                    size[0].setText(Integer.toString(Math.max(metrics.widthPixels, metrics.heightPixels)));
                    size[1].setText(Integer.toString(Math.min(metrics.widthPixels, metrics.heightPixels)));
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
            MaterialAlertDialogBuilder builder = new MaterialAlertDialogBuilder(activity).setTitle(title).setView(scrollable(form))
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
            com.limelight.ui.UiNavigation.bindPreferences(list);
            list.setNextFocusDownId(R.id.host_profile_save);
            list.setDivider(null);
            list.setSelector(R.drawable.list_focus_row);
            list.setPadding(0, 0, 0, getResources().getDimensionPixelSize(R.dimen.space_s));
            return view;
        }
    }
}
