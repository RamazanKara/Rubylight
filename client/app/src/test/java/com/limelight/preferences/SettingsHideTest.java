package com.limelight.preferences;

import android.app.Application;
import android.content.Context;
import android.content.Intent;
import android.content.SharedPreferences;
import android.content.pm.PackageManager;
import android.preference.Preference;
import android.preference.PreferenceFragment;
import android.preference.PreferenceGroup;
import android.preference.PreferenceManager;
import android.preference.PreferenceScreen;
import android.preference.TwoStatePreference;

import com.limelight.R;
import com.limelight.binding.input.touch.TouchMode;

import org.junit.After;
import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;

import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;
import java.util.function.BiConsumer;

import static com.limelight.preferences.SettingsStructureTest.keys;
import static com.limelight.preferences.SettingsStructureTest.open;
import static com.limelight.preferences.SettingsStructureTest.resultGroup;
import static com.limelight.preferences.SettingsStructureTest.search;
import static com.limelight.preferences.SettingsStructureTest.settle;
import static com.limelight.preferences.SettingsStructureTest.shown;
import static org.junit.Assert.*;
import static org.robolectric.Shadows.shadowOf;

/**
 * Settings shows the rows that can do something: Debug only with Show debug settings or a changed Debug setting, and the
 * rows of SettingRules.WHEN_ROWS only in the mode or on the device they act in, or while they hold a changed value.
 * Search finds every one of them.
 */
@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 28, qualifiers = "en", application = Application.class,
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class SettingsHideTest {
    /** A touch-screen handheld with nothing attached, that does not fold, on a chip Chipset tuning leaves alone. */
    static final SettingDevices NOTHING_ATTACHED = new Devices();

    /** Device facts a test switches. */
    static final class Devices implements SettingDevices {
        boolean touchpad, mouse, keyboard, folds, tunedChip, television, touchscreen = true;
        @Override public boolean touchscreen() { return touchscreen; }
        @Override public boolean touchpad() { return touchpad; }
        @Override public boolean mouse() { return mouse; }
        @Override public boolean keyboard() { return keyboard; }
        @Override public boolean folds() { return folds; }
        @Override public boolean tunedChip() { return tunedChip; }
        @Override public boolean television() { return television; }
    }

    private final Devices devices = new Devices();

    private static Context context() {
        return RuntimeEnvironment.getApplication();
    }

    private static SharedPreferences prefs() {
        return PreferenceManager.getDefaultSharedPreferences(context());
    }

    private static SharedPreferences gameInfoKeys() {
        return com.limelight.console.GameInfo.keyPrefs(context());
    }

    @Before
    public void touchscreen() {
        shadowOf(context().getPackageManager()).setSystemFeature(PackageManager.FEATURE_TOUCHSCREEN, true);
    }

    @After
    public void clear() {
        prefs().edit().clear().commit();
        gameInfoKeys().edit().clear().commit();
        context().getSharedPreferences("SettingsScreenState", Context.MODE_PRIVATE).edit().clear().commit();
        for (String name : new String[] {StreamProfiles.PREFERENCES, "AppStreamSettings", "HostStreamProfiles", "AdaptiveQuality"}) {
            context().getSharedPreferences(name, Context.MODE_PRIVATE).edit().clear().commit();
        }
    }

    private ActivityController<StreamSettings> start() {
        ActivityController<StreamSettings> built = Robolectric.buildActivity(StreamSettings.class);
        built.get().devices = devices;
        built.get().savedPcReader = task -> { };
        return built.setup();
    }

    /** The keys of the rows a section's list shows now. */
    private static List<String> visible(StreamSettings.SettingsFragment fragment) {
        List<String> keys = new ArrayList<>();
        PreferenceScreen screen = fragment.getPreferenceScreen();
        for (int i = 0; i < screen.getPreferenceCount(); i++) {
            PreferenceGroup group = (PreferenceGroup) screen.getPreference(i);
            assertTrue(group.getKey() + " shows with no row", group.getPreferenceCount() > 0);
            keys.addAll(keys(group));
        }
        return keys;
    }

    /** Whether the list of sections (the overview, with More settings open) shows this row. */
    private static boolean listed(StreamSettings screen, String row) {
        return ((PreferenceFragment) shown(screen)).getPreferenceScreen().findPreference(row) != null;
    }

    private static Preference result(StreamSettings.SettingsFragment fragment, String key) {
        return fragment.getPreferenceScreen().findPreference("search_result_" + key);
    }

    @Test
    public void showDebugSettingsIsAnAppSwitchOffByDefaultAndItsValueIsKept() {
        try (ActivityController<StreamSettings> controller = start()) {
            StreamSettings screen = controller.get();
            StreamSettings.SettingsFragment app = open(screen, "app");
            Preference row = app.findPreference(SettingRules.SHOW_DEBUG);
            assertTrue("a row of App", visible(app).contains(SettingRules.SHOW_DEBUG));
            assertTrue("behind App's More row", StreamSettings.foldsInSection(context(), "app", SettingRules.SHOW_DEBUG));
            assertFalse(((TwoStatePreference) row).isChecked());
            assertFalse(prefs().getBoolean(SettingRules.SHOW_DEBUG, true));
            ((TwoStatePreference) row).setChecked(true);
            assertTrue("stored", prefs().getBoolean(SettingRules.SHOW_DEBUG, false));
        }
        try (ActivityController<StreamSettings> controller = start()) {
            StreamSettings.SettingsFragment app = open(controller.get(), "app");
            assertTrue("read back", ((TwoStatePreference) app.findPreference(SettingRules.SHOW_DEBUG)).isChecked());
        }
    }

    @Test
    public void debugIsListedOnlyOnRequestOrWhileOneOfItsSettingsIsChanged() {
        StreamSettings.rememberRail(context(), true);
        try (ActivityController<StreamSettings> controller = start()) {
            StreamSettings screen = controller.get();
            settle(screen);
            assertTrue("More settings is open", listed(screen, "advanced"));
            assertFalse("not by default", listed(screen, "debug"));
            prefs().edit().putBoolean(SettingRules.SHOW_DEBUG, true).commit();
            settle(screen);
            assertTrue("with the switch, at once", listed(screen, "debug"));
            prefs().edit().putBoolean(SettingRules.SHOW_DEBUG, false).commit();
            settle(screen);
            assertFalse(listed(screen, "debug"));
            prefs().edit().putBoolean("checkbox_link_probe", true).commit();
            settle(screen);
            assertTrue("a changed Debug setting keeps it in sight", listed(screen, "debug"));
            prefs().edit().putBoolean("checkbox_link_probe", false).commit();
            settle(screen);
            assertFalse("back at its default", listed(screen, "debug"));
        }
    }

    @Test
    public void searchFindsADebugSettingAndALinkStillOpensDebug() {
        try (ActivityController<StreamSettings> controller = start()) {
            StreamSettings screen = controller.get();
            StreamSettings.SettingsFragment results = search(screen, "Test the link");
            assertNotNull("found with the switch off", resultGroup(results, "checkbox_link_probe"));
            result(results, "checkbox_link_probe").getOnPreferenceClickListener().onPreferenceClick(result(results, "checkbox_link_probe"));
            settle(screen);
            StreamSettings.SettingsFragment debug = (StreamSettings.SettingsFragment) shown(screen);
            assertTrue("the result opens Debug", visible(debug).contains("checkbox_link_probe"));
            assertTrue("listed while it shows", screen.debugListed());
        }
        Intent link = StreamSettings.intent(context(), null, "checkbox_link_probe");
        ActivityController<StreamSettings> built = Robolectric.buildActivity(StreamSettings.class, link);
        built.get().devices = devices;
        built.get().savedPcReader = task -> { };
        try (ActivityController<StreamSettings> controller = built.setup()) {
            settle(controller.get());
            assertTrue("a link into Debug opens it", visible((StreamSettings.SettingsFragment) shown(controller.get())).contains("checkbox_link_probe"));
        }
    }

    /**
     * A row of SettingRules.WHEN_ROWS: its section, what makes it do nothing, what makes it act, and a changed value. A
     * device row's facts come from SettingDevices, whose listener devicesChanged stands in for here
     * (aMouseAttachedOrRemovedShowsOrHidesItsRowsThroughTheListener runs the real one).
     */
    private static final class Row {
        final String key, section;
        final boolean device;
        final BiConsumer<SharedPreferences.Editor, Devices> off, on, changed;

        Row(String key, String section, BiConsumer<SharedPreferences.Editor, Devices> off,
                BiConsumer<SharedPreferences.Editor, Devices> on, BiConsumer<SharedPreferences.Editor, Devices> changed) {
            this(key, section, false, off, on, changed);
        }

        Row(String key, String section, boolean device, BiConsumer<SharedPreferences.Editor, Devices> off,
                BiConsumer<SharedPreferences.Editor, Devices> on, BiConsumer<SharedPreferences.Editor, Devices> changed) {
            this.key = key;
            this.section = section;
            this.device = device;
            this.off = off;
            this.on = on;
            this.changed = changed;
        }
    }

    private static final BiConsumer<SharedPreferences.Editor, Devices> NO_TOUCH_SCREEN = (e, d) -> d.touchscreen = false;
    private static final BiConsumer<SharedPreferences.Editor, Devices> TOUCH_SCREEN = (e, d) -> d.touchscreen = true;

    /** No touch screen, and a changed value. */
    private static BiConsumer<SharedPreferences.Editor, Devices> untouchable(BiConsumer<SharedPreferences.Editor, Devices> changed) {
        return (e, d) -> { d.touchscreen = false; changed.accept(e, d); };
    }

    private static final BiConsumer<SharedPreferences.Editor, Devices> NOTHING = (e, d) -> { };

    private static BiConsumer<SharedPreferences.Editor, Devices> on(String key) {
        return (e, d) -> e.putBoolean(key, true);
    }

    private static BiConsumer<SharedPreferences.Editor, Devices> set(String key, String value) {
        return (e, d) -> e.putString(key, value);
    }

    private static BiConsumer<SharedPreferences.Editor, Devices> number(String key, int value) {
        return (e, d) -> e.putInt(key, value);
    }

    private static final List<Row> ROWS = Arrays.asList(
            new Row("seekbar_mouse_speed", "input", true, NOTHING, (e, d) -> d.mouse = true, number("seekbar_mouse_speed", 150)),
            new Row("checkbox_mouse_nav_buttons", "input", true, NOTHING, (e, d) -> d.mouse = true, on("checkbox_mouse_nav_buttons")),
            // The touch rows act in any touch mode (an external display, the stream menu, the second screen): only a
            // device without a touch screen leaves them out.
            new Row("seekbar_trackpad_speed", "input", true, NO_TOUCH_SCREEN, TOUCH_SCREEN, untouchable(number("seekbar_trackpad_speed", 150))),
            new Row("seekbar_local_cursor_speed", "input", true, NO_TOUCH_SCREEN, TOUCH_SCREEN, untouchable(number("seekbar_local_cursor_speed", 150))),
            new Row(SettingRules.ACCELERATION_CURVE, "input", true, NO_TOUCH_SCREEN, TOUCH_SCREEN, untouchable(set(SettingRules.ACCELERATION_CURVE, "strong"))),
            new Row(SettingRules.MOMENTUM_GLIDE, "input", true, NO_TOUCH_SCREEN, TOUCH_SCREEN, untouchable(set(SettingRules.MOMENTUM_GLIDE, "long"))),
            new Row("list_trackpad_area", "input", true, NO_TOUCH_SCREEN, TOUCH_SCREEN, untouchable(set("list_trackpad_area", "right"))),
            new Row("checkbox_multi_touch_finger_taps", "input", true, NO_TOUCH_SCREEN, TOUCH_SCREEN, untouchable(on("checkbox_multi_touch_finger_taps"))),
            new Row("checkbox_finger_tap_on_cancel", "input", true, NO_TOUCH_SCREEN, TOUCH_SCREEN, untouchable(on("checkbox_finger_tap_on_cancel"))),
            new Row("checkbox_pinch_to_zoom", "input", true, NO_TOUCH_SCREEN, TOUCH_SCREEN, untouchable(on("checkbox_pinch_to_zoom"))),
            new Row("seekbar_trackpad_sensitivity_x", "input", true, NOTHING, (e, d) -> d.touchpad = true, number("seekbar_trackpad_sensitivity_x", 150)),
            new Row("seekbar_trackpad_sensitivity_y", "input", true, NOTHING, (e, d) -> d.touchpad = true, number("seekbar_trackpad_sensitivity_y", 150)),
            new Row("checkbox_trackpad_swap_axis", "input", true, NOTHING, (e, d) -> d.touchpad = true, on("checkbox_trackpad_swap_axis")),
            new Row("checkbox_native_touchpad", "input", true, NOTHING, (e, d) -> d.touchpad = true, on("checkbox_native_touchpad")),
            new Row("checkbox_back_as_meta", "input", true, NOTHING, (e, d) -> d.keyboard = true, on("checkbox_back_as_meta")),
            new Row("performance_overlay_position", "streaming", NOTHING, (e, d) -> OverlayChoice.save(e, 1),
                    set("performance_overlay_position", "bottom_right")),
            new Row("list_resolution_screen2", "streaming", NOTHING, on("checkbox_dual_screen"), set("list_resolution_screen2", "1280x720")),
            new Row("checkbox_covering_decoder_size", "streaming", NOTHING, set(com.limelight.ui.StreamOrientation.PREF, "rotate"),
                    on("checkbox_covering_decoder_size")),
            new Row("checkbox_experimental_chipset_tuning", "advanced", true, NOTHING, (e, d) -> d.tunedChip = true,
                    on("checkbox_experimental_chipset_tuning")),
            new Row("checkbox_tv_display_compatibility", "advanced", true, NOTHING, (e, d) -> d.television = true,
                    on("checkbox_tv_display_compatibility")),
            new Row("game_info_igdb_client_secret", "advanced", NOTHING,
                    (e, d) -> gameInfoKeys().edit().putString(com.limelight.console.GameInfo.KEY_IGDB_ID, "id").commit(),
                    (e, d) -> gameInfoKeys().edit().putString(com.limelight.console.GameInfo.KEY_IGDB_SECRET, "secret").commit()));

    private void reset(BiConsumer<SharedPreferences.Editor, Devices> state) {
        prefs().edit().clear().commit();
        gameInfoKeys().edit().clear().commit();
        devices.touchpad = devices.mouse = devices.keyboard = devices.folds = devices.tunedChip = devices.television = false;
        devices.touchscreen = true;
        SharedPreferences.Editor editor = prefs().edit();
        state.accept(editor, devices);
        editor.commit();
    }

    @Test
    public void everyWhenRowIsTested() {
        List<String> tested = new ArrayList<>();
        for (Row row : ROWS) tested.add(row.key);
        List<String> rows = new ArrayList<>(SettingRules.WHEN_ROWS);
        // Removed on a device without amplitude control, as Robolectric's vibrator is; its rule is Vibrate device for rumble.
        rows.remove("seekbar_vibrate_fallback_strength");
        assertEquals(rows.size(), tested.size());
        assertTrue(tested.containsAll(rows));
    }

    @Test
    public void aRowThatCanDoNothingLeavesTheListAndComesBackWhenItCan() {
        try (ActivityController<StreamSettings> controller = start()) {
            StreamSettings screen = controller.get();
            for (Row row : ROWS) {
                reset(row.off);
                StreamSettings.SettingsFragment fragment = open(screen, row.section);
                assertNotNull(row.key, fragment.findPreference(row.key));
                assertFalse(row.key + " does nothing now", visible(fragment).contains(row.key));
                // The IGDB client ID is set in its own dialog (anIgdbClientIdSetInItsDialogShowsTheSecretAtOnce).
                if (row.key.equals(SettingRules.IGDB_SECRET)) continue;
                // Without leaving the screen: the setting it follows changes (its listener), or the device is attached.
                SharedPreferences.Editor editor = prefs().edit();
                row.on.accept(editor, devices);
                editor.commit();
                if (row.device) fragment.devicesChanged();
                settle(screen);
                assertTrue(row.key + " acts now", visible(fragment).contains(row.key));
            }
        }
    }

    @Test
    public void aRowHoldingAChangedValueStaysAndSaysWhy() {
        try (ActivityController<StreamSettings> controller = start()) {
            StreamSettings screen = controller.get();
            for (Row row : ROWS) {
                reset(row.off);
                SharedPreferences.Editor editor = prefs().edit();
                row.changed.accept(editor, devices);
                editor.commit();
                StreamSettings.SettingsFragment fragment = open(screen, row.section);
                assertTrue(row.key + " holds a changed value", visible(fragment).contains(row.key));
                String reason = SettingRules.whenReason(context(), row.key, prefs(), devices);
                assertNotNull(row.key + " still does nothing", reason);
                CharSequence summary = fragment.findPreference(row.key).getSummary();
                assertTrue(row.key + ": " + summary, summary != null && summary.toString().contains(reason));
            }
        }
    }

    @Test
    public void searchFindsARowThatDoesNothingAndOpeningItShowsIt() {
        try (ActivityController<StreamSettings> controller = start()) {
            StreamSettings screen = controller.get();
            StreamSettings.SettingsFragment results = search(screen, "Trackpad sensitivity X");
            Preference found = result(results, "seekbar_trackpad_sensitivity_x");
            assertNotNull("found without a touchpad", found);
            assertTrue(String.valueOf(found.getSummary()),
                    String.valueOf(found.getSummary()).endsWith(context().getString(R.string.tag_unused_no_touchpad)));
            found.getOnPreferenceClickListener().onPreferenceClick(found);
            settle(screen);
            assertTrue("shown for this visit", visible((StreamSettings.SettingsFragment) shown(screen)).contains("seekbar_trackpad_sensitivity_x"));
        }
    }

    @Test
    public void aRowSetBackToItsDefaultStaysForThisVisit() {
        try (ActivityController<StreamSettings> controller = start()) {
            StreamSettings screen = controller.get();
            reset(on("checkbox_trackpad_swap_axis"));
            StreamSettings.SettingsFragment fragment = open(screen, "input");
            assertTrue(visible(fragment).contains("checkbox_trackpad_swap_axis"));
            prefs().edit().putBoolean("checkbox_trackpad_swap_axis", false).commit();
            settle(screen);
            assertTrue("the edit keeps its row", visible(fragment).contains("checkbox_trackpad_swap_axis"));
        }
        try (ActivityController<StreamSettings> controller = start()) {
            assertFalse("gone on the next visit", visible(open(controller.get(), "input")).contains("checkbox_trackpad_swap_axis"));
        }
    }

    @Test
    public void theFoldedTrackpadRowsLeaveWithoutATouchScreenUnlessChanged() {
        try (ActivityController<StreamSettings> controller = start()) {
            StreamSettings screen = controller.get();
            // Trackpad acceleration and Glide after a flick are one row each, Off among their values.
            reset((e, d) -> TouchMode.save(e, TouchMode.DIRECT));
            StreamSettings.SettingsFragment fragment = open(screen, "input");
            assertTrue(visible(fragment).containsAll(Arrays.asList(SettingRules.ACCELERATION_CURVE,
                    "seekbar_trackpad_speed", SettingRules.MOMENTUM_GLIDE)));
            // No touch screen: both leave, unless changed; a changed Glide stays, saying why.
            prefs().edit().putString(SettingRules.MOMENTUM_GLIDE, "long").commit();
            devices.touchscreen = false;
            fragment.devicesChanged();
            settle(screen);
            List<String> rows = visible(fragment);
            assertFalse(rows.contains(SettingRules.ACCELERATION_CURVE));
            assertTrue("changed, so it stays", rows.contains(SettingRules.MOMENTUM_GLIDE));
            assertTrue(String.valueOf(fragment.findPreference(SettingRules.MOMENTUM_GLIDE).getSummary())
                    .contains(context().getString(R.string.tag_unused_no_touchscreen)));
            prefs().edit().putString(SettingRules.ACCELERATION_CURVE, "strong").commit();
            settle(screen);
            assertTrue(visible(fragment).contains(SettingRules.ACCELERATION_CURVE));
        }
    }

    /**
     * The rows the stream menu or a PC's, game's or profile's settings can make act are always shown, and say why while
     * the main settings leave them nothing to do; Thermal protection keeps its own tag. Settings reads no PC's, game's
     * or profile's values to decide this.
     */
    @Test
    public void rowsAStreamOrAPcCanMakeActAreAlwaysShownWithTheirReason() {
        prefs().edit().putBoolean(SettingRules.SHOW_DEBUG, true).commit();
        String[][] rows = {
                {"video", "adaptive_quality_min_resolution", "adaptive_quality_mode", "motion"},
                {"video", "adaptive_quality_min_fps", "adaptive_quality_mode", "motion"},
                {"display", "upscaling_sharpness", "upscaling_mode", "fsr1"},
                {"streaming", PreferenceConfiguration.COMPACT_OVERLAY_DEVICE_INFO_PREF_STRING,
                        PreferenceConfiguration.ENABLE_PERF_OVERLAY_STRING, "bool:true"},
                {"debug", SettingRules.PYROWAVE_PREACQUIRE, "video_format", "forcepyrowave"}};
        assertEquals(rows.length, SettingRules.NOTE_ROWS.size());
        try (ActivityController<StreamSettings> controller = start()) {
            StreamSettings screen = controller.get();
            assertTrue(visible(open(screen, "video")).contains(PreferenceConfiguration.THERMAL_PROTECTION_PREF_STRING));
            for (String[] row : rows) {
                assertTrue(row[1], SettingRules.NOTE_ROWS.contains(row[1]));
                StreamSettings.SettingsFragment fragment = open(screen, row[0]);
                assertTrue(row[1] + " is shown", visible(fragment).contains(row[1]));
                String reason = SettingRules.whenReason(context(), row[1], prefs(), devices);
                assertNotNull(row[1], reason);
                CharSequence summary = fragment.findPreference(row[1]).getSummary();
                assertTrue(row[1] + ": " + summary, summary != null && summary.toString().contains(reason));
                // Its mode comes on: still shown, without the reason.
                if ("bool:true".equals(row[3])) {
                    prefs().edit().putBoolean(row[2], true).commit();
                } else {
                    prefs().edit().putString(row[2], row[3]).commit();
                }
                settle(screen);
                assertTrue(visible(fragment).contains(row[1]));
                assertNull(row[1], SettingRules.whenReason(context(), row[1], prefs(), devices));
                prefs().edit().remove(row[2]).commit();
                settle(screen);
            }
        }
    }

    @Test
    public void aMouseAttachedOrRemovedShowsOrHidesItsRowsThroughTheListener() throws Exception {
        android.hardware.input.InputManager input =
                (android.hardware.input.InputManager) context().getSystemService(Context.INPUT_SERVICE);
        ActivityController<StreamSettings> built = Robolectric.buildActivity(StreamSettings.class);
        built.get().savedPcReader = task -> { };
        // The device's own answers (SettingDevices.of), not a stand-in.
        try (ActivityController<StreamSettings> controller = built.setup()) {
            StreamSettings screen = controller.get();
            StreamSettings.SettingsFragment fragment = open(screen, "input");
            assertFalse(visible(fragment).contains("checkbox_mouse_nav_buttons"));
            // Android 9 has no InputDevice.Builder: a named device given a mouse's source and an ID.
            android.view.InputDevice mouse = org.robolectric.shadows.ShadowInputDevice.makeInputDeviceNamed("Mouse");
            org.robolectric.util.ReflectionHelpers.setField(mouse, "mId", 41);
            org.robolectric.util.ReflectionHelpers.setField(mouse, "mSources", android.view.InputDevice.SOURCE_MOUSE);
            shadowOf(input).addInputDevice(mouse);
            notifyListeners(input, 41, true);
            settle(screen);
            assertTrue("attached", visible(fragment).contains("checkbox_mouse_nav_buttons"));
            removeDevice(input, 41);
            notifyListeners(input, 41, false);
            settle(screen);
            assertFalse("removed", visible(fragment).contains("checkbox_mouse_nav_buttons"));
            // Paused, the screen no longer listens.
            controller.pause();
            assertEquals(0, listeners(input).size());
        }
    }

    /** InputManager's registered listeners, as Android calls them when a device comes or goes. */
    private static List<android.hardware.input.InputManager.InputDeviceListener> listeners(android.hardware.input.InputManager input)
            throws Exception {
        java.lang.reflect.Field delegates = android.hardware.input.InputManager.class.getDeclaredField("mInputDeviceListeners");
        delegates.setAccessible(true);
        List<android.hardware.input.InputManager.InputDeviceListener> found = new ArrayList<>();
        for (Object delegate : (List<?>) delegates.get(input)) {
            java.lang.reflect.Field listener = delegate.getClass().getDeclaredField("mListener");
            listener.setAccessible(true);
            found.add((android.hardware.input.InputManager.InputDeviceListener) listener.get(delegate));
        }
        return found;
    }

    private static void notifyListeners(android.hardware.input.InputManager input, int id, boolean added) throws Exception {
        List<android.hardware.input.InputManager.InputDeviceListener> registered = listeners(input);
        assertFalse("the screen listens for devices", registered.isEmpty());
        for (android.hardware.input.InputManager.InputDeviceListener listener : registered) {
            if (added) listener.onInputDeviceAdded(id);
            else listener.onInputDeviceRemoved(id);
        }
    }

    /** Takes a device out of InputManager's list, as Android does when it is unplugged. */
    private static void removeDevice(android.hardware.input.InputManager input, int id) throws Exception {
        java.lang.reflect.Field devices = android.hardware.input.InputManager.class.getDeclaredField("mInputDevices");
        devices.setAccessible(true);
        ((android.util.SparseArray<?>) devices.get(input)).remove(id);
    }

    @Test
    public void anIgdbClientIdSetInItsDialogShowsTheSecretAtOnce() {
        try (ActivityController<StreamSettings> controller = start()) {
            StreamSettings screen = controller.get();
            StreamSettings.SettingsFragment fragment = open(screen, "advanced");
            assertFalse(visible(fragment).contains(SettingRules.IGDB_SECRET));
            Preference id = fragment.findPreference("game_info_igdb_client_id");
            id.getOnPreferenceClickListener().onPreferenceClick(id);
            androidx.appcompat.app.AlertDialog dialog =
                    (androidx.appcompat.app.AlertDialog) org.robolectric.shadows.ShadowDialog.getLatestDialog();
            android.widget.EditText field = findEditText(dialog.getWindow().getDecorView());
            field.setText("client");
            dialog.getButton(android.content.DialogInterface.BUTTON_POSITIVE).performClick();
            settle(screen);
            assertTrue("the secret can act now", visible(fragment).contains(SettingRules.IGDB_SECRET));
        }
    }

    private static android.widget.EditText findEditText(android.view.View view) {
        if (view instanceof android.widget.EditText) return (android.widget.EditText) view;
        if (view instanceof android.view.ViewGroup) {
            android.view.ViewGroup group = (android.view.ViewGroup) view;
            for (int i = 0; i < group.getChildCount(); i++) {
                android.widget.EditText found = findEditText(group.getChildAt(i));
                if (found != null) return found;
            }
        }
        return null;
    }

    @Test
    public void chipsetTuningIsLeftOutOnlyOnAChipKnownNotToBeTuned() {
        assertTrue(SettingDevices.tunedChip("Mediatek", "mt6893"));
        assertTrue(SettingDevices.tunedChip("Samsung", "s5e9925"));
        assertTrue("Tensor may name its decoders c2.exynos", SettingDevices.tunedChip("Google", "gs201"));
        assertFalse(SettingDevices.tunedChip("QTI", "qcom"));
        assertTrue("before Android 12, by its hardware name", SettingDevices.tunedChip(null, "exynos2100"));
        assertFalse(SettingDevices.tunedChip(null, "qcom"));
        assertTrue("an unknown chip may be one", SettingDevices.tunedChip(null, "something"));
    }

    @Test
    public void theMoreCountLeavesOutRowsThatDoNothing() {
        try (ActivityController<StreamSettings> controller = start()) {
            StreamSettings screen = controller.get();
            StreamSettings.SettingsFragment fragment = open(screen, "input");
            screen.moreOpen.remove("input");
            fragment.devicesChanged();
            String without = String.valueOf(fragment.findPreference("more_input").getTitle());
            devices.touchpad = true;
            fragment.devicesChanged();
            String with = String.valueOf(fragment.findPreference("more_input").getTitle());
            assertNotEquals("a touchpad adds its rows to the count", without, with);
        }
    }

    @Test
    public void aGamesFullColorRangeRowIsAlwaysListedWithTheImageRows() {
        Intent intent = HostStreamSettings.intentForGame(context(), "pc", "PC", "game-uuid", 7, "Racer");
        for (boolean set : new boolean[] {false, true}) {
            AppStreamSettings.save(context(), "pc", "game-uuid", 7, HostStreamProfile.deserialize(set ? "v2;full_range=true" : "v2"));
            try (ActivityController<HostStreamSettings> controller = Robolectric.buildActivity(HostStreamSettings.class, intent).setup()) {
                PreferenceFragment fragment = (PreferenceFragment) controller.get().getFragmentManager().findFragmentById(R.id.host_profile_settings);
                Preference fullRange = fragment.findPreference("full_range");
                assertNotNull(fullRange);
                assertSame(fragment.findPreference("yuv444").getParent(), fullRange.getParent());
                assertNotNull("always: a layer above may choose an upscaler that sharpens", fragment.findPreference("sharpness"));
            }
        }
    }

}
