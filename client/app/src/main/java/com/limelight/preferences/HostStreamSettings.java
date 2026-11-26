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
import android.view.LayoutInflater;
import android.view.View;
import android.view.ViewGroup;
import android.widget.ListView;
import android.widget.TextView;
import android.widget.Toast;

import androidx.activity.OnBackPressedCallback;
import androidx.appcompat.app.AlertDialog;

import com.google.android.material.appbar.MaterialToolbar;
import com.google.android.material.dialog.MaterialAlertDialogBuilder;
import com.limelight.binding.video.HdrPolicy;
import com.limelight.R;
import com.limelight.binding.video.DisplayFrameRatePolicy;
import com.limelight.computers.ComputerManagerService;
import com.limelight.computers.HostActions;
import com.limelight.computers.HostCapabilityCache;
import com.limelight.computers.HostNames;
import com.limelight.computers.IdentityManager;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.wol.WakeTargets;
import com.limelight.ui.NavigationDialogBuilder;
import com.limelight.ui.NightjarActivity;
import com.limelight.ui.TopAppBar;
import com.limelight.utils.PyroWaveBandwidthTest;
import com.limelight.utils.PyroWavePaceCalibration;
import com.limelight.utils.ServerHelper;
import com.limelight.utils.ShortcutHelper;
import com.limelight.utils.UiHelper;

import java.util.Objects;

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
    /** A TV lays out Settings' screens at {@link com.limelight.ui.AppTheme#TV_COMPACT_PERCENT} of the interface size. */
    @Override
    protected boolean compactOnTelevision() {
        return true;
    }

    static final String EXTRA_MODE = "mode";
    static final String EXTRA_PROFILE_ID = "profile_id";
    static final String EXTRA_APP_UUID = "app_uuid";
    static final String EXTRA_APP_ID = "app_id";
    static final String EXTRA_APP_NAME = "app_name";
    /** The row the list starts at. */
    static final String EXTRA_FOCUS_KEY = "focus_key";
    /** The first of the network tests, as Settings' Network tests row opens them. */
    static final String NETWORK_TESTS_ROW = "connection_test";

    /** Whose values this screen edits. */
    enum Mode { PC, PROFILE, GAME }

    Mode mode = Mode.PC;
    private String profileId, appUuid, appName;
    private int appId;
    StreamLayers.Result below;
    PreferenceConfiguration global;
    HostStreamProfile overrides;
    String hostUuid;
    HostCapabilities caps = HostCapabilities.UNKNOWN;
    ComputerDetails computer;
    private boolean managerBound;
    private ComputerManagerService.ComputerManagerBinder manager;
    private boolean foreground;
    private boolean polling;
    final HostActions hostActions = new HostActions(this, new HostActions.Callbacks() {
        @Override public ComputerManagerService.ComputerManagerBinder binder() { return manager; }
        @Override public boolean isForeground() { return foreground; }
        @Override public void onPaired(ComputerDetails details) { }
        @Override public void onRemoved(ComputerDetails details) { finish(); }
        @Override public void onRenamed(ComputerDetails details) { showName(); }
        @Override public void onWakeSent(ComputerDetails details) { }
        @Override public void pausePolling() { }
        @Override public void resumePolling() { }
    });
    private PyroWaveBandwidthTest connectionTest;
    private PyroWavePaceCalibration paceCalibration;
    private final HostFrontendExporter frontendExporter = new HostFrontendExporter(this, this::refreshActions);
    private String savedProfile;
    String editorKey;
    AlertDialog editorDialog;
    Bundle restoredEditor;
    String restoredEditorKey;

    private final ServiceConnection managerConnection = new ServiceConnection() {
        @Override
        public void onServiceConnected(ComponentName name, IBinder binder) {
            if (!(binder instanceof ComputerManagerService.ComputerManagerBinder)) return;
            ComputerManagerService.ComputerManagerBinder bound = (ComputerManagerService.ComputerManagerBinder) binder;
            manager = bound;
            if (foreground) startPolling();
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
        public void onServiceDisconnected(ComponentName name) { }
    };

    public static void show(Activity activity, String hostUuid, String hostName) {
        activity.startActivity(new Intent(activity, HostStreamSettings.class)
                .putExtra("host_uuid", hostUuid).putExtra("host_name", hostName));
    }

    /** A PC's settings, opened at its network tests: the connection test and the frame pace calibration. */
    public static void showTests(Activity activity, String hostUuid, String hostName) {
        activity.startActivity(new Intent(activity, HostStreamSettings.class)
                .putExtra("host_uuid", hostUuid).putExtra("host_name", hostName)
                .putExtra(EXTRA_FOCUS_KEY, NETWORK_TESTS_ROW));
    }

    /** A named profile's values. */
    public static void showProfile(Activity activity, String profileId) {
        activity.startActivity(new Intent(activity, HostStreamSettings.class)
                .putExtra(EXTRA_MODE, Mode.PROFILE.name()).putExtra(EXTRA_PROFILE_ID, profileId));
    }

    /** One game's values on one PC. */
    public static void showGame(Activity activity, String hostUuid, String hostName, String appUuid, int appId,
                                String appName) {
        activity.startActivity(intentForGame(activity, hostUuid, hostName, appUuid, appId, appName));
    }

    static Intent intentForGame(android.content.Context context, String hostUuid, String hostName, String appUuid,
                                int appId, String appName) {
        return new Intent(context, HostStreamSettings.class)
                .putExtra(EXTRA_MODE, Mode.GAME.name()).putExtra("host_uuid", hostUuid).putExtra("host_name", hostName)
                .putExtra(EXTRA_APP_UUID, appUuid).putExtra(EXTRA_APP_ID, appId).putExtra(EXTRA_APP_NAME, appName);
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
            overrides = HostStreamProfile.deserializeLenient(state.getString("draft"));
            restoredEditorKey = state.getString("editor_key");
            restoredEditor = state.getBundle("editor_state");
            frontendExporter.restoreState(state);
        }
        setContentView(R.layout.activity_host_stream_settings);
        com.limelight.ui.NightSkyView.behind(this);
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

    private HostStreamProfile loadOwn(StreamProfiles.Snapshot profiles) {
        String raw = mode == Mode.PROFILE ? profiles.rawValues(profileId) : mode == Mode.GAME ?
                AppStreamSettings.raw(this, hostUuid, appUuid, appId) : HostStreamProfile.raw(this, hostUuid);
        return raw == null ? new HostStreamProfile() : HostStreamProfile.deserializeLenient(raw);
    }

    void saveVirtualDisplay(Boolean value) {
        overrides.virtualDisplay = value;
        HostStreamProfile saved = HostStreamProfile.deserializeLenient(savedProfile);
        saved.virtualDisplay = value;
        saved.save(this, hostUuid);
        savedProfile = saved.serialize();
    }

    private void saveOwn() {
        switch (mode) {
            case PROFILE: StreamProfiles.saveValues(this, profileId, overrides); break;
            case GAME: AppStreamSettings.save(this, hostUuid, appUuid, appId, overrides); break;
            default: overrides.save(this, hostUuid); break;
        }
    }

    private PreferenceConfiguration baseConfig() {
        PreferenceConfiguration config = PreferenceConfiguration.readPreferences(this);
        applyWithScreen(below.values, config, below.values.pacing);
        config.automaticBitrate &= !below.bitrateLayered;
        return config;
    }

    int qualityBitrate() {
        PreferenceConfiguration main = PreferenceConfiguration.readPreferences(this);
        String raw = HostStreamProfile.raw(this, hostUuid);
        HostStreamProfile pc = raw == null ? null : HostStreamProfile.deserializeLenient(raw);
        return GameProfilePreset.qualityBitrate(GameProfilePreset.baseBitrate(main.bitrate, pc));
    }

    private void applyWithScreen(HostStreamProfile values, PreferenceConfiguration config, String pacing) {
        boolean followedScreen = config.resolutionFollowsScreen;
        values.applyTo(config);
        config.vrr &= DisplayFrameRatePolicy.vrrOffered(this);
        config.resolveHostFrameTimes();
        if (config.resolutionFollowsScreen && !followedScreen) {
            String nativeResolution = PreferenceConfiguration.nativeResolution(this);
            if (nativeResolution != null) {
                config.width = PreferenceConfiguration.getWidthFromResolutionString(nativeResolution);
                config.height = PreferenceConfiguration.getHeightFromResolutionString(nativeResolution);
            }
        }
        if (config.automaticBitrate && values.bitrate == null) config.bitrate = config.automaticBitrateKbps();
        config.framePacing = PreferenceConfiguration.layeredFramePacing(this, config, pacing);
    }

    @Override
    protected void onResume() {
        super.onResume();
        foreground = true;
        startPolling();
    }

    @Override
    protected void onPause() {
        super.onPause();
        foreground = false;
        stopPolling();
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
        stopPolling();
        if (managerBound) {
            unbindService(managerConnection);
            managerBound = false;
        }
    }

    void showComputer(ComputerDetails details) {
        details.alias = HostNames.get(this, hostUuid);
        computer = details;
        showName();
        boolean capsChanged = details.caps.isKnown() && !details.caps.equals(caps);
        if (details.caps.isKnown()) {
            caps = details.caps;
            if (mode == Mode.PC) {
                ((MaterialToolbar) findViewById(R.id.host_profile_toolbar)).setSubtitle(caps.familyLabel(this));
            }
        }
        if (capsChanged) {
            SettingsFragment fragment = (SettingsFragment) getFragmentManager().findFragmentById(R.id.host_profile_settings);
            if (fragment != null && fragment.isAdded()) {
                fragment.rebuildForCapabilities();
                return;
            }
        }
        refreshActions();
    }

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

    void followComputer(ComputerDetails details) {
        if (sameRows(computer, details)) {
            details.alias = computer.alias;
            computer = details;
        } else {
            showComputer(details);
        }
    }

    private static boolean sameRows(ComputerDetails shown, ComputerDetails polled) {
        return isOnline(shown) == isOnline(polled) && shown.pairState == polled.pairState
                && Objects.equals(shown.name, polled.name) && Objects.equals(shown.activeAddress, polled.activeAddress)
                && shown.httpsPort == polled.httpsPort && Objects.equals(shown.macAddress, polled.macAddress)
                && shown.caps.equals(polled.caps);
    }

    boolean isOnline() {
        return isOnline(computer);
    }

    private static boolean isOnline(ComputerDetails pc) {
        return pc.state == ComputerDetails.State.ONLINE && pc.activeAddress != null;
    }

    void startConnectionTest() {
        if (connectionTest != null) connectionTest.cancel();
        connectionTest = new PyroWaveBandwidthTest(this, computer, new IdentityManager(this).getUniqueId(), effective().bitrate);
        connectionTest.show();
    }

    void startPaceCalibration() {
        if (paceCalibration != null) paceCalibration.cancel();
        PreferenceConfiguration config = effective();
        paceCalibration = new PyroWavePaceCalibration(this, computer, new IdentityManager(this).getUniqueId(),
                config.bitrate, config.fps, this::refreshActions);
        paceCalibration.show();
    }

    void showHostAdministration() {
        okhttp3.HttpUrl url = ServerHelper.getHostSettingsUrl(computer);
        if (url == null) return;
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

    void showWakeHelp() {
        MaterialAlertDialogBuilder dialog = new NavigationDialogBuilder(this).setTitle(R.string.wol_help_title)
                .setMessage(R.string.wol_help_text);
        if (computer.state != ComputerDetails.State.ONLINE && WakeTargets.hasMac(this, computer)) {
            dialog.setNegativeButton(android.R.string.cancel, null)
                    .setPositiveButton(R.string.pcview_menu_send_wol, (prompt, which) -> hostActions.wake(computer));
        } else {
            dialog.setPositiveButton(android.R.string.ok, null);
        }
        UiHelper.showDialog(this, dialog.create());
    }

    private void startPolling() {
        if (polling || manager == null) return;
        polling = true;
        manager.startPolling(details -> {
            if (!hostUuid.equalsIgnoreCase(details.uuid)) return;
            ComputerDetails copy = new ComputerDetails(details);
            runOnUiThread(() -> {
                if (!isFinishing() && !isDestroyed()) followComputer(copy);
            });
        });
    }

    private void stopPolling() {
        if (polling) {
            polling = false;
            manager.stopPolling();
        }
    }

    void pinShortcut() {
        if (!new ShortcutHelper(this).createPinnedPcShortcut(computer)) {
            Toast.makeText(this, R.string.unable_to_pin_shortcut, Toast.LENGTH_LONG).show();
        }
    }

    boolean canPinShortcuts() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return false;
        ShortcutManager shortcuts = getSystemService(ShortcutManager.class);
        return shortcuts != null && shortcuts.isRequestPinShortcutSupported();
    }

    void startFrontendExport() {
        frontendExporter.start();
    }

    private void refreshActions() {
        SettingsFragment fragment = (SettingsFragment) getFragmentManager().findFragmentById(R.id.host_profile_settings);
        if (fragment != null && fragment.activity != null) {
            fragment.showActions();
        }
    }

    @Override
    protected void onActivityResult(int requestCode, int resultCode, Intent data) {
        super.onActivityResult(requestCode, resultCode, data);
        frontendExporter.onActivityResult(requestCode, resultCode, data);
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

    void updateSaveState() {
        boolean changed = !savedProfile.equals(overrides.serialize());
        findViewById(R.id.host_profile_save).setEnabled(changed);
        int foot = changed ? View.VISIBLE : View.GONE;
        findViewById(R.id.host_profile_save).setVisibility(foot);
        findViewById(R.id.host_profile_status).setVisibility(foot);
        ((TextView) findViewById(R.id.host_profile_status)).setText(mode != Mode.PC ?
                (changed ? R.string.layers_unsaved_context : R.string.layers_saved_context) : changed ?
                R.string.settings_profile_unsaved_context : R.string.settings_profile_saved_context);
    }

    PreferenceConfiguration effective() {
        PreferenceConfiguration config = baseConfig();
        if (overrides.has(HostStreamProfile.Field.PACING)) {
            PreferenceConfiguration main = PreferenceConfiguration.readPreferences(this);
            config.gridHostFrameTimes = main.gridHostFrameTimes;
            config.dropLateFrames = main.dropLateFrames;
        }
        applyWithScreen(overrides, config, overrides.pacing != null ? overrides.pacing : below.values.pacing);
        config.automaticBitrate &= overrides.bitrate == null;
        return config;
    }

    @Override
    protected void onSaveInstanceState(Bundle state) {
        super.onSaveInstanceState(state);
        if (overrides != null) state.putString("draft", overrides.serialize());
        frontendExporter.saveState(state);
        if (editorDialog != null && editorDialog.isShowing()) {
            state.putString("editor_key", editorKey);
            state.putBundle("editor_state", editorDialog.onSaveInstanceState());
        }
    }

    public static class SettingsFragment extends PreferenceFragment {
        HostStreamSettings activity;
        private PreferenceCategory connection;
        private PreferenceCategory host;

        @Override
        public void onActivityCreated(Bundle state) {
            super.onActivityCreated(state);
            activity = (HostStreamSettings) getActivity();
            buildPreferenceScreen();
            View root = getView();
            View list = root == null ? null : root.findViewById(android.R.id.list);
            String focus = state == null ? activity.getIntent().getStringExtra(EXTRA_FOCUS_KEY) : null;
            if (list != null) list.post(() -> {
                int position = focus != null && list instanceof ListView ?
                        StreamSettings.RootFragment.positionOf((ListView) list, focus) : -1;
                if (position >= 0) ((ListView) list).setSelection(position);
                if (!list.isInTouchMode() && !list.hasFocus()) list.requestFocus();
            });
        }

        void rebuildForCapabilities() {
            if (activity == null) return;
            View root = getView();
            ListView list = root != null && root.findViewById(android.R.id.list) instanceof ListView
                    ? (ListView) root.findViewById(android.R.id.list) : null;
            int firstVisible = list == null ? -1 : list.getFirstVisiblePosition();
            int topOffset = firstVisible >= 0 && list.getChildCount() > 0
                    ? list.getChildAt(0).getTop() - list.getPaddingTop() : 0;
            String topKey = preferenceKeyAt(list, firstVisible);
            int selectedPos = list == null ? ListView.INVALID_POSITION : list.getSelectedItemPosition();
            String focusedKey = preferenceKeyAt(list, selectedPos);
            boolean hadFocus = list != null && list.hasFocus();

            buildPreferenceScreen();

            View rebuiltRoot = getView();
            ListView rebuiltList = rebuiltRoot != null && rebuiltRoot.findViewById(android.R.id.list) instanceof ListView
                    ? (ListView) rebuiltRoot.findViewById(android.R.id.list) : null;
            if (rebuiltList != null) {
                rebuiltList.post(() -> restoreListState(rebuiltList, topKey, firstVisible, topOffset, focusedKey, hadFocus));
            }
        }

        private static String preferenceKeyAt(ListView list, int position) {
            if (list == null || list.getAdapter() == null || position < 0 || position >= list.getAdapter().getCount()) {
                return null;
            }
            Object item = list.getAdapter().getItem(position);
            return item instanceof Preference ? ((Preference) item).getKey() : null;
        }

        private static void restoreListState(ListView list, String topKey, int fallbackPos, int topOffset,
                                             String focusedKey, boolean hadFocus) {
            if (list.getAdapter() == null || list.getAdapter().getCount() == 0) return;
            int anchor = topKey != null ? StreamSettings.RootFragment.positionOf(list, topKey) : -1;
            if (anchor < 0 && fallbackPos >= 0) {
                anchor = Math.min(fallbackPos, list.getAdapter().getCount() - 1);
            }
            if (anchor >= 0) {
                list.setSelectionFromTop(anchor, topOffset);
            }
            int focusPos = focusedKey != null ? StreamSettings.RootFragment.positionOf(list, focusedKey) : -1;
            if (focusPos >= 0 && !list.isInTouchMode()) {
                list.setSelection(focusPos);
            }
            if (hadFocus && !list.isInTouchMode() && !list.hasFocus()) {
                list.requestFocus();
            }
        }

        private void buildPreferenceScreen() {
            PreferenceScreen screen = getPreferenceManager().createPreferenceScreen(activity);
            setPreferenceScreen(screen);
            connection = null;
            host = null;
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
            if (HdrPolicy.hasHdr10(HdrPolicy.displayHdrTypes(activity.getWindowManager().getDefaultDisplay()))) {
                row(stream, "hdr_format", R.string.title_hdr_format, this::showHdrFormat);
            }
            row(stream, "vrr", R.string.title_vrr, this::showVrr);
            row(stream, "pacing", R.string.title_frame_pacing, this::showPacing);
            row(stream, "yuv444", R.string.title_sharp_color, () -> showSwitch("yuv444", R.string.title_sharp_color));
            row(stream, "full_range", R.string.title_full_range, () -> showSwitch("full_range", R.string.title_full_range));
            if ((activity.mode == Mode.PC || activity.mode == Mode.GAME) && activity.caps.offersEnhancement(HostCapabilities.RTX_HDR)) {
                row(stream, "rtx_hdr", R.string.title_rtx_hdr, () -> showEnhancement(HostStreamProfile.Field.RTX_HDR, R.string.title_rtx_hdr));
            }
            if ((activity.mode == Mode.PC || activity.mode == Mode.GAME) && activity.caps.offersEnhancement(HostCapabilities.RTX_VIDEO)) {
                row(stream, "rtx_video", R.string.title_rtx_video, () -> showEnhancement(HostStreamProfile.Field.RTX_VIDEO, R.string.title_rtx_video));
            }
            boolean pc = activity.mode == Mode.PC;
            int reset = pc ? R.string.host_profile_reset : R.string.layers_reset;
            row(stream, "reset", reset, () -> UiHelper.showDialog(activity,
                    new NavigationDialogBuilder(activity).startOnCancel().setTitle(reset)
                            .setMessage(pc ? R.string.settings_profile_reset_message : R.string.layers_reset_message)
                            .setPositiveButton(reset, (dialog, which) -> {
                                activity.overrides = new HostStreamProfile();
                                updateSummaries();
                            }).setNegativeButton(android.R.string.cancel, null).create()));

            boolean displayModes = pc && HostStreamProfileSummary.gate(activity, HostCapabilities.Feature.HOST_DISPLAY_MODES).isVisible();
            boolean virtualDisplay = activity.mode != Mode.PROFILE
                    && HostStreamProfileSummary.gate(activity, HostCapabilities.Feature.VIRTUAL_DISPLAY).isVisible();
            boolean displayScale = activity.mode != Mode.PROFILE
                    && HostStreamProfileSummary.gate(activity, HostCapabilities.Feature.DISPLAY_SCALE).isVisible();
            if (displayModes || virtualDisplay || displayScale) {
                PreferenceCategory display = category(screen, R.string.pc_settings_display);
                if (displayModes) {
                    row(display, "host_display", R.string.host_display_menu, this::showDisplay);
                }
                if (virtualDisplay) {
                    if (!(displayModes && HostStreamProfileSummary.gate(activity, HostCapabilities.Feature.HOST_DISPLAY_MODES).isEnabled())) {
                        row(display, "virtual", R.string.title_virtual_display, () -> showSwitch("virtual", R.string.title_virtual_display));
                    }
                    row(display, "scale", R.string.title_virtual_display_scale, () -> showNumber("scale", R.string.host_profile_scale));
                }
                if (displayScale) {
                    row(display, "dpi", R.string.title_virtual_display_dpi, this::showDisplayScale);
                }
            }
            if (pc) {
                connection = category(screen, R.string.pc_settings_connection);
                host = category(screen, R.string.pc_settings_host);
                showActions();
            }
            Preference footer = new Preference(activity);
            footer.setLayoutResource(R.layout.settings_preference);
            footer.setSummary(pc ? R.string.host_profile_footer : R.string.layers_footer);
            footer.setSelectable(false);
            screen.addPreference(footer);
            updateSummaries();
        }

        private PreferenceCategory category(PreferenceScreen screen, int title) {
            PreferenceCategory category = new PreferenceCategory(activity);
            category.setTitle(title);
            category.setLayoutResource(R.layout.settings_category);
            screen.addPreference(category);
            return category;
        }

        Preference row(PreferenceCategory category, String key, int title, Runnable onClick) {
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

        void showActions() {
            HostStreamProfileSummary.showActions(this, activity, connection, host);
        }

        void updateSummaries() {
            HostStreamProfileSummary.updateSummaries(this, activity);
        }

        void showEditor(String key, AlertDialog dialog) {
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
                case "wake_macs": showWakeMacs(); break;
                default: activity.restoredEditorKey = null; break;
            }
        }

        private void showSwitch(String key, int title) { HostStreamSettingDialogs.showSwitch(this, activity, key, title); }
        private void showVrr() { HostStreamSettingDialogs.showVrr(this, activity); }
        private void showPreset() { HostStreamSettingDialogs.showPreset(this, activity); }
        private void showPacing() { HostStreamSettingDialogs.showPacing(this, activity); }
        private void showEnhancement(HostStreamProfile.Field field, int title) { HostStreamSettingDialogs.showEnhancement(this, activity, field, title); }
        private void showDisplayScale() { HostStreamSettingDialogs.showDisplayScale(this, activity); }
        private void showCodec() { HostStreamSettingDialogs.showCodec(this, activity); }
        private void showHdrFormat() { HostStreamSettingDialogs.showHdrFormat(this, activity); }
        private void showUpscaling() { HostStreamSettingDialogs.showUpscaling(this, activity); }
        private void showDisplay() { HostStreamSettingDialogs.showDisplay(this, activity); }
        private void showSharpness() { HostStreamSettingDialogs.showSharpness(this, activity); }
        private void showResolution() { HostStreamSettingDialogs.showResolution(this, activity); }
        private void showNumber(String key, int title) { HostStreamSettingDialogs.showNumber(this, activity, key, title); }
        void showWakeMacs() { HostStreamSettingDialogs.showWakeMacs(this, activity); }

        @Override
        public View onCreateView(LayoutInflater inflater, ViewGroup container, Bundle state) {
            View view = super.onCreateView(inflater, container, state);
            ListView list = GroupedListView.adopt(view);
            com.limelight.ui.UiNavigation.bindPreferencesWhenReady(list);
            list.setNextFocusDownId(R.id.host_profile_save);
            list.setDivider(null);
            list.setSelector(R.drawable.settings_row_focus);
            list.setPadding(0, 0, 0, getResources().getDimensionPixelSize(R.dimen.space_s));
            return view;
        }
    }
}
