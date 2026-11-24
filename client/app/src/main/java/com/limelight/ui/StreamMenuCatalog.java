package com.limelight.ui;

import android.app.Activity;
import android.content.Context;
import android.content.SharedPreferences;

import androidx.core.view.ViewCompat;

import com.google.android.material.button.MaterialButton;
import com.limelight.LimeLog;
import com.limelight.R;
import com.limelight.binding.video.PerformanceOverlay;
import com.limelight.nativeapi.NativeAudio;
import com.limelight.nativeapi.NativeCapabilities;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.http.NvApp;
import com.limelight.ui.StreamMenu.Entry;
import com.limelight.ui.StreamMenu.Item;
import com.limelight.ui.StreamMenu.Options;
import com.limelight.ui.StreamMenu.Page;
import com.limelight.ui.StreamMenu.Section;

import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collections;
import java.util.HashMap;
import java.util.List;
import java.util.Locale;
import java.util.Map;

/**
 * Pure entry catalog, label mapping, header formatting, and controller-button preference helpers
 * for {@link StreamMenu}.
 */
final class StreamMenuCatalog {
    private StreamMenuCatalog() { }

    /**
     * The ways out under the list when the model cannot answer, as its footer has them: Disconnect and what ends the game (or a
     * remote monitor's or input-only stream's End). End session, which leaves the game running, is a row as on More.
     */
    static final List<String> FALLBACK_FOOTER = Collections.unmodifiableList(Arrays.asList(
            id(Item.DISCONNECT), id(Item.CLOSE_GAME), id(Item.QUIT_GAME), id(Item.END_SESSION)));

    static int title(NvApp.Role role) {
        return role == NvApp.Role.REMOTE_MONITOR ? R.string.stream_monitor_title :
                role == NvApp.Role.INPUT_ONLY ? R.string.stream_input_title : R.string.stream_menu;
    }

    static List<Entry> mainPage(Options o) {
        List<Entry> entries = new ArrayList<>();
        ComputerDetails host = o.host;
        HostCapabilities caps = host.caps;

        // The keyboards open and close the menu: actions, not switches. The PC keyboard is the full
        // layout drawn here; Keyboard is Android's own.
        if (o.canSendInput) entries.add(action(Section.QUICK, Item.KEYBOARD, R.string.desktop_controls_system_keyboard, R.drawable.ic_keyboard));
        if (o.canSendInput) entries.add(action(Section.QUICK, Item.PC_KEYBOARD, R.string.pc_keyboard_title, R.drawable.ic_keyboard_keys));
        if (o.mediaStream()) {
            entries.add(entry(Section.QUICK, Item.OVERLAY, R.string.stream_overlay_menu, R.drawable.ic_overlay, o.overlayShown));
        }
        if (o.microphoneOffered) {
            entries.add(entry(Section.QUICK, Item.MICROPHONE, R.string.stream_microphone,
                    o.microphoneMuted ? R.drawable.ic_mic_off : R.drawable.ic_mic, !o.microphoneMuted));
        }
        // The host's audio processing for this stream, once the host has said it may change now.
        if (o.role == NvApp.Role.STREAM && o.hostAudio != null) {
            NativeAudio.Setting night = o.hostAudio.get(NativeCapabilities.NIGHT_MODE);
            if (o.nightModeOffered && night != null && night.switchable()) {
                entries.add(entry(Section.QUICK, Item.NIGHT_MODE, R.string.stream_night_mode, R.drawable.ic_night_mode, night.value));
            }
            NativeAudio.Setting surround = o.hostAudio.get(NativeCapabilities.HEADPHONE_SURROUND);
            if (o.headphoneSurroundOffered && surround != null && surround.switchable()) {
                entries.add(entry(Section.QUICK, Item.HEADPHONE_SURROUND, R.string.stream_headphone_surround,
                        R.drawable.ic_headphones, surround.value));
            }
        }

        addSession(entries, o);

        if (o.canSendInput) {
            if (o.localOutput) entries.add(action(Section.INPUT, Item.TOUCH_MODE, R.string.stream_touch_mode, R.drawable.ic_touch));
            if (o.mouseConnected) {
                entries.add(action(Section.INPUT, Item.MOUSE_CURSOR, o.cursorVisible ?
                        R.string.stream_cursor_hide : R.string.stream_cursor_show, R.drawable.ic_mouse_pointer));
            }
            if (o.controllerMouseAvailable) {
                entries.add(entry(Section.INPUT, Item.CONTROLLER_MOUSE, R.string.stream_controller_mouse,
                        R.drawable.ic_gamepad, o.controllerMouseOn));
            }
            entries.add(entry(Section.INPUT, Item.ONSCREEN_CONTROLS, o.onscreenControls ?
                    R.string.stream_controls_hide : R.string.stream_controls_show, R.drawable.ic_gamepad, o.onscreenControls));
            if (o.onscreenControls) {
                entries.add(action(Section.INPUT, Item.CONTROLS_LAYOUT, R.string.stream_controls_layout, R.drawable.ic_edit));
            }
            if (o.microphoneOffered && o.micButtonAvailable) {
                entries.add(action(Section.INPUT, Item.MIC_BUTTON, o.micButtonShown ?
                        R.string.stream_mic_button_hide : R.string.stream_mic_button_show, R.drawable.ic_mic_button));
            }
            if (o.menuButtonAvailable) {
                entries.add(action(Section.INPUT, Item.MENU_BUTTON, o.menuButtonShown ?
                        R.string.stream_menu_button_hide : R.string.stream_menu_button_show, R.drawable.ic_menu_button));
            }
            if (o.gameControls) {
                entries.add(action(Section.INPUT, Item.GAME_CONTROLS, R.string.touch_layout_title, R.drawable.ic_touch_controls));
            }
            entries.add(action(Section.INPUT, Item.KEYBOARD_KEYS, o.keyboardKeysShown ?
                    R.string.desktop_controls_close : R.string.desktop_controls_open, R.drawable.ic_keys));
        }

        if (o.mediaStream()) {
            if (o.scalingAvailable && o.localOutput) {
                entries.add(action(Section.DISPLAY, Item.VIDEO_SCALING, R.string.video_scaling_title, R.drawable.ic_stretch));
            }
            if (o.localOutput) {
                entries.add(entry(Section.DISPLAY, Item.ZOOM, o.zoomEnabled ?
                        R.string.desktop_zoom_disable : R.string.desktop_zoom_enable, R.drawable.ic_zoom_in, o.zoomEnabled));
                if (o.zoomEnabled) {
                    entries.add(action(Section.DISPLAY, Item.ZOOM_RESET, R.string.desktop_zoom_reset, R.drawable.ic_zoom_out));
                }
            }
            entries.add(action(Section.DISPLAY, Item.OVERLAY_PAGE, R.string.stream_overlay_page, R.drawable.ic_chart));
            if (o.pictureInPicture && o.localOutput) {
                entries.add(action(Section.DISPLAY, Item.PICTURE_IN_PICTURE, R.string.stream_picture_in_picture, R.drawable.ic_picture_in_picture));
            }
            if (o.externalDisplays || !o.localOutput) {
                entries.add(action(Section.DISPLAY, Item.VIDEO_OUTPUT, R.string.desktop_output, R.drawable.ic_monitor));
            }

            // Bitrate and automatic quality change the host's encoder: they need its runtime bitrate
            // control and permission to view and launch. A host that offered this stream 0x553C
            // changes the bitrate and the frame rate in one dialog instead.
            boolean runtimeBitrate = caps.has(HostCapabilities.Feature.RUNTIME_BITRATE) &&
                    host.hasPermission(ComputerDetails.PERMISSION_VIEW | ComputerDetails.PERMISSION_LAUNCH);
            if (o.rateChange && o.role == NvApp.Role.STREAM) {
                entries.add(action(Section.QUALITY, Item.RATE_CHANGE, R.string.stream_rate_change, R.drawable.ic_speed));
            } else if (runtimeBitrate) {
                entries.add(action(Section.QUALITY, Item.BITRATE, R.string.stream_bitrate_manual, R.drawable.ic_speed));
            }
            if (runtimeBitrate && o.automaticQuality && o.role == NvApp.Role.STREAM) {
                entries.add(action(Section.QUALITY, Item.AUTOMATIC_QUALITY, R.string.title_adaptive_quality, R.drawable.ic_tune));
            }
            if ((runtimeBitrate && host.encoderBitrateSupported) || o.rateChangeBitrate) {
                // The setting's own name, checked while it is on, as desktop's switch shows it.
                entries.add(entry(Section.QUALITY, Item.AUTOMATIC_BITRATE, R.string.title_auto_bitrate,
                        R.drawable.ic_network, o.automaticBitrateOn));
            }
        }

        // A native API host adds its own page: the session actions, the connection and the Radeon settings.
        if (o.role == NvApp.Role.STREAM && caps.has(HostCapabilities.Feature.NATIVE_API)) {
            entries.add(action(Section.HOST, Item.NATIVE_PAGE, R.string.native_menu_row, R.drawable.ic_tune));
        }
        // Only when the host's native capabilities offer it, whoever made the host.
        if (o.role == NvApp.Role.STREAM && o.hostMenu) {
            entries.add(action(Section.HOST, Item.HOST_MENU, R.string.stream_host_menu, R.drawable.ic_computer));
        }
        if (o.role != NvApp.Role.REMOTE_MONITOR && host.canRunServerCommand(0)) {
            entries.add(action(Section.HOST, Item.SERVER_COMMANDS, R.string.stream_server_commands, R.drawable.ic_terminal));
        }
        if (o.role != NvApp.Role.REMOTE_MONITOR && host.canWriteClipboard()) {
            entries.add(action(Section.HOST, Item.CLIPBOARD_SEND, R.string.stream_clipboard_send, R.drawable.ic_clipboard_send));
        }
        if (host.canReadClipboard()) {
            entries.add(action(Section.HOST, Item.CLIPBOARD_RECEIVE, R.string.stream_clipboard_receive, R.drawable.ic_clipboard_receive));
        }
        // Images ride the same permissions as text, on a host that offers them.
        if (o.imageClipboard && o.role != NvApp.Role.REMOTE_MONITOR && host.canWriteClipboard()) {
            entries.add(action(Section.HOST, Item.IMAGE_CLIPBOARD_SEND, R.string.stream_clipboard_image_send, R.drawable.ic_image_send));
        }
        if (o.imageClipboard && host.canReadClipboard()) {
            entries.add(action(Section.HOST, Item.IMAGE_CLIPBOARD_RECEIVE, R.string.stream_clipboard_image_receive, R.drawable.ic_image_receive));
        }
        if (o.role != NvApp.Role.REMOTE_MONITOR && o.usbForwarding) {
            entries.add(action(Section.HOST, Item.USB_DEVICES, R.string.stream_usb_devices, R.drawable.ic_usb));
        }
        // Only the device playing (or one with Host control) manages who joined its game.
        if (o.role == NvApp.Role.STREAM && o.sessionGuests) {
            entries.add(action(Section.HOST, Item.GUESTS, R.string.guests_title, R.drawable.ic_guests));
        }
        // File transfer belongs to the PC, not the session; its screen says whether this device may use it.
        if (caps.has(HostCapabilities.Feature.NATIVE_API)) {
            entries.add(action(Section.HOST, Item.FILES, R.string.files_menu, R.drawable.ic_files));
        }
        if (o.webUiAddress && caps.mayHave(HostCapabilities.Feature.WEB_UI)) {
            entries.add(action(Section.HOST, Item.HOST_WEB_UI, R.string.host_administration_menu, R.drawable.ic_web));
        }
        // Without device permissions or a frame limiter the host has no status to show.
        if (host.permission != -1 || caps.has(HostCapabilities.Feature.FRAME_LIMITER_STATUS)) {
            entries.add(action(Section.HOST, Item.HOST_STATUS, R.string.stream_host_status, R.drawable.ic_info));
        }

        return entries;
    }

    /**
     * Disconnect first, right under Return to stream; the actions that end the PC's game or session close the group in the
     * error colour. Under the grid stand Disconnect and the game's end; Reconnect and End session wait on the More page.
     */
    private static void addSession(List<Entry> entries, Options o) {
        ComputerDetails host = o.host;
        HostCapabilities caps = host.caps;
        entries.add(action(Section.SESSION, Item.DISCONNECT, R.string.stream_disconnect, R.drawable.ic_close));
        entries.add(action(Section.SESSION, Item.RECONNECT, R.string.stream_reconnect, R.drawable.ic_refresh));
        if (host.hasPermission(ComputerDetails.PERMISSION_LAUNCH)) {
            if (o.role == NvApp.Role.STREAM && caps.has(HostCapabilities.Feature.NATIVE_API)) {
                // The native API tells ending the session (the game keeps running) from closing the game.
                entries.add(new Entry(Section.SESSION, Item.END_HOST_SESSION, R.string.stream_end_session, R.drawable.ic_stop, null, true));
                // Close game needs host control, which is off by default; Quit app (/cancel) does not.
                entries.add(o.nativeHostControl
                        ? new Entry(Section.SESSION, Item.CLOSE_GAME, R.string.stream_close_game, R.drawable.ic_stop, null, true)
                        : new Entry(Section.SESSION, Item.QUIT_GAME, R.string.stream_quit_game, R.drawable.ic_stop, null, true));
            } else if (o.role == NvApp.Role.STREAM) {
                entries.add(new Entry(Section.SESSION, Item.QUIT_GAME, R.string.stream_quit_game, R.drawable.ic_stop, null, true));
            } else {
                entries.add(new Entry(Section.SESSION, Item.END_SESSION, o.role == NvApp.Role.REMOTE_MONITOR ?
                        R.string.stream_end_monitor : R.string.stream_end_input, R.drawable.ic_stop, null, true));
            }
        }
    }

    static Item firstFocus(List<Entry> entries) {
        for (Entry entry : entries) {
            if (entry.item != Item.DISCONNECT && !entry.destructive) return entry.item;
        }
        return entries.isEmpty() ? null : entries.get(0).item;
    }

    static List<Entry> nativePage(Options o) {
        List<Entry> entries = new ArrayList<>();
        entries.add(action(Section.NATIVE_SESSION, Item.NATIVE_RESUME, R.string.native_resume, R.drawable.ic_play));
        entries.add(action(Section.NATIVE_SESSION, Item.DISCONNECT, R.string.stream_disconnect, R.drawable.ic_close));
        if (o.host.hasPermission(ComputerDetails.PERMISSION_LAUNCH)) {
            entries.add(new Entry(Section.NATIVE_SESSION, Item.END_HOST_SESSION, R.string.stream_end_session, R.drawable.ic_stop, null, true));
            entries.add(o.nativeHostControl
                    ? new Entry(Section.NATIVE_SESSION, Item.CLOSE_GAME, R.string.stream_close_game, R.drawable.ic_stop, null, true)
                    : new Entry(Section.NATIVE_SESSION, Item.QUIT_GAME, R.string.stream_quit_game, R.drawable.ic_stop, null, true));
        }
        return entries;
    }

    static List<Entry> overlayPage(Options o) {
        List<Entry> entries = new ArrayList<>();
        // Off first, like the settings picker; a mode counts as chosen only while the overlay shows.
        entries.add(entry(Section.OVERLAY_STYLE, Item.OVERLAY_OFF, R.string.overlay_off, R.drawable.ic_check, !o.overlayShown));
        String[] modes = {PerformanceOverlay.MODE_COMPACT, PerformanceOverlay.MODE_STATISTICS, PerformanceOverlay.MODE_DEBUG};
        Item[] items = {Item.OVERLAY_COMPACT, Item.OVERLAY_STATISTICS, Item.OVERLAY_DEBUG};
        int[] labels = {R.string.overlay_compact, R.string.overlay_statistics, R.string.overlay_debug};
        for (int i = 0; i < modes.length; i++) {
            entries.add(entry(Section.OVERLAY_STYLE, items[i], labels[i], R.drawable.ic_check,
                    o.overlayShown && modes[i].equals(o.overlayMode)));
        }
        OverlayPlacement.Corner[] corners = OverlayPlacement.Corner.values();
        Item[] positions = {Item.OVERLAY_TOP_LEFT, Item.OVERLAY_TOP_RIGHT, Item.OVERLAY_BOTTOM_LEFT, Item.OVERLAY_BOTTOM_RIGHT};
        int[] positionLabels = {R.string.overlay_position_top_left, R.string.overlay_position_top_right,
                R.string.overlay_position_bottom_left, R.string.overlay_position_bottom_right};
        OverlayPlacement.Corner current = OverlayPlacement.Corner.parse(o.overlayPosition);
        for (int i = 0; i < corners.length; i++) {
            entries.add(entry(Section.OVERLAY_POSITION, positions[i], positionLabels[i], R.drawable.ic_check, corners[i] == current));
        }
        entries.add(action(Section.DIAGNOSTICS, Item.OVERLAY_COPY, R.string.overlay_copy, R.drawable.ic_copy));
        entries.add(action(Section.DIAGNOSTICS, Item.STREAM_HEALTH, R.string.health_title, R.drawable.ic_pulse));
        if (o.canSendInput) {
            entries.add(action(Section.DIAGNOSTICS, Item.LATENCY_TEST, R.string.latency_test, R.drawable.ic_timer));
        }
        return entries;
    }

    static Entry entry(Section section, Item item, int label, int icon, boolean checked) {
        return new Entry(section, item, label, icon, checked, false);
    }

    static Entry action(Section section, Item item, int label, int icon) {
        return new Entry(section, item, label, icon, null, false);
    }

    static List<Entry> pageEntries(Activity activity, Page target, Options o) {
        List<Entry> list = target == Page.MAIN ? mainPage(o) : target == Page.NATIVE ? nativePage(o) : overlayPage(o);
        if ((activity.getResources().getConfiguration().uiMode & android.content.res.Configuration.UI_MODE_TYPE_MASK)
                == android.content.res.Configuration.UI_MODE_TYPE_TELEVISION) {
            list.removeIf(entry -> entry.item == Item.TOUCH_MODE || entry.item == Item.ONSCREEN_CONTROLS ||
                    entry.item == Item.CONTROLS_LAYOUT || entry.item == Item.MIC_BUTTON || entry.item == Item.MENU_BUTTON ||
                    entry.item == Item.GAME_CONTROLS ||
                    entry.item == Item.ZOOM);
        }
        return list;
    }

    // The More page: its switches first, then the rest in the model's order.
    static List<Entry> moreEntries(List<Entry> offered) {
        List<Entry> switches = new ArrayList<>();
        List<Entry> rest = new ArrayList<>();
        StreamMenuModel.Layout model;
        try {
            model = StreamMenuModel.layout(offers(offered), null);
        } catch (StreamMenuModel.Refused e) {
            // Which entries sit behind More is the model's to say: without it, More lists them all.
            LimeLog.warning("Stream menu: the model failed for More: " + e.getMessage());
            return offered;
        }
        for (String id : model.more) {
            Entry entry = find(offered, id);
            if (entry == null) continue;
            if (entry.section == Section.QUICK) switches.add(entry);
            else rest.add(entry);
        }
        switches.addAll(rest);
        return switches;
    }

    static String id(Item item) {
        return item.name().toLowerCase(Locale.ROOT);
    }

    static Entry find(List<Entry> list, String id) {
        for (Entry entry : list) {
            if (id(entry.item).equals(id)) return entry;
        }
        return null;
    }

    static List<StreamMenuModel.Offer> offers(List<Entry> list) {
        List<StreamMenuModel.Offer> offers = new ArrayList<>();
        for (Entry entry : list) offers.add(new StreamMenuModel.Offer(id(entry.item), entry.checked));
        return offers;
    }

    static List<Item> items(List<Entry> list) {
        List<Item> items = new ArrayList<>();
        for (Entry entry : list) items.add(entry.item);
        return items;
    }

    // The model's cells that say something about the stream read better as rows with their value than as one more tile.
    static boolean isStatusRow(String id) {
        return id.equals(id(Item.BITRATE)) || id.equals(id(Item.RATE_CHANGE))
                || id.equals(id(Item.OVERLAY_PAGE)) || id.equals(id(Item.NATIVE_PAGE));
    }

    static int heading(Page target, Options o) {
        switch (target) {
            case NATIVE: return R.string.native_menu_row;
            case OVERLAY: return R.string.stream_overlay_page;
            case MORE: return R.string.stream_menu_more;
            case SHORTCUTS: return R.string.stream_menu_shortcuts;
            default: return title(o.role);
        }
    }

    /**
     * A tile's short name, the same as desktop Nightjar's: the {@code stream_tile_<key>} string for the key the shared
     * model gives its id. A tile the model names no label for is a disagreement between the two, which a test catches.
     */
    static String label(Activity activity, Map<String, String> labels, String id) {
        String key = labels.get(id);
        int label = key == null ? 0 : tileLabel(key);
        if (label == 0) throw new IllegalStateException("the stream menu model names no label for " + id + " (" + key + ")");
        return activity.getString(label);
    }

    static int tileLabel(String key) {
        switch (key) {
            case "keyboard": return R.string.stream_tile_keyboard;
            case "pc_keyboard": return R.string.stream_tile_pc_keyboard;
            case "mouse": return R.string.stream_tile_mouse;
            case "overlay": return R.string.stream_tile_overlay;
            case "shortcuts": return R.string.stream_tile_shortcuts;
            case "microphone": return R.string.stream_tile_microphone;
            case "touch_mode": return R.string.stream_tile_touch_mode;
            case "onscreen_controls": return R.string.stream_tile_onscreen_controls;
            case "quality": return R.string.stream_tile_quality;
            case "video_output": return R.string.stream_tile_video_output;
            case "zoom": return R.string.stream_tile_zoom;
            case "send_clipboard": return R.string.stream_tile_send_clipboard;
            case "get_clipboard": return R.string.stream_tile_get_clipboard;
            case "pc_commands": return R.string.stream_tile_pc_commands;
            case "diagnostics": return R.string.stream_tile_diagnostics;
            case "session_graphics": return R.string.stream_tile_session_graphics;
            case "more": return R.string.stream_tile_more;
            case "return": return R.string.stream_tile_return;
            case "add_shortcut": return R.string.stream_tile_add_shortcut;
            case "disconnect": return R.string.stream_tile_disconnect;
            case "end_session": return R.string.stream_tile_end_session;
            case "close_game": return R.string.stream_tile_close_game;
            case "quit_game": return R.string.stream_tile_quit_game;
            default: return 0;
        }
    }

    static int builtInLabel(String id) {
        switch (id) {
            case "task_manager": return R.string.keyboard_task_manager;
            case "switch_window": return R.string.keyboard_switch_window;
            case "close_window": return R.string.keyboard_close_window;
            case "start_menu": return R.string.stream_menu_start_menu;
            case "show_desktop": return R.string.desktop_controls_shortcut_desktop;
            case "game_bar": return R.string.keyboard_game_bar;
            case "record_that": return R.string.keyboard_record_that;
            default: throw new IllegalArgumentException("unknown built-in shortcut " + id);
        }
    }

    static String[] header(Activity activity, Options o) {
        String app = o.appName == null ? "" : o.appName.trim();
        if (o.role == NvApp.Role.STREAM && !app.isEmpty()) {
            String mode = null;
            try {
                mode = StreamMenuModel.modeLine(o.streamWidth, o.streamHeight, o.streamFps, o.videoFormat, o.hdr);
            } catch (StreamMenuModel.Refused e) {
                LimeLog.warning("Stream menu: the mode line failed: " + e.getMessage());
            }
            String host = o.host.name == null ? "" : o.host.name.trim();
            return new String[] {app, mode != null ? mode : host};
        }
        return new String[] {activity.getString(title(o.role)), subtitle(o)};
    }

    private static String subtitle(Options o) {
        String app = o.appName == null ? "" : o.appName.trim();
        String host = o.host.name == null ? "" : o.host.name.trim();
        if (app.isEmpty() && host.isEmpty()) return null;
        return app.isEmpty() ? host : host.isEmpty() ? app : app + " · " + host;
    }

    static Map<String, String> buttonChoices(Context context) {
        SharedPreferences prefs = android.preference.PreferenceManager.getDefaultSharedPreferences(context);
        Map<String, String> choices = new HashMap<>();
        for (String key : new String[] {"y", "x", "lb", "rb"}) {
            String choice = buttonChoice(prefs.getString("list_stream_menu_button_" + key, "default"));
            if (!"default".equals(choice)) choices.put(key, choice);
        }
        return choices;
    }

    static String buttonChoice(String saved) {
        if ("end_host_session".equals(saved)) return "default";
        return "close_game".equals(saved) ? "quit_game" : saved;
    }

    static void upgradeButtonChoices(SharedPreferences prefs) {
        SharedPreferences.Editor editor = null;
        for (String key : new String[] {"y", "x", "lb", "rb"}) {
            String name = "list_stream_menu_button_" + key;
            String saved = prefs.getString(name, null);
            if (saved != null && !saved.equals(buttonChoice(saved))) {
                if (editor == null) editor = prefs.edit();
                editor.putString(name, buttonChoice(saved));
            }
        }
        if (editor != null) editor.commit();
    }

    /** The controller button the user gave this way out under the grid, as its cap ({@code "Y"}), or null. */
    static String footerHotkey(StreamMenuModel.Layout layout, Item item) {
        if (layout == null) return null;
        // In the buttons' own order, as a tile shows the first button that names it.
        for (String key : new String[] {"y", "x", "lb", "rb"}) {
            if (id(item).equals(layout.buttons.get(key))) return key.toUpperCase(Locale.ROOT);
        }
        return null;
    }

    // A choice of several: a check on the chosen one, announced as selected.
    static void markChoice(MaterialButton button, boolean chosen) {
        if (!chosen) {
            button.setIconTint(android.content.res.ColorStateList.valueOf(android.graphics.Color.TRANSPARENT));
        }
        ViewCompat.setStateDescription(button, button.getContext().getString(chosen ?
                R.string.stream_choice_selected : R.string.stream_choice_not_selected));
    }
}
