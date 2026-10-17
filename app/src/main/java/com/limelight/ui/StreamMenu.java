package com.limelight.ui;

import android.app.Activity;
import android.widget.LinearLayout;

import androidx.core.view.ViewCompat;

import com.google.android.material.button.MaterialButton;
import com.limelight.R;
import com.limelight.binding.video.PerformanceOverlay;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.http.NvApp;

import java.util.ArrayList;
import java.util.EnumMap;
import java.util.List;
import java.util.Map;
import java.util.function.Supplier;

/**
 * The stream menu: one sheet with a row of quick toggles, then Input, Display, Quality and Host
 * sections, then the session actions. A page (overlay and diagnostics) opens in the same sheet, and
 * Back there returns to the first page. Items a host or this stream cannot use are left out.
 */
public final class StreamMenu {
    public enum Item {
        KEYBOARD, OVERLAY, MICROPHONE,
        TOUCH_MODE, MOUSE_CURSOR, ONSCREEN_CONTROLS, CONTROLS_LAYOUT, GAME_CONTROLS, KEYBOARD_KEYS,
        VIDEO_SCALING, ZOOM, ZOOM_RESET, OVERLAY_PAGE, PICTURE_IN_PICTURE, VIDEO_OUTPUT,
        BITRATE, AUTOMATIC_QUALITY, AUTOMATIC_BITRATE,
        SERVER_COMMANDS, CLIPBOARD_SEND, CLIPBOARD_RECEIVE, HOST_WEB_UI, HOST_STATUS,
        RECONNECT, DISCONNECT, QUIT_GAME, END_SESSION,
        OVERLAY_COMPACT, OVERLAY_STATISTICS, OVERLAY_DEBUG, OVERLAY_COPY, STREAM_HEALTH, LATENCY_TEST
    }

    public enum Section {
        QUICK(0), INPUT(R.string.stream_section_input), DISPLAY(R.string.stream_section_display),
        QUALITY(R.string.stream_section_quality), HOST(R.string.stream_section_host), SESSION(0),
        OVERLAY_STYLE(0), DIAGNOSTICS(0);

        public final int title;

        Section(int title) {
            this.title = title;
        }
    }

    public enum Page { MAIN, OVERLAY }

    /** What this stream and its host offer right now; the stream activity fills it each time the menu opens. */
    public static final class Options {
        public final NvApp.Role role;
        public final ComputerDetails host;
        public boolean canSendInput = true;
        public boolean microphoneOffered, microphoneMuted;
        public boolean overlayShown;
        public String overlayMode = PerformanceOverlay.MODE_COMPACT;
        /** False while video plays on an external display and this device is the touchpad. */
        public boolean localOutput = true;
        public boolean mouseConnected, cursorVisible;
        public boolean onscreenControls, gameControls, keyboardKeysShown;
        /** Stretching can change live: local MediaCodec video without an upscaler. */
        public boolean scalingAvailable;
        public boolean zoomEnabled;
        public boolean pictureInPicture;
        public boolean externalDisplays;
        /** The host can change resolution and frame rate in place, which automatic quality needs. */
        public boolean automaticQuality;
        public boolean automaticBitrateOn;
        /** The host's web UI address is known for this stream. */
        public boolean webUiAddress;

        public Options(NvApp.Role role, ComputerDetails host) {
            this.role = role;
            this.host = host;
        }

        boolean mediaStream() {
            return role != NvApp.Role.INPUT_ONLY;
        }
    }

    public static final class Entry {
        public final Section section;
        public final Item item;
        public final int label;
        public final int icon;
        /** For toggles and choices; null for plain actions. */
        public final Boolean checked;
        /** In the error colour: it ends the PC's game or session. */
        public final boolean destructive;

        Entry(Section section, Item item, int label, int icon, Boolean checked, boolean destructive) {
            this.section = section;
            this.item = item;
            this.label = label;
            this.icon = icon;
            this.checked = checked;
            this.destructive = destructive;
        }

        /** Quick toggles change in place; everything else closes the menu or opens a page. */
        public boolean keepsMenuOpen() {
            return item == Item.OVERLAY || item == Item.MICROPHONE;
        }

        public boolean opensPage() {
            return item == Item.OVERLAY_PAGE;
        }

        @Override
        public String toString() {
            return section + ":" + item;
        }
    }

    public static int title(NvApp.Role role) {
        return role == NvApp.Role.REMOTE_MONITOR ? R.string.stream_monitor_title :
                role == NvApp.Role.INPUT_ONLY ? R.string.stream_input_title : R.string.stream_menu;
    }

    public static List<Entry> mainPage(Options o) {
        List<Entry> entries = new ArrayList<>();
        ComputerDetails host = o.host;
        HostCapabilities caps = host.caps;

        // Keyboard opens Android's keyboard and closes the menu: an action, not a switch.
        if (o.canSendInput) entries.add(action(Section.QUICK, Item.KEYBOARD, R.string.stream_keyboard, R.drawable.ic_keyboard));
        if (o.mediaStream()) {
            entries.add(entry(Section.QUICK, Item.OVERLAY, R.string.stream_overlay_menu, R.drawable.ic_overlay, o.overlayShown));
        }
        if (o.microphoneOffered) {
            entries.add(entry(Section.QUICK, Item.MICROPHONE, R.string.stream_microphone,
                    o.microphoneMuted ? R.drawable.ic_mic_off : R.drawable.ic_mic, !o.microphoneMuted));
        }

        if (o.canSendInput) {
            if (o.localOutput) entries.add(action(Section.INPUT, Item.TOUCH_MODE, R.string.stream_touch_mode, R.drawable.ic_touch));
            if (o.mouseConnected) {
                entries.add(action(Section.INPUT, Item.MOUSE_CURSOR, o.cursorVisible ?
                        R.string.stream_cursor_hide : R.string.stream_cursor_show, R.drawable.ic_mouse_pointer));
            }
            entries.add(action(Section.INPUT, Item.ONSCREEN_CONTROLS, o.onscreenControls ?
                    R.string.stream_controls_hide : R.string.stream_controls_show, R.drawable.ic_gamepad));
            if (o.onscreenControls) {
                entries.add(action(Section.INPUT, Item.CONTROLS_LAYOUT, R.string.stream_controls_layout, R.drawable.ic_edit));
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
                entries.add(action(Section.DISPLAY, Item.ZOOM, o.zoomEnabled ?
                        R.string.desktop_zoom_disable : R.string.desktop_zoom_enable, R.drawable.ic_zoom_in));
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
            // control and permission to view and launch.
            if (caps.has(HostCapabilities.Feature.RUNTIME_BITRATE) &&
                    host.hasPermission(ComputerDetails.PERMISSION_VIEW | ComputerDetails.PERMISSION_LAUNCH)) {
                entries.add(action(Section.QUALITY, Item.BITRATE, R.string.stream_bitrate_manual, R.drawable.ic_speed));
                if (o.automaticQuality && o.role == NvApp.Role.STREAM) {
                    entries.add(action(Section.QUALITY, Item.AUTOMATIC_QUALITY, R.string.title_adaptive_quality, R.drawable.ic_tune));
                }
                if (host.encoderBitrateSupported) {
                    entries.add(action(Section.QUALITY, Item.AUTOMATIC_BITRATE, o.automaticBitrateOn ?
                            R.string.stream_auto_bitrate_disable : R.string.stream_auto_bitrate_enable, R.drawable.ic_network));
                }
            }
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
        if (o.webUiAddress && caps.mayHave(HostCapabilities.Feature.WEB_UI)) {
            entries.add(action(Section.HOST, Item.HOST_WEB_UI, R.string.host_administration_menu, R.drawable.ic_web));
        }
        // Without device permissions or a frame limiter the host has no status to show.
        if (host.permission != -1 || caps.has(HostCapabilities.Feature.FRAME_LIMITER_STATUS)) {
            entries.add(action(Section.HOST, Item.HOST_STATUS, R.string.stream_host_status, R.drawable.ic_info));
        }

        entries.add(action(Section.SESSION, Item.RECONNECT, R.string.stream_reconnect, R.drawable.ic_refresh));
        entries.add(action(Section.SESSION, Item.DISCONNECT, R.string.stream_disconnect, R.drawable.ic_close));
        if (host.hasPermission(ComputerDetails.PERMISSION_LAUNCH)) {
            if (o.role == NvApp.Role.STREAM) {
                entries.add(new Entry(Section.SESSION, Item.QUIT_GAME, R.string.stream_quit_game, R.drawable.ic_stop, null, true));
            } else {
                entries.add(new Entry(Section.SESSION, Item.END_SESSION, o.role == NvApp.Role.REMOTE_MONITOR ?
                        R.string.stream_end_monitor : R.string.stream_end_input, R.drawable.ic_stop, null, true));
            }
        }
        return entries;
    }

    public static List<Entry> overlayPage(Options o) {
        List<Entry> entries = new ArrayList<>();
        String[] modes = {PerformanceOverlay.MODE_COMPACT, PerformanceOverlay.MODE_STATISTICS, PerformanceOverlay.MODE_DEBUG};
        Item[] items = {Item.OVERLAY_COMPACT, Item.OVERLAY_STATISTICS, Item.OVERLAY_DEBUG};
        int[] labels = {R.string.overlay_compact, R.string.overlay_statistics, R.string.overlay_debug};
        for (int i = 0; i < modes.length; i++) {
            entries.add(entry(Section.OVERLAY_STYLE, items[i], labels[i], R.drawable.ic_check, modes[i].equals(o.overlayMode)));
        }
        entries.add(action(Section.DIAGNOSTICS, Item.OVERLAY_COPY, R.string.overlay_copy, R.drawable.ic_copy));
        entries.add(action(Section.DIAGNOSTICS, Item.STREAM_HEALTH, R.string.health_title, R.drawable.ic_pulse));
        if (o.canSendInput) {
            entries.add(action(Section.DIAGNOSTICS, Item.LATENCY_TEST, R.string.latency_test, R.drawable.ic_timer));
        }
        return entries;
    }

    private static Entry entry(Section section, Item item, int label, int icon, boolean checked) {
        return new Entry(section, item, label, icon, checked, false);
    }

    private static Entry action(Section section, Item item, int label, int icon) {
        return new Entry(section, item, label, icon, null, false);
    }

    /** Runs a chosen item. Items that close the menu run once it is gone. */
    public interface Listener {
        void onStreamMenuItem(Item item);
    }

    private final Activity activity;
    private final Supplier<Options> options;
    private final Listener listener;
    private final ActionSheet sheet;
    private final Map<Item, MaterialButton> rows = new EnumMap<>(Item.class);
    private Page page = Page.MAIN;
    private List<Entry> entries = new ArrayList<>();

    public StreamMenu(Activity activity, Supplier<Options> options, Listener listener) {
        this.activity = activity;
        this.options = options;
        this.listener = listener;
        Options current = options.get();
        sheet = new ActionSheet(activity, activity.getString(title(current.role)));
        render(Page.MAIN, current, null);
    }

    public ActionSheet sheet() {
        return sheet;
    }

    public Page page() {
        return page;
    }

    public List<Entry> entries() {
        return entries;
    }

    /** The row for an item on the current page, or null when the page leaves it out. */
    public MaterialButton row(Item item) {
        return rows.get(item);
    }

    /** Back: from a page to the first page; the first page closes. */
    public void back() {
        if (page != Page.MAIN) render(Page.MAIN, options.get(), Item.OVERLAY_PAGE);
        else sheet.dismiss();
    }

    private void render(Page target, Options o, Item focus) {
        page = target;
        sheet.clearActions();
        rows.clear();
        sheet.setHeading(activity.getString(target == Page.MAIN ? title(o.role) : R.string.stream_overlay_page));
        sheet.setBackHandler(target == Page.MAIN ? null : this::back);
        entries = target == Page.MAIN ? mainPage(o) : overlayPage(o);
        LinearLayout quickRow = null;
        Section section = null;
        for (Entry entry : entries) {
            MaterialButton button;
            if (entry.section == Section.QUICK) {
                if (quickRow == null) quickRow = sheet.addToggleRow();
                button = sheet.addToggle(quickRow, entry.icon, activity.getString(entry.label), Boolean.TRUE.equals(entry.checked));
                if (entry.checked == null) button.setCheckable(false);
            } else {
                if (entry.section != section) {
                    if (entry.section.title != 0) sheet.addSection(entry.section.title);
                    else if (section != null) sheet.addDivider();
                }
                if (entry.destructive) button = sheet.addDestructiveAction(entry.icon, activity.getString(entry.label));
                else if (entry.opensPage()) button = sheet.addPageAction(entry.icon, activity.getString(entry.label));
                else button = sheet.addAction(entry.icon, activity.getString(entry.label));
                if (entry.checked != null) markChoice(button, entry.checked);
            }
            section = entry.section;
            rows.put(entry.item, button);
            button.setOnClickListener(view -> choose(entry));
        }
        sheet.focusFirstAction();
        MaterialButton focused = focus == null ? null : rows.get(focus);
        if (focused != null && !focused.isInTouchMode()) focused.requestFocus();
    }

    // A choice of several: a check on the chosen one, announced as selected.
    private static void markChoice(MaterialButton button, boolean chosen) {
        if (!chosen) {
            button.setIconTint(android.content.res.ColorStateList.valueOf(android.graphics.Color.TRANSPARENT));
        }
        ViewCompat.setStateDescription(button, button.getContext().getString(chosen ?
                R.string.stream_choice_selected : R.string.stream_choice_not_selected));
    }

    private void choose(Entry entry) {
        if (entry.opensPage()) {
            render(Page.OVERLAY, options.get(), null);
            return;
        }
        listener.onStreamMenuItem(entry.item);
        if (entry.keepsMenuOpen()) {
            refreshToggles();
        }
    }

    // A quick toggle changes in place: show its new state without rebuilding the page or moving focus.
    private void refreshToggles() {
        Options o = options.get();
        for (Entry entry : mainPage(o)) {
            MaterialButton button = rows.get(entry.item);
            if (entry.section != Section.QUICK || entry.checked == null || button == null) continue;
            button.setChecked(Boolean.TRUE.equals(entry.checked));
            button.setIconResource(entry.icon);
        }
    }

    public void show() {
        sheet.show();
    }
}
