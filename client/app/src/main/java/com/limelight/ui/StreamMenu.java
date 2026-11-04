package com.limelight.ui;

import android.app.Activity;
import android.widget.LinearLayout;
import android.widget.TextView;

import androidx.core.view.ViewCompat;

import com.google.android.material.button.MaterialButton;
import com.limelight.LimeLog;
import com.limelight.R;
import com.limelight.binding.input.KeyboardShortcuts;
import com.limelight.binding.video.PerformanceOverlay;
import com.limelight.console.ConnectionInfo;
import com.limelight.console.NativeMenuController;
import com.limelight.nativeapi.NativeAudio;
import com.limelight.nativeapi.NativeCapabilities;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.http.NvApp;

import java.util.ArrayList;
import java.util.EnumMap;
import java.util.HashMap;
import java.util.List;
import java.util.Locale;
import java.util.Map;
import java.util.function.Supplier;

/**
 * The stream menu, a panel over the stream: the game over the stream's mode in its header, a grid of tiles four to
 * a row that the shared model (StreamMenuModel, the same one desktop Nightjar draws from) lays out with the user's
 * shortcuts and Add at its end, then Return to stream and the ways out of the session, always in view under the
 * grid. The rarely used items sit on the More page; the
 * overlay, the host's page, the shortcuts and their editor open in the same sheet, and Back there returns to
 * the first page. Y, X, LB and RB run the tiles that show them. Items a host or this stream cannot use are left out.
 */
public final class StreamMenu {
    public enum Item {
        KEYBOARD, PC_KEYBOARD, OVERLAY, MICROPHONE, NIGHT_MODE, HEADPHONE_SURROUND,
        TOUCH_MODE, MOUSE_CURSOR, CONTROLLER_MOUSE, ONSCREEN_CONTROLS, CONTROLS_LAYOUT, MIC_BUTTON, MENU_BUTTON, GAME_CONTROLS, KEYBOARD_KEYS,
        VIDEO_SCALING, ZOOM, ZOOM_RESET, OVERLAY_PAGE, PICTURE_IN_PICTURE, VIDEO_OUTPUT,
        BITRATE, RATE_CHANGE, AUTOMATIC_QUALITY, AUTOMATIC_BITRATE,
        HOST_MENU, SERVER_COMMANDS, CLIPBOARD_SEND, CLIPBOARD_RECEIVE, IMAGE_CLIPBOARD_SEND, IMAGE_CLIPBOARD_RECEIVE,
        USB_DEVICES, FILES, HOST_WEB_UI, HOST_STATUS,
        RECONNECT, DISCONNECT, QUIT_GAME, END_SESSION, END_HOST_SESSION, CLOSE_GAME,
        NATIVE_PAGE, NATIVE_RESUME, SHORTCUTS, MORE, CLOSE,
        OVERLAY_OFF, OVERLAY_COMPACT, OVERLAY_STATISTICS, OVERLAY_DEBUG,
        OVERLAY_TOP_LEFT, OVERLAY_TOP_RIGHT, OVERLAY_BOTTOM_LEFT, OVERLAY_BOTTOM_RIGHT, OVERLAY_COPY, STREAM_HEALTH, LATENCY_TEST
    }

    public enum Section {
        QUICK(0), INPUT(R.string.stream_section_input), DISPLAY(R.string.stream_section_display),
        QUALITY(R.string.stream_section_quality), HOST(R.string.stream_section_host), SESSION(0),
        OVERLAY_STYLE(0), OVERLAY_POSITION(R.string.overlay_position_section), DIAGNOSTICS(0), NATIVE_SESSION(R.string.native_section_session);

        public final int title;

        Section(int title) {
            this.title = title;
        }
    }

    public enum Page { MAIN, MORE, SHORTCUTS, EDITOR, OVERLAY, NATIVE }

    /** What this stream and its host offer right now; the stream activity fills it each time the menu opens. */
    public static final class Options {
        public final NvApp.Role role;
        public final ComputerDetails host;
        /** The streamed app's name for the line under the title; null leaves only the PC's name. */
        public String appName;
        public boolean canSendInput = true;
        public boolean microphoneOffered, microphoneMuted;
        /** The stream has a microphone button (not on a remote monitor), and whether it shows (its setting is on). */
        public boolean micButtonAvailable, micButtonShown;
        /** The stream has a floating menu button, and whether it shows (its setting is on). */
        public boolean menuButtonAvailable, menuButtonShown;
        /** The stream's size, frame rate, Moonlight video format bits and HDR, for the header's mode line; 0 while unknown. */
        public int streamWidth, streamHeight, streamFps, videoFormat;
        public boolean hdr;
        public boolean overlayShown;
        public String overlayMode = PerformanceOverlay.MODE_COMPACT;
        /** The corner the overlay sits in, as stored under {@code performance_overlay_position}. */
        public String overlayPosition = OverlayPlacement.Corner.TOP_LEFT.key;
        /** False while video plays on an external display and this device is the touchpad. */
        public boolean localOutput = true;
        public boolean mouseConnected, cursorVisible;
        /** A controller is connected, and whether it drives the mouse now. */
        public boolean controllerMouseAvailable, controllerMouseOn;
        public boolean onscreenControls, gameControls, keyboardKeysShown;
        /** Stretching can change live: local MediaCodec video without an upscaler. */
        public boolean scalingAvailable;
        public boolean zoomEnabled;
        public boolean pictureInPicture;
        public boolean externalDisplays;
        /** The host can change resolution and frame rate in place, which automatic quality needs. */
        public boolean automaticQuality;
        public boolean automaticBitrateOn;
        /** The host offered 0x553C rate changes to this Rust stream: bitrate and frame rate change in one dialog. */
        public boolean rateChange;
        /** Automatic bitrate on this device can run here, so the automatic bitrate toggle drives it. */
        public boolean rateChangeBitrate;
        /** The host's web UI address is known for this stream. */
        public boolean webUiAddress;
        /** A native API host lets this device close the game (the host control permission); without it Quit app does. */
        public boolean nativeHostControl;
        /** The host's native API offers its own menu in the stream ({@code session.couch_menu}). */
        public boolean hostMenu;
        /** The host offers USB forwarding and it is on here (Rust engine only). */
        public boolean usbForwarding;
        /** The host's native API carries clipboard images ({@code clipboard.image}). */
        public boolean imageClipboard;
        /** The host offers night mode and headphone surround for this device's stream ({@code audio.*}). */
        public boolean nightModeOffered, headphoneSurroundOffered;
        /** What the host last said about them (GET /audio); null until it answered. */
        public NativeAudio hostAudio;
        /** The host offers them but GET /audio failed: the first page says so in a line. */
        public boolean hostAudioUnread;
        /** Host, codec, size, bitrate and link for the native page; null leaves the connection facts out. */
        public ConnectionInfo connection;

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
            return item == Item.OVERLAY || item == Item.MICROPHONE || item == Item.NIGHT_MODE || item == Item.HEADPHONE_SURROUND;
        }

        public boolean opensPage() {
            return item == Item.OVERLAY_PAGE || item == Item.NATIVE_PAGE || item == Item.SHORTCUTS;
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
                entries.add(action(Section.QUALITY, Item.AUTOMATIC_BITRATE, o.automaticBitrateOn ?
                        R.string.stream_auto_bitrate_disable : R.string.stream_auto_bitrate_enable, R.drawable.ic_network));
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
     * Disconnect first, right under Return to stream; Reconnect waits on the More page, and the actions that end the
     * PC's game or session close the group in the error colour.
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

    /**
     * The row D-pad focus starts on: the first that doesn't end the stream, so OK pressed right after opening the
     * menu never disconnects or quits.
     */
    public static Item firstFocus(List<Entry> entries) {
        for (Entry entry : entries) {
            if (entry.item != Item.DISCONNECT && !entry.destructive) return entry.item;
        }
        return entries.isEmpty() ? null : entries.get(0).item;
    }

    /**
     * The native page's actions: Resume closes the menu, Disconnect leaves the session suspended, End session
     * ends it, and the game is closed with Close game when this device has host control, else with Quit app.
     * The connection facts and the Radeon panel follow them.
     */
    public static List<Entry> nativePage(Options o) {
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

    public static List<Entry> overlayPage(Options o) {
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

    /** The user's own shortcuts: kept by the app (KeyboardShortcuts), shown and edited in the menu. */
    public interface Shortcuts {
        List<KeyboardShortcuts.Shortcut> saved();

        void save(List<KeyboardShortcuts.Shortcut> shortcuts);

        /** Closes the menu and presses {@code keys} on the PC once the stream has focus again. */
        void send(int[] keys);
    }

    private final Activity activity;
    private final Supplier<Options> options;
    private final Listener listener;
    private final Shortcuts shortcuts;
    private final ActionSheet sheet;
    private final Map<Item, MaterialButton> rows = new EnumMap<>(Item.class);
    private Page page = Page.MAIN;
    private List<Entry> entries = new ArrayList<>();
    private final NativeMenuController radeon;
    private RadeonPanelView radeonView;
    private TextView audioNote;
    private StreamMenuModel.Layout layout;
    // What the page was built from, so a refresh can tell a changed state from items that came or went.
    private List<Item> offeredItems = new ArrayList<>();
    private TileGrid grid;
    private TextView failure;
    // The shortcut being edited (null for a new one), and the editor's name and keys in their saved names.
    private KeyboardShortcuts.Shortcut editing;
    private String draftName = "";
    private final List<String> draftKeys = new ArrayList<>();
    private TextView draftChord, draftError;
    private final Map<String, MaterialButton> keyCaps = new HashMap<>();

    public StreamMenu(Activity activity, Supplier<Options> options, Listener listener) {
        this(activity, options, listener, null, null);
    }

    public StreamMenu(Activity activity, Supplier<Options> options, Listener listener, NativeMenuController radeon) {
        this(activity, options, listener, radeon, null);
    }

    /**
     * {@code radeon} reads and writes the host's Radeon settings for the native page; null leaves the panel out.
     * {@code shortcuts} puts the user's shortcuts in the grid; null leaves them and the Shortcuts tile out.
     */
    public StreamMenu(Activity activity, Supplier<Options> options, Listener listener, NativeMenuController radeon, Shortcuts shortcuts) {
        this.activity = activity;
        this.options = options;
        this.listener = listener;
        this.radeon = radeon;
        this.shortcuts = shortcuts;
        Options current = options.get();
        // A panel over the stream, as wide as desktop Nightjar's menu, the stream visible beside it.
        sheet = new ActionSheet(activity, activity.getString(title(current.role)), true);
        sheet.setOnClosed(this::stopRadeon);
        sheet.setKeyHandler(this::hotkey);
        render(Page.MAIN, current, null);
    }

    // The panel polls the host only while its page is on screen.
    private void stopRadeon() {
        if (radeonView != null) {
            radeonView.detach();
            radeonView = null;
        }
        if (radeon != null) radeon.stop();
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

    /** The row or tile for an item on the current page, or null when the page leaves it out. */
    public MaterialButton row(Item item) {
        return rows.get(item);
    }

    /** The first page's grid as the shared model laid it out; null when the model failed or another page shows. */
    public StreamMenuModel.Layout layout() {
        return page == Page.MAIN ? layout : null;
    }

    /** The first page's tiles, in order; null on other pages. */
    public TileGrid grid() {
        return grid;
    }

    /** The line saying why the menu could not be laid out, or null when it could. */
    public TextView failure() {
        return failure;
    }

    /** Back: from the editor to the shortcuts, from any other page to the first page; the first page closes. */
    public void back() {
        switch (page) {
            case MAIN: sheet.dismiss(); return;
            case EDITOR: render(Page.SHORTCUTS, options.get(), null); return;
            case NATIVE: render(Page.MAIN, options.get(), Item.NATIVE_PAGE); return;
            case OVERLAY: render(Page.MAIN, options.get(), Item.OVERLAY_PAGE); return;
            case SHORTCUTS: render(Page.MAIN, options.get(), Item.SHORTCUTS); return;
            default: render(Page.MAIN, options.get(), Item.MORE);
        }
    }

    private static String id(Item item) {
        return item.name().toLowerCase(Locale.ROOT);
    }

    private static int heading(Page target, Options o) {
        switch (target) {
            case NATIVE: return R.string.native_menu_row;
            case OVERLAY: return R.string.stream_overlay_page;
            case MORE: return R.string.stream_menu_more;
            case SHORTCUTS: return R.string.stream_menu_shortcuts;
            default: return title(o.role);
        }
    }

    private void render(Page target, Options o, Item focus) {
        page = target;
        stopRadeon();
        sheet.clearActions();
        rows.clear();
        keyCaps.clear();
        grid = null;
        layout = null;
        failure = null;
        audioNote = null;
        draftChord = null;
        draftError = null;
        String[] header = target == Page.MAIN ? header(o) : null;
        sheet.setHeading(target == Page.EDITOR
                ? activity.getString(editing == null ? R.string.keyboard_shortcuts_add : R.string.keyboard_shortcuts_edit)
                : header != null ? header[0] : activity.getString(heading(target, o)));
        sheet.setSubtitle(header == null ? null : header[1]);
        sheet.setBackHandler(target == Page.MAIN ? null : this::back);
        if (target == Page.MAIN) renderGrid(o);
        else if (target == Page.SHORTCUTS) renderShortcuts();
        else if (target == Page.EDITOR) renderEditor();
        else if (target == Page.MORE) renderList(moreEntries(o));
        else renderList(target == Page.NATIVE ? nativePage(o) : overlayPage(o));
        if (target == Page.NATIVE) addNativeFacts(o);
        sheet.focusFirstAction();
        MaterialButton focused = focus == null ? null : rows.get(focus);
        if (focused != null && !focused.isInTouchMode()) focused.requestFocus();
    }

    // What the first page offers: the entries this stream can use, and the shortcuts where they can be sent.
    private List<Entry> offered(Options o) {
        List<Entry> list = pageEntries(Page.MAIN, o);
        if (shortcuts != null && o.canSendInput) {
            list.add(action(Section.QUICK, Item.SHORTCUTS, R.string.stream_menu_shortcuts, R.drawable.ic_keyboard));
        }
        return list;
    }

    private Entry find(List<Entry> list, String id) {
        for (Entry entry : list) {
            if (id(entry.item).equals(id)) return entry;
        }
        return null;
    }

    // The More page: its switches first, then the rest in the model's order.
    private List<Entry> moreEntries(Options o) {
        List<Entry> offered = offered(o);
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

    private static List<StreamMenuModel.Offer> offers(List<Entry> list) {
        List<StreamMenuModel.Offer> offers = new ArrayList<>();
        for (Entry entry : list) offers.add(new StreamMenuModel.Offer(id(entry.item), entry.checked));
        return offers;
    }

    /**
     * The first page: the grid the shared model lays out (the user's shortcuts and Add at its end), then Return to
     * stream and the ways out of the session in the panel's footer. When the model cannot answer, the page says why and
     * keeps the ways out.
     */
    private void renderGrid(Options o) {
        if (o.role == NvApp.Role.STREAM && o.hostAudioUnread) {
            audioNote = sheet.addNote(sheet.addGroup(), activity.getString(R.string.stream_audio_unread), false);
        }
        List<Entry> offered = offered(o);
        offeredItems = items(offered);
        entries = new ArrayList<>();
        List<KeyboardShortcuts.Shortcut> saved = shortcuts != null && o.canSendInput ? shortcuts.saved() : null;
        java.util.Map<String, String> labels;
        try {
            layout = StreamMenuModel.layout(offers(offered), saved == null ? null : saved.size());
            labels = StreamMenuModel.labels();
        } catch (StreamMenuModel.Refused e) {
            // Without the model the stream still runs (the C engine needs no libnightjar_core): every entry stays
            // reachable as a plain list, in this client's order and words, above the ways out.
            LimeLog.warning("Stream menu: the model failed: " + e.getMessage());
            layout = null;
            failure = sheet.addNote(sheet.addGroup(), activity.getString(R.string.stream_menu_model_failed, e.getMessage()), true);
            List<Entry> plain = new ArrayList<>();
            for (Entry entry : offered) if (entry.section != Section.SESSION) plain.add(entry);
            addRows(plain);
            entries.addAll(plain);
            addFooter(offered, null, null);
            sheet.setFirstAction(plain.isEmpty() ? rows.get(Item.CLOSE) : rows.get(firstFocus(plain)));
            return;
        }
        grid = sheet.addTileGrid();
        for (StreamMenuModel.Cell cell : layout.grid) {
            MaterialButton tile;
            switch (cell.kind) {
                case SHORTCUT: {
                    KeyboardShortcuts.Shortcut shortcut = saved.get(cell.shortcut);
                    tile = grid.addTile(R.drawable.ic_keyboard, shortcut.name, keysUnder(shortcut), null, null);
                    tile.setOnClickListener(view -> shortcuts.send(shortcut.keys()));
                    break;
                }
                case ADD:
                    tile = grid.addTile(R.drawable.ic_plus, label(labels, StreamMenuModel.ADD), null, null, null);
                    tile.setOnClickListener(view -> edit(null));
                    break;
                default: {
                    if (StreamMenuModel.MORE.equals(cell.id)) {
                        tile = grid.addTile(R.drawable.ic_more_horiz, label(labels, StreamMenuModel.MORE), null, null, null);
                        rows.put(Item.MORE, tile);
                        tile.setOnClickListener(view -> render(Page.MORE, options.get(), null));
                        break;
                    }
                    Entry entry = find(offered, cell.id);
                    tile = grid.addTile(entry.icon, label(labels, cell.id), null, cell.on,
                            cell.hotkey == null ? null : cell.hotkey.toUpperCase(Locale.ROOT));
                    rows.put(entry.item, tile);
                    entries.add(entry);
                    tile.setOnClickListener(view -> choose(entry));
                }
            }
            if (grid.getChildCount() == 1) sheet.setFirstAction(tile);
        }
        addFooter(offered, layout.footer, labels);
    }

    /**
     * A tile's short name, the same as desktop Nightjar's: the {@code stream_tile_<key>} string for the key the shared
     * model gives its id. A tile the model names no label for is a disagreement between the two, which a test catches.
     */
    private String label(java.util.Map<String, String> labels, String id) {
        String key = labels.get(id);
        int label = key == null ? 0 : tileLabel(key);
        if (label == 0) throw new IllegalStateException("the stream menu model names no label for " + id + " (" + key + ")");
        return activity.getString(label);
    }

    /** The string for a label key of the shared model ({@code stream_menu::label}), or 0 for a key this client lacks. */
    public static int tileLabel(String key) {
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

    /**
     * The first page's title and the line under it, as desktop Nightjar's: the game over the stream's mode
     * ({@code 1440p · 144 FPS · HEVC}), or over the PC while the mode is unknown; without a game, the menu's title
     * over the app and PC.
     */
    String[] header(Options o) {
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

    private String subtitle(Options o) {
        String app = o.appName == null ? "" : o.appName.trim();
        String host = o.host.name == null ? "" : o.host.name.trim();
        if (app.isEmpty() && host.isEmpty()) return null;
        return app.isEmpty() ? host : host.isEmpty() ? app : app + " · " + host;
    }

    /**
     * Return to stream, wide and filled, then the ways out in the model's order and in its words (all of them, in this
     * client's words, when it failed). A way out only Android offers, a remote monitor's, keeps its own words.
     */
    private void addFooter(List<Entry> offered, List<String> footer, java.util.Map<String, String> labels) {
        String back = labels != null ? label(labels, StreamMenuModel.CLOSE) : activity.getString(R.string.stream_menu_return);
        MaterialButton close = sheet.addPrimaryAction(R.drawable.ic_play, back, "B");
        rows.put(Item.CLOSE, close);
        close.setOnClickListener(view -> sheet.dismiss());
        if (grid == null) sheet.setFirstAction(close);
        for (Entry entry : offered) {
            if (entry.section != Section.SESSION || (footer != null && !footer.contains(id(entry.item)))) continue;
            String name = labels != null && labels.containsKey(id(entry.item)) ? label(labels, id(entry.item)) : activity.getString(entry.label);
            MaterialButton button = sheet.addFooterAction(entry.icon, name, entry.destructive);
            rows.put(entry.item, button);
            entries.add(entry);
            button.setOnClickListener(view -> choose(entry));
        }
    }

    private static List<Item> items(List<Entry> list) {
        List<Item> items = new ArrayList<>();
        for (Entry entry : list) items.add(entry.item);
        return items;
    }

    // The keys' text the model gives, as Ctrl + Shift + Esc; the saved names when it cannot.
    /** The keys under a shortcut's name, or null when the name already says them (a shortcut saved unnamed). */
    private static String keysUnder(KeyboardShortcuts.Shortcut shortcut) {
        String keys = chordText(shortcut.keys());
        return keys.equals(shortcut.name) ? null : keys;
    }

    private static String chordText(int[] keys) {
        String[] names = names(keys);
        try {
            return StreamMenuModel.chordLabel(names);
        } catch (StreamMenuModel.Refused e) {
            return String.join(" + ", names);
        }
    }

    private static String[] names(int[] keys) {
        return KeyboardShortcuts.format(keys).split(" \\+ ");
    }

    /** A controller button on the first page runs the tile that shows it: Y, X, LB or RB. */
    public boolean hotkey(int keyCode) {
        if (page != Page.MAIN || layout == null || grid == null) return false;
        String key;
        switch (keyCode) {
            case android.view.KeyEvent.KEYCODE_BUTTON_Y: key = "y"; break;
            case android.view.KeyEvent.KEYCODE_BUTTON_X: key = "x"; break;
            case android.view.KeyEvent.KEYCODE_BUTTON_L1: key = "lb"; break;
            case android.view.KeyEvent.KEYCODE_BUTTON_R1: key = "rb"; break;
            default: return false;
        }
        int index = layout.hotkey(key);
        if (index < 0) return false;
        grid.getChildAt(index).performClick();
        return true;
    }

    // The shortcuts page: the user's, in their order, then the built-ins not saved under a name of their own, Add, then a
    // row to edit each. The user's come first so a new built-in never pushes them along (nightjar#61).
    private void renderShortcuts() {
        entries = new ArrayList<>();
        List<KeyboardShortcuts.Shortcut> saved = shortcuts.saved();
        List<String[]> keys = new ArrayList<>();
        for (KeyboardShortcuts.Shortcut shortcut : saved) keys.add(names(shortcut.keys()));
        List<StreamMenuModel.BuiltIn> builtIns;
        try {
            builtIns = StreamMenuModel.builtIns(keys);
        } catch (StreamMenuModel.Refused e) {
            failure = sheet.addNote(sheet.addGroup(), activity.getString(R.string.stream_menu_model_failed, e.getMessage()), true);
            builtIns = new ArrayList<>();
        }
        TileGrid tiles = sheet.addTileGrid();
        for (KeyboardShortcuts.Shortcut shortcut : saved) {
            MaterialButton tile = tiles.addTile(R.drawable.ic_keyboard, shortcut.name, keysUnder(shortcut), null, null);
            tile.setOnClickListener(view -> shortcuts.send(shortcut.keys()));
            if (tiles.getChildCount() == 1) sheet.setFirstAction(tile);
        }
        for (StreamMenuModel.BuiltIn builtIn : builtIns) {
            int[] chord = KeyboardShortcuts.parseChord(String.join("+", builtIn.keys));
            MaterialButton tile = tiles.addTile(R.drawable.ic_keyboard, activity.getString(builtInLabel(builtIn.id)), builtIn.label, null, null);
            tile.setOnClickListener(view -> shortcuts.send(chord));
            if (tiles.getChildCount() == 1) sheet.setFirstAction(tile);
        }
        if (saved.size() < KeyboardShortcuts.MAX_SHORTCUTS) {
            MaterialButton add = tiles.addTile(R.drawable.ic_plus, activity.getString(R.string.keyboard_shortcuts_add), null, null, null);
            add.setOnClickListener(view -> edit(null));
            if (tiles.getChildCount() == 1) sheet.setFirstAction(add);
        }
        if (saved.isEmpty()) return;
        sheet.addSection(R.string.desktop_controls_edit_shortcuts);
        for (KeyboardShortcuts.Shortcut shortcut : saved) {
            MaterialButton row = sheet.addAction(R.drawable.ic_edit, shortcut.name, chordText(shortcut.keys()));
            row.setOnClickListener(view -> edit(shortcut));
        }
    }

    /** A built-in's name in this language. */
    public static int builtInLabel(String id) {
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

    private void edit(KeyboardShortcuts.Shortcut shortcut) {
        editing = shortcut;
        draftName = shortcut == null ? "" : shortcut.name;
        draftKeys.clear();
        if (shortcut != null) java.util.Collections.addAll(draftKeys, names(shortcut.keys()));
        render(Page.EDITOR, options.get(), null);
    }

    // The editor: a name (the keys' text when left empty), the chord so far, the key picker, then Save, Cancel and Delete.
    private void renderEditor() {
        entries = new ArrayList<>();
        android.widget.LinearLayout group = sheet.addGroup();
        com.google.android.material.textfield.TextInputLayout box = new com.google.android.material.textfield.TextInputLayout(activity);
        box.setHint(activity.getString(R.string.keyboard_shortcuts_name));
        com.google.android.material.textfield.TextInputEditText name = new com.google.android.material.textfield.TextInputEditText(box.getContext());
        name.setSingleLine(true);
        name.setText(draftName);
        name.setFilters(new android.text.InputFilter[] {new android.text.InputFilter.LengthFilter(80)});
        name.addTextChangedListener(new android.text.TextWatcher() {
            @Override public void beforeTextChanged(CharSequence s, int start, int count, int after) { }
            @Override public void onTextChanged(CharSequence s, int start, int before, int count) { }
            @Override public void afterTextChanged(android.text.Editable s) { draftName = s.toString(); }
        });
        box.addView(name);
        int spacing = sheet.spacing();
        box.setPadding(spacing, spacing, spacing, 0);
        group.addView(box);
        draftChord = new TextView(activity);
        draftChord.setTextAppearance(activity, R.style.TextAppearance_Nightjar_ScreenTitle);
        draftChord.setPadding(spacing, spacing * 2, spacing, spacing);
        group.addView(draftChord);
        List<StreamMenuModel.Key> picker;
        try {
            picker = StreamMenuModel.picker();
        } catch (StreamMenuModel.Refused e) {
            failure = sheet.addNote(group, activity.getString(R.string.stream_menu_model_failed, e.getMessage()), true);
            picker = new ArrayList<>();
        }
        TileGrid caps = new TileGrid(activity, spacing / 2, 6, 44);
        caps.setPadding(spacing, 0, spacing, spacing);
        group.addView(caps);
        // Keys an older shortcut saved that the picker lacks (F13, a media key, an Android-only code): caps of their
        // own first, so they show and can be taken out.
        java.util.Set<String> offered = new java.util.HashSet<>();
        for (StreamMenuModel.Key key : picker) offered.add(key.name);
        for (String key : new ArrayList<>(draftKeys)) {
            if (!offered.contains(key)) addCap(caps, key, keyText(key));
        }
        for (StreamMenuModel.Key key : picker) addCap(caps, key.name, key.label);
        draftError = sheet.addNote(group, "", true);
        draftError.setVisibility(android.view.View.GONE);
        showDraft();
        MaterialButton save = sheet.addAction(R.drawable.ic_check, activity.getString(R.string.keyboard_shortcuts_save));
        save.setOnClickListener(view -> saveDraft());
        MaterialButton cancel = sheet.addAction(R.drawable.ic_close, activity.getString(R.string.applist_menu_cancel));
        cancel.setOnClickListener(view -> back());
        if (editing != null) {
            KeyboardShortcuts.Shortcut target = editing;
            MaterialButton delete = sheet.addDestructiveAction(R.drawable.ic_delete, activity.getString(R.string.keyboard_shortcuts_delete));
            delete.setOnClickListener(view -> {
                List<KeyboardShortcuts.Shortcut> list = new ArrayList<>(shortcuts.saved());
                list.removeIf(s -> s.id.equals(target.id));
                shortcuts.save(list);
                render(Page.SHORTCUTS, options.get(), null);
            });
        }
    }

    // A key's cap text from the model; its saved name when the model cannot say.
    private static String keyText(String key) {
        try {
            return StreamMenuModel.chordLabel(new String[] {key});
        } catch (StreamMenuModel.Refused e) {
            return key;
        }
    }

    private void addCap(TileGrid caps, String name, String label) {
        MaterialButton cap = new MaterialButton(activity, null, com.google.android.material.R.attr.materialButtonOutlinedStyle);
        cap.setText(label);
        cap.setAllCaps(false);
        cap.setTextAppearance(activity, R.style.TextAppearance_Nightjar_Label);
        cap.setSingleLine(true);
        cap.setInsetTop(0);
        cap.setInsetBottom(0);
        cap.setMinWidth(0);
        cap.setMinimumWidth(0);
        cap.setPadding(0, 0, 0, 0);
        cap.setCheckable(true);
        int[][] states = {new int[] {android.R.attr.state_checked}, new int[] {}};
        cap.setBackgroundTintList(new android.content.res.ColorStateList(states, new int[] {
                com.google.android.material.color.MaterialColors.getColor(activity, com.google.android.material.R.attr.colorPrimaryContainer, 0),
                android.graphics.Color.TRANSPARENT}));
        cap.setTextColor(new android.content.res.ColorStateList(states, new int[] {
                com.google.android.material.color.MaterialColors.getColor(activity, com.google.android.material.R.attr.colorOnPrimaryContainer, 0),
                com.google.android.material.color.MaterialColors.getColor(activity, com.google.android.material.R.attr.colorOnSurface, 0)}));
        caps.addView(cap);
        keyCaps.put(name, cap);
        cap.setOnClickListener(view -> toggleKey(name));
        if (caps.getChildCount() == 1) sheet.setFirstAction(cap);
    }

    private void toggleKey(String key) {
        if (!draftKeys.remove(key) && draftKeys.size() < KeyboardShortcuts.MAX_KEYS) draftKeys.add(key);
        showDraft();
    }

    private void showDraft() {
        for (Map.Entry<String, MaterialButton> cap : keyCaps.entrySet()) cap.getValue().setChecked(draftKeys.contains(cap.getKey()));
        String text = "—";
        if (!draftKeys.isEmpty()) {
            try {
                text = StreamMenuModel.chordLabel(draftKeys.toArray(new String[0]));
            } catch (StreamMenuModel.Refused e) {
                text = String.join(" + ", draftKeys);
            }
        }
        draftChord.setText(text);
    }

    /** The editor's Save: the model checks the chord, then the list is saved with it added or replaced. */
    void saveDraft() {
        String[] keys = draftKeys.toArray(new String[0]);
        try {
            if (keys.length == 0) throw new StreamMenuModel.Refused("chord_length", "");
            String name = draftName.trim().isEmpty() ? StreamMenuModel.chordLabel(keys) : draftName.trim();
            String[] checked = StreamMenuModel.validate(name, keys);
            int[] codes = KeyboardShortcuts.parseChord(String.join("+", checked));
            List<KeyboardShortcuts.Shortcut> list = new ArrayList<>(shortcuts.saved());
            String id = editing != null ? editing.id : java.util.UUID.randomUUID().toString();
            KeyboardShortcuts.Shortcut shortcut = new KeyboardShortcuts.Shortcut(id, name, codes);
            int at = -1;
            for (int i = 0; i < list.size(); i++) if (list.get(i).id.equals(id)) at = i;
            if (at >= 0) list.set(at, shortcut);
            else if (list.size() < KeyboardShortcuts.MAX_SHORTCUTS) list.add(shortcut);
            else throw new StreamMenuModel.Refused("too_many", "");
            shortcuts.save(list);
            render(Page.SHORTCUTS, options.get(), null);
        } catch (StreamMenuModel.Refused | IllegalArgumentException e) {
            String code = e instanceof StreamMenuModel.Refused ? ((StreamMenuModel.Refused) e).code : "";
            String detail = e instanceof StreamMenuModel.Refused ? ((StreamMenuModel.Refused) e).detail : "";
            if ("chord_length".equals(code)) draftError.setText(R.string.stream_shortcut_pick_keys);
            // The key that cannot go in a shortcut, as its cap shows it, so it can be taken out.
            else if ("unsupported_key".equals(code) && !detail.isEmpty()) {
                draftError.setText(activity.getString(R.string.stream_shortcut_key_unsupported, keyText(detail)));
            } else draftError.setText(R.string.keyboard_shortcuts_invalid);
            draftError.setVisibility(android.view.View.VISIBLE);
        }
    }

    // A page of rows: switches side by side at the top, then the rest under their sections.
    private void renderList(List<Entry> list) {
        entries = list;
        offeredItems = items(list);
        addRows(list);
        sheet.setFirstAction(rows.get(firstFocus(list)));
    }

    private void addRows(List<Entry> list) {
        LinearLayout quickRow = null;
        Section section = null;
        for (Entry entry : list) {
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
    }

    private List<Entry> pageEntries(Page target, Options o) {
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

    /**
     * Shows what changed while the menu is open, such as the host's answer about its audio: the switches in
     * place, or the page again when items came or went.
     */
    public void refresh() {
        if ((page != Page.MAIN && page != Page.MORE) || !sheet.isShowing()) return;
        Options o = options.get();
        List<Item> now = items(page == Page.MAIN ? offered(o) : moreEntries(o));
        boolean noted = page == Page.MAIN && o.role == NvApp.Role.STREAM && o.hostAudioUnread;
        if (now.equals(offeredItems) && noted == (audioNote != null)) refreshToggles();
        else render(page, o, null);
    }

    /** The line saying the host's audio settings could not be read, or null when the page has none. */
    public TextView audioNote() {
        return audioNote;
    }

    // The connection facts and the Radeon panel under the native page's actions.
    private void addNativeFacts(Options o) {
        if (o.connection != null) {
            sheet.addSection(R.string.native_section_connection);
            for (ConnectionInfo.Line line : o.connection.lines(activity)) {
                sheet.addInfo(activity.getString(line.label), line.value);
            }
        }
        if (radeon != null) {
            radeonView = new RadeonPanelView(sheet, sheet.addGroup(), radeon::write);
            radeon.start(radeonView::show);
        }
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
            render(entry.item == Item.NATIVE_PAGE ? Page.NATIVE : entry.item == Item.SHORTCUTS ? Page.SHORTCUTS : Page.OVERLAY,
                    options.get(), null);
            return;
        }
        listener.onStreamMenuItem(entry.item);
        if (entry.keepsMenuOpen()) {
            refreshToggles();
        }
    }

    // A switch changes in place: show its new state without rebuilding the page or moving focus.
    private void refreshToggles() {
        Options o = options.get();
        for (Entry entry : mainPage(o)) {
            MaterialButton button = rows.get(entry.item);
            if (entry.checked == null || button == null || !button.isCheckable()) continue;
            button.setChecked(Boolean.TRUE.equals(entry.checked));
            button.setIconResource(entry.icon);
        }
    }

    public void show() {
        sheet.show();
    }
}
