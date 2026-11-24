package com.limelight.ui;

import android.app.Activity;
import android.widget.LinearLayout;
import android.widget.TextView;

import com.google.android.material.button.MaterialButton;
import com.limelight.LimeLog;
import com.limelight.R;
import com.limelight.binding.input.KeyboardShortcuts;
import com.limelight.binding.video.PerformanceOverlay;
import com.limelight.console.ConnectionInfo;
import com.limelight.console.NativeMenuController;
import com.limelight.nativeapi.NativeAudio;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvApp;

import java.util.ArrayList;
import java.util.EnumMap;
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
        USB_DEVICES, GUESTS, FILES, HOST_WEB_UI, HOST_STATUS,
        RECONNECT, DISCONNECT, CONFIRM_DISCONNECT, QUIT_GAME, END_SESSION, END_HOST_SESSION, CLOSE_GAME,
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
        /** The game's cover for the header; null leaves it out. */
        public android.graphics.Bitmap cover;
        /** The stream's bitrate for the slider on the first page, in kbps; 0 leaves the slider out. */
        public int bitrateKbps;
        /**
         * Sends a bitrate the slider settled on and answers the rate the slider is to show: the one sent, or the one still applied
         * when nothing could be sent. With no way to send it the slider is left out.
         */
        public java.util.function.IntUnaryOperator bitrateChange;
        /** The PC has not said yet what it counts the stream's bitrate as: the slider shows it disabled. */
        public boolean bitrateWaiting;
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
        /** The host lets other devices join this game as guests and this device manages them ({@code session.guests}). */
        public boolean sessionGuests;
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
        return StreamMenuCatalog.title(role);
    }

    public static List<Entry> mainPage(Options o) {
        return StreamMenuCatalog.mainPage(o);
    }

    /**
     * The row D-pad focus starts on: the first that doesn't end the stream, so OK pressed right after opening the
     * menu never disconnects or quits.
     */
    public static Item firstFocus(List<Entry> entries) {
        return StreamMenuCatalog.firstFocus(entries);
    }

    /**
     * The native page's actions: Resume closes the menu, Disconnect leaves the session suspended, End session
     * ends it, and the game is closed with Close game when this device has host control, else with Quit app.
     * The connection facts and the Radeon panel follow them.
     */
    public static List<Entry> nativePage(Options o) {
        return StreamMenuCatalog.nativePage(o);
    }

    public static List<Entry> overlayPage(Options o) {
        return StreamMenuCatalog.overlayPage(o);
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

    static final List<String> FALLBACK_FOOTER = StreamMenuCatalog.FALLBACK_FOOTER;

    /** Set by a test for a controller that is there (true) or not (false); null asks Android. */
    static Boolean gamepadForTest;

    private final Activity activity;
    private final Supplier<Options> options;
    private final Listener listener;
    private final Shortcuts shortcuts;
    private final ActionSheet sheet;
    private final StreamMenuShortcutsPage shortcutsPage;
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
    // What each of the model's cells became: a tile or a status row. Hotkeys go by the model's order.
    private android.view.View[] cells;
    private StepSlider bitrateSlider;
    // The rate the slider's row shows beside it.
    private TextView bitrateShown;
    private TextView failure;

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
        shortcutsPage = new StreamMenuShortcutsPage(activity, sheet, shortcuts, this::back,
                () -> render(Page.SHORTCUTS, options.get(), null),
                () -> render(Page.EDITOR, options.get(), null));
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

    private void render(Page target, Options o, Item focus) {
        page = target;
        stopRadeon();
        sheet.clearActions();
        rows.clear();
        shortcutsPage.clearViews();
        grid = null;
        cells = null;
        bitrateSlider = null;
        bitrateShown = null;
        layout = null;
        failure = null;
        audioNote = null;
        String[] header = target == Page.MAIN ? header(o) : null;
        sheet.setHeading(target == Page.EDITOR
                ? activity.getString(shortcutsPage.editorHeading())
                : header != null ? header[0] : activity.getString(StreamMenuCatalog.heading(target, o)));
        sheet.setSubtitle(header == null ? null : header[1]);
        // The game's cover heads the first page only; a page below says where it is, its back button in the cover's place.
        sheet.setCover(target == Page.MAIN && o.role == NvApp.Role.STREAM ? o.cover : null);
        sheet.setBackHandler(target == Page.MAIN ? null : this::back);
        if (target == Page.MAIN) renderGrid(o);
        else if (target == Page.SHORTCUTS) {
            entries = new ArrayList<>();
            failure = shortcutsPage.renderShortcuts();
        } else if (target == Page.EDITOR) {
            entries = new ArrayList<>();
            failure = shortcutsPage.renderEditor();
        } else if (target == Page.MORE) renderList(StreamMenuCatalog.moreEntries(offered(o)));
        else renderList(target == Page.NATIVE ? nativePage(o) : overlayPage(o));
        if (target == Page.NATIVE) addNativeFacts(o);
        sheet.focusFirstAction();
        MaterialButton focused = focus == null ? null : rows.get(focus);
        if (focused != null && !focused.isInTouchMode()) focused.requestFocus();
    }

    // What the first page offers: the entries this stream can use, and the shortcuts where they can be sent.
    private List<Entry> offered(Options o) {
        List<Entry> list = StreamMenuCatalog.pageEntries(activity, Page.MAIN, o);
        if (shortcuts != null && o.canSendInput) {
            list.add(StreamMenuCatalog.action(Section.QUICK, Item.SHORTCUTS, R.string.stream_menu_shortcuts, R.drawable.ic_keyboard));
        }
        return list;
    }

    /**
     * The first page: the grid the shared model lays out (the user's shortcuts and Add at its end), the bitrate and status
     * rows, and the ways out of the session in the panel's footer; the header's X goes back to the stream. When the model
     * cannot answer, the page says why and keeps the ways out.
     */
    private void renderGrid(Options o) {
        if (o.role == NvApp.Role.STREAM && o.hostAudioUnread) {
            audioNote = sheet.addNote(sheet.addGroup(), activity.getString(R.string.stream_audio_unread), false);
        }
        List<Entry> offered = offered(o);
        offeredItems = StreamMenuCatalog.items(offered);
        entries = new ArrayList<>();
        List<KeyboardShortcuts.Shortcut> saved = shortcuts != null && o.canSendInput ? shortcuts.saved() : null;
        Map<String, String> labels;
        try {
            layout = StreamMenuModel.layout(StreamMenuCatalog.offers(offered), saved == null ? null : saved.size(), buttonChoices(activity));
            labels = StreamMenuModel.labels();
        } catch (StreamMenuModel.Refused e) {
            // Without the model the stream still runs: every entry stays
            // reachable as a plain list, in this client's order and words, above the ways out.
            LimeLog.warning("Stream menu: the model failed: " + e.getMessage());
            layout = null;
            failure = sheet.addNote(sheet.addGroup(), activity.getString(R.string.stream_menu_model_failed, e.getMessage()), true);
            // The ways out stay the model's two, Disconnect and the game's end; Reconnect and End session join the list.
            List<Entry> plain = new ArrayList<>();
            for (Entry entry : offered) if (!FALLBACK_FOOTER.contains(StreamMenuCatalog.id(entry.item))) plain.add(entry);
            addRows(plain);
            entries.addAll(plain);
            addFooter(offered, FALLBACK_FOOTER, null);
            sheet.setFirstAction(plain.isEmpty() ? rows.get(Item.CLOSE) : rows.get(firstFocus(plain)));
            return;
        }
        grid = sheet.addTileGrid();
        // A remote has no face buttons: the caps show only while a controller could press them.
        grid.setCapsShown(capsShown());
        sheet.setCapsShown(capsShown());
        cells = new android.view.View[layout.grid.size()];
        List<StreamMenuModel.Cell> statusCells = new ArrayList<>();
        for (int at = 0; at < layout.grid.size(); at++) {
            StreamMenuModel.Cell cell = layout.grid.get(at);
            if (cell.kind == StreamMenuModel.Cell.Kind.TILE && StreamMenuCatalog.isStatusRow(cell.id)) {
                statusCells.add(cell);
                continue;
            }
            MaterialButton tile;
            switch (cell.kind) {
                case SHORTCUT: {
                    KeyboardShortcuts.Shortcut shortcut = saved.get(cell.shortcut);
                    tile = grid.addTile(R.drawable.ic_keyboard, shortcut.name, StreamMenuShortcutsPage.keysUnder(shortcut), null, null);
                    tile.setOnClickListener(view -> shortcuts.send(shortcut.keys()));
                    break;
                }
                case ADD:
                    tile = grid.addTile(R.drawable.ic_plus, StreamMenuCatalog.label(activity, labels, StreamMenuModel.ADD), null, null, null);
                    tile.setOnClickListener(view -> shortcutsPage.edit(null));
                    break;
                default: {
                    if (StreamMenuModel.MORE.equals(cell.id)) {
                        tile = grid.addTile(R.drawable.ic_more_horiz, StreamMenuCatalog.label(activity, labels, StreamMenuModel.MORE), null, null, null);
                        rows.put(Item.MORE, tile);
                        tile.setOnClickListener(view -> render(Page.MORE, options.get(), null));
                        break;
                    }
                    Entry entry = StreamMenuCatalog.find(offered, cell.id);
                    tile = grid.addTile(entry.icon, StreamMenuCatalog.label(activity, labels, cell.id), null, cell.on,
                            cell.hotkey == null ? null : cell.hotkey.toUpperCase(Locale.ROOT));
                    rows.put(entry.item, tile);
                    entries.add(entry);
                    tile.setOnClickListener(view -> choose(entry));
                }
            }
            cells[at] = tile;
            if (grid.getChildCount() == 1) sheet.setFirstAction(tile);
        }
        addStatus(o, offered, statusCells, labels);
        addFooter(offered, layout.footer, labels);
    }

    /** The bitrate slider and the status rows between the tiles and the ways out: the same items, in the model's words. */
    private void addStatus(Options o, List<Entry> offered, List<StreamMenuModel.Cell> statusCells, Map<String, String> labels) {
        if (statusCells.isEmpty()) return;
        boolean native_ = false;
        boolean quality = false;
        for (StreamMenuModel.Cell cell : statusCells) {
            native_ |= cell.id.equals(StreamMenuCatalog.id(Item.NATIVE_PAGE));
            quality |= cell.id.equals(StreamMenuCatalog.id(Item.BITRATE)) || cell.id.equals(StreamMenuCatalog.id(Item.RATE_CHANGE));
        }
        if (quality && o.bitrateKbps > 0 && o.bitrateChange != null) {
            LinearLayout sliders = sheet.addRowGroup();
            TextView[] shown = new TextView[1];
            java.util.function.IntUnaryOperator change = o.bitrateChange;
            bitrateSlider = BitrateSlider.inline(activity, o.bitrateKbps, text -> {
                if (shown[0] != null) shown[0].setText(text);
                // The bitrate row below shows the same rate: keep it in step with the slider.
                if (rows.get(Item.BITRATE) instanceof MenuRow bitrateRow) bitrateRow.setValue(text);
            }, change);
            shown[0] = sheet.addSliderRow(sliders, activity.getString(R.string.stream_bitrate), BitrateSlider.text(activity, o.bitrateKbps), bitrateSlider);
            bitrateShown = shown[0];
            bitrateSlider.setEnabled(!o.bitrateWaiting);
        }
        LinearLayout group = sheet.addRowGroup();
        String facts = o.connection == null ? null : o.connection.summary(activity);
        for (StreamMenuModel.Cell cell : statusCells) {
            Entry entry = StreamMenuCatalog.find(offered, cell.id);
            String value = null;
            if (entry.item == Item.NATIVE_PAGE || (entry.item == Item.OVERLAY_PAGE && !native_)) value = facts;
            // The rate-change row opens the frame-rate dialog; a host without it only has the bitrate row (nightjar #134).
            else if (entry.item == Item.RATE_CHANGE && o.streamFps > 0) value = o.streamFps + " FPS";
            else if (entry.item == Item.BITRATE && o.bitrateKbps > 0) value = BitrateSlider.text(activity, o.bitrateKbps);
            MenuRow row = sheet.addStatusRow(group, StreamMenuCatalog.label(activity, labels, cell.id), value,
                    cell.hotkey == null ? null : cell.hotkey.toUpperCase(Locale.ROOT));
            rows.put(entry.item, row);
            entries.add(entry);
            row.setOnClickListener(view -> choose(entry));
            cells[layout.grid.indexOf(cell)] = row;
        }
    }

    /** The view of the model's cell at this index on the first page. */
    android.view.View cell(int index) {
        return cells[index];
    }

    /**
     * The stream's bitrate while the menu is open: the slider and the bitrate row show it, without sending anything,
     * and the slider is usable or disabled (while the PC has not said what it counts the bitrate as).
     */
    public void showBitrate(int kbps, boolean usable) {
        if (bitrateSlider == null || kbps <= 0) return;
        bitrateSlider.setEnabled(usable);
        BitrateSlider.moveTo(bitrateSlider, kbps);
        String text = BitrateSlider.text(activity, kbps);
        if (bitrateShown != null) bitrateShown.setText(text);
        if (rows.get(Item.BITRATE) instanceof MenuRow bitrateRow) bitrateRow.setValue(text);
    }

    /** The first page's bitrate slider; null when it has none. */
    StepSlider bitrateSlider() {
        return bitrateSlider;
    }

    /** The string for a label key of the shared model ({@code stream_menu::label}), or 0 for a key this client lacks. */
    public static int tileLabel(String key) {
        return StreamMenuCatalog.tileLabel(key);
    }

    /**
     * The first page's title and the line under it, as desktop Nightjar's: the game over the stream's mode
     * ({@code 1440p · 144 FPS · HEVC}), or over the PC while the mode is unknown; without a game, the menu's title
     * over the app and PC.
     */
    String[] header(Options o) {
        return StreamMenuCatalog.header(activity, o);
    }

    /**
     * The round X back to the stream, then the ways out in the model's order and in its words ({@link #FALLBACK_FOOTER}, in
     * this client's words, when it failed); the last that ends something is the one red control. A way out only Android offers, a remote
     * monitor's, keeps its own words.
     */
    private void addFooter(List<Entry> offered, List<String> footer, Map<String, String> labels) {
        // The ways out follow the grid: their caps show only while a controller could press them.
        sheet.setCapsShown(capsShown());
        String back = labels != null ? StreamMenuCatalog.label(activity, labels, StreamMenuModel.CLOSE)
                : activity.getString(R.string.stream_menu_return);
        MaterialButton close = sheet.panelClose();
        close.setContentDescription(back + ", B");
        rows.put(Item.CLOSE, close);
        if (grid == null) sheet.setFirstAction(close);
        Entry red = null;
        for (Entry entry : offered) {
            if (entry.section == Section.SESSION && entry.destructive && (footer == null || footer.contains(StreamMenuCatalog.id(entry.item)))) red = entry;
        }
        for (Entry entry : offered) {
            if (entry.section != Section.SESSION || (footer != null && !footer.contains(StreamMenuCatalog.id(entry.item)))) continue;
            String name = labels != null && labels.containsKey(StreamMenuCatalog.id(entry.item))
                    ? StreamMenuCatalog.label(activity, labels, StreamMenuCatalog.id(entry.item)) : activity.getString(entry.label);
            int icon = entry == red ? R.drawable.ic_glass_power : entry.item == Item.DISCONNECT ? R.drawable.ic_stream_leave : entry.icon;
            MaterialButton button = sheet.addFooterAction(icon, name, entry == red, StreamMenuCatalog.footerHotkey(layout, entry.item));
            rows.put(entry.item, button);
            entries.add(entry);
            button.setOnClickListener(view -> choose(entry));
        }
    }

    /** A remote has no face buttons: the caps show only while a controller could press them. */
    private static boolean capsShown() {
        return gamepadForTest != null ? gamepadForTest : StreamMenuHint.hasGamepad();
    }

    /** What the user chose in Settings for the controller's buttons here: by button, the buttons left on Default out. */
    static Map<String, String> buttonChoices(android.content.Context context) {
        return StreamMenuCatalog.buttonChoices(context);
    }

    /**
     * A saved choice as this version reads it: Close game and Quit game became one choice, Quit game; End session moved under
     * More, where no button runs it, so a button that ran it is back on Default.
     */
    static String buttonChoice(String saved) {
        return StreamMenuCatalog.buttonChoice(saved);
    }

    /**
     * Saves an older install's close_game choice as quit_game, so the setting's list shows Quit game selected. Runs
     * before the settings screen reads the values; the menu reads either the same way.
     */
    public static void upgradeButtonChoices(android.content.SharedPreferences prefs) {
        StreamMenuCatalog.upgradeButtonChoices(prefs);
    }

    /** A controller button on the first page runs its tile, or a way out under the grid: Y, X, LB or RB. */
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
        if (index >= 0) {
            cells[index].performClick();
            return true;
        }
        String id = layout.buttons.get(key);
        for (Entry entry : entries) {
            MaterialButton row = rows.get(entry.item);
            if (row != null && StreamMenuCatalog.id(entry.item).equals(id)) {
                // From a button Disconnect asks first, as Quit, Close and End always do: a stray press must not end the stream.
                if (entry.item == Item.DISCONNECT) listener.onStreamMenuItem(Item.CONFIRM_DISCONNECT);
                else row.performClick();
                return true;
            }
        }
        return false;
    }

    /** A built-in's name in this language. */
    public static int builtInLabel(String id) {
        return StreamMenuCatalog.builtInLabel(id);
    }

    /** The editor's Save: the model checks the chord, then the list is saved with it added or replaced. */
    void saveDraft() {
        shortcutsPage.saveDraft();
    }

    // A page of rows: switches side by side at the top, then the rest under their sections.
    private void renderList(List<Entry> list) {
        entries = list;
        offeredItems = StreamMenuCatalog.items(list);
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
                if (entry.checked != null) StreamMenuCatalog.markChoice(button, entry.checked);
            }
            section = entry.section;
            rows.put(entry.item, button);
            button.setOnClickListener(view -> choose(entry));
        }
    }

    /**
     * Shows what changed while the menu is open, such as the host's answer about its audio or the stream's new size:
     * the switches and the first page's header in place, or the page again when items came or went.
     */
    public void refresh() {
        if ((page != Page.MAIN && page != Page.MORE) || !sheet.isShowing()) return;
        Options o = options.get();
        List<Item> now = StreamMenuCatalog.items(page == Page.MAIN ? offered(o) : StreamMenuCatalog.moreEntries(offered(o)));
        boolean noted = page == Page.MAIN && o.role == NvApp.Role.STREAM && o.hostAudioUnread;
        if (now.equals(offeredItems) && noted == (audioNote != null)) {
            refreshToggles();
            if (page == Page.MAIN) showHeader(o);
        } else {
            render(page, o, null);
        }
    }

    // The first page's title and mode line as render sets them; the title is set again only when it changed.
    private void showHeader(Options o) {
        String[] header = header(o);
        if (!android.text.TextUtils.equals(sheet.heading(), header[0])) sheet.setHeading(header[0]);
        sheet.setSubtitle(header[1]);
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
