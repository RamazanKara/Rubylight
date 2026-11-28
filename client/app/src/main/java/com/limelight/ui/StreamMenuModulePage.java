/**
 * One module's page of the Control Center for {@link StreamMenu}: the cards nightjar-policy's {@code control_center::dock}
 * gives the module (the user's shortcuts and Add at the end of Quick), a switch among its rows as one more card, the
 * bitrate as a wide card with its track inside, the rows that say something about the stream with their value, then the
 * rows that lead one level down. Quick starts with what the stream had to say when it started, Picture ends with the
 * video encoder's load while the PC reports it, and Clips with Instant Replay's state.
 */
package com.limelight.ui;

import android.app.Activity;
import android.view.View;
import android.widget.LinearLayout;
import android.widget.TextView;

import com.google.android.material.button.MaterialButton;
import com.limelight.R;
import com.limelight.binding.input.KeyboardShortcuts;
import com.limelight.ui.StreamMenu.Entry;
import com.limelight.ui.StreamMenu.Item;
import com.limelight.ui.StreamMenu.Options;
import com.limelight.ui.StreamMenu.Section;

import java.util.ArrayList;
import java.util.EnumMap;
import java.util.List;
import java.util.Locale;
import java.util.Map;

final class StreamMenuModulePage {
    /** What a page needs of the menu it is part of. */
    interface Host {
        /** A card or row was chosen. */
        void choose(Entry entry);

        /** A card or row now stands for {@code entry}. */
        void put(Entry entry, MaterialButton view);

        /** A saved shortcut's card was chosen. */
        void send(KeyboardShortcuts.Shortcut shortcut);

        /** Add was chosen. */
        void addShortcut();
    }

    private final Activity activity;
    // The Control Center's own context, night whatever the app's theme is: its cards are made in it.
    private final android.content.Context context;
    private final ControlCenterPage view;
    private final Host host;
    private final Map<Item, CharSequence> names = new EnumMap<>(Item.class);
    private TileGrid grid;
    // What each of the model's cards became: a card or a status row. Hotkeys go by the model's order.
    private View[] cells = new View[0];
    private StepSlider bitrateSlider;
    private TextView bitrateShown, audioNote, launchNotice, encoderLoad;
    private MenuRow bitrateRow;
    // What Clips shows of Instant Replay's state; null on another page.
    private String dvrShown;

    StreamMenuModulePage(Activity activity, android.content.Context context, ControlCenterPage view, Host host) {
        this.activity = activity;
        this.context = context;
        this.view = view;
        this.host = host;
    }

    void clear() {
        names.clear();
        grid = null;
        cells = new View[0];
        bitrateSlider = null;
        bitrateShown = audioNote = launchNotice = encoderLoad = null;
        bitrateRow = null;
        dvrShown = null;
    }

    TileGrid grid() {
        return grid;
    }

    /** The view of the model's card at this index on the page, or null for one this client has nothing for. */
    View cell(int index) {
        return index >= 0 && index < cells.length ? cells[index] : null;
    }

    StepSlider bitrateSlider() {
        return bitrateSlider;
    }

    TextView audioNote() {
        return audioNote;
    }

    /** The line with what the stream said when it started, on Quick; null without one. */
    TextView launchNotice() {
        return launchNotice;
    }

    /** The line with the video encoder's load, on Picture; null while the PC reports none. */
    TextView encoderLoad() {
        return encoderLoad;
    }

    /** Whether the page showing now has the lines this stream calls for: a refresh rebuilds it when it has not. */
    boolean notesMatch(String module, Options o) {
        boolean quick = ControlCenterModel.QUICK.equals(module);
        boolean audio = quick && o.role == com.limelight.nvstream.http.NvApp.Role.STREAM && o.hostAudioUnread;
        boolean notice = quick && o.launchNotice != null && !o.launchNotice.isEmpty();
        boolean encoder = ControlCenterModel.PICTURE.equals(module) && o.encoderLoad != null;
        String dvr = ControlCenterModel.CLIPS.equals(module) && o.dvr != null ? o.dvr.shown() : null;
        return audio == (audioNote != null) && notice == (launchNotice != null) && encoder == (encoderLoad != null)
                && java.util.Objects.equals(dvr, dvrShown);
    }

    /**
     * Whether anything of {@code page} can be shown: a module whose cards this client has nothing for stays off the rail,
     * Quick too unless the stream has something to say there.
     */
    static boolean shows(ControlCenterModel.Page page, List<Entry> offered, Options o) {
        if (ControlCenterModel.QUICK.equals(page.module) && ((o.launchNotice != null && !o.launchNotice.isEmpty())
                || (o.role == com.limelight.nvstream.http.NvApp.Role.STREAM && o.hostAudioUnread))) return true;
        for (StreamMenuModel.Cell card : page.cards) {
            if (card.kind != StreamMenuModel.Cell.Kind.TILE || StreamMenuCatalog.find(offered, card.id) != null) return true;
        }
        for (String id : page.rows) if (StreamMenuCatalog.find(offered, id) != null) return true;
        return false;
    }

    private String name(Entry entry, Map<String, String> labels) {
        String id = StreamMenuCatalog.id(entry.item);
        // A card says it in the short words both apps share; an item the model names no tile for keeps this client's.
        return labels.containsKey(id) ? StreamMenuCatalog.label(activity, labels, id) : activity.getString(entry.label);
    }

    void render(ControlCenterModel.Page page, Options o, List<Entry> offered, Map<String, String> labels,
                List<KeyboardShortcuts.Shortcut> saved, boolean capsShown) {
        boolean quick = ControlCenterModel.QUICK.equals(page.module);
        if (quick && o.launchNotice != null && !o.launchNotice.isEmpty()) {
            launchNotice = view.addNotice(R.drawable.ic_info, o.launchNotice);
        }
        if (quick && o.role == com.limelight.nvstream.http.NvApp.Role.STREAM && o.hostAudioUnread) {
            audioNote = view.addNote(view.addGroup(), activity.getString(R.string.stream_audio_unread), false);
        }
        grid = view.addCardGrid();
        cells = new View[page.cards.size()];
        List<StreamMenuModel.Cell> status = new ArrayList<>();
        for (int at = 0; at < page.cards.size(); at++) {
            StreamMenuModel.Cell cell = page.cards.get(at);
            TileGrid.Tile card;
            if (cell.kind == StreamMenuModel.Cell.Kind.SHORTCUT) {
                if (saved == null || cell.shortcut >= saved.size()) continue;
                KeyboardShortcuts.Shortcut shortcut = saved.get(cell.shortcut);
                card = ControlCenterCards.card(context, R.drawable.ic_keyboard, shortcut.name,
                        StreamMenuShortcutsPage.keysUnder(shortcut), null, null, capsShown, view.windowHeightDp());
                card.setOnClickListener(pressed -> host.send(shortcut));
            } else if (cell.kind == StreamMenuModel.Cell.Kind.ADD) {
                card = ControlCenterCards.card(context, R.drawable.ic_plus,
                        StreamMenuCatalog.label(activity, labels, StreamMenuModel.ADD), null, null, null, capsShown,
                        view.windowHeightDp());
                card.setOnClickListener(pressed -> host.addShortcut());
            } else {
                Entry entry = StreamMenuCatalog.find(offered, cell.id);
                // A card of a later feature this client has nothing behind yet.
                if (entry == null) continue;
                if (StreamMenuCatalog.isStatusRow(cell.id)) {
                    status.add(cell);
                    continue;
                }
                card = card(entry, name(entry, labels), cell.on, cell.hotkey, capsShown);
            }
            grid.addView(card);
            cells[at] = card;
            if (grid.getChildCount() == 1) view.setFirstAction(card);
        }
        // A switch among the rows is a card too: on and off read the same wherever the model put it.
        List<Entry> below = new ArrayList<>();
        for (String id : page.rows) {
            Entry entry = StreamMenuCatalog.find(offered, id);
            if (entry == null) continue;
            if (entry.section == Section.QUICK && entry.checked != null) {
                TileGrid.Tile card = card(entry, activity.getString(entry.label), entry.checked, null, capsShown);
                grid.addView(card);
                if (grid.getChildCount() == 1) view.setFirstAction(card);
            } else {
                below.add(entry);
            }
        }
        if (grid.getChildCount() == 0) grid.setVisibility(View.GONE);
        addStatus(page, o, offered, status, labels);
        if (ControlCenterModel.PICTURE.equals(page.module) && o.encoderLoad != null) {
            encoderLoad = view.addNotice(R.drawable.ic_speed, activity.getString(R.string.hardware_gpu_encoder, o.encoderLoad));
        }
        // Under Instant Replay's cards: the buffer, the recording's time, or why the host cannot save.
        if (ControlCenterModel.CLIPS.equals(page.module) && o.dvr != null) {
            dvrShown = o.dvr.shown();
            o.dvr.addNotes(view);
        }
        for (Entry entry : below) {
            // Never in red, even one that ends something: Quit game under the pages is the one red control.
            MaterialButton row = entry.opensPage() ? view.addPageAction(entry.icon, activity.getString(entry.label))
                    : view.addAction(entry.icon, activity.getString(entry.label));
            if (entry.checked != null) {
                if (entry.icon == R.drawable.ic_check) StreamMenuCatalog.markChoice(row, entry.checked);
                else StreamMenuCatalog.markToggleRow(row, entry.checked);
            }
            host.put(entry, row);
            row.setOnClickListener(pressed -> host.choose(entry));
        }
    }

    private TileGrid.Tile card(Entry entry, CharSequence name, Boolean on, String hotkey, boolean capsShown) {
        TileGrid.Tile card = ControlCenterCards.card(context, entry.icon, name,
                on == null ? null : ControlCenterCards.switchState(activity, on), on,
                hotkey == null ? null : hotkey.toUpperCase(Locale.ROOT), capsShown, view.windowHeightDp());
        names.put(entry.item, name);
        host.put(entry, card);
        card.setOnClickListener(pressed -> host.choose(entry));
        return card;
    }

    /** A switch's card shows its new state in place: amber and On, or plain and Off. */
    boolean refresh(Entry entry, MaterialButton button) {
        if (!(button instanceof TileGrid.Tile card) || !names.containsKey(entry.item) || entry.checked == null) return false;
        boolean on = Boolean.TRUE.equals(entry.checked);
        card.setChecked(on);
        card.setIconResource(entry.icon);
        ControlCenterCards.write(card, names.get(entry.item), ControlCenterCards.switchState(activity, on));
        return true;
    }

    // The bitrate's wide card and the rows with their values, under the cards: the same items, in the model's words.
    private void addStatus(ControlCenterModel.Page page, Options o, List<Entry> offered, List<StreamMenuModel.Cell> status,
                           Map<String, String> labels) {
        if (status.isEmpty()) return;
        boolean nativeOffered = StreamMenuCatalog.find(offered, StreamMenuCatalog.id(Item.NATIVE_PAGE)) != null;
        boolean quality = false;
        for (StreamMenuModel.Cell cell : status) {
            quality |= cell.id.equals(StreamMenuCatalog.id(Item.BITRATE)) || cell.id.equals(StreamMenuCatalog.id(Item.RATE_CHANGE));
        }
        if (quality && o.bitrateKbps > 0 && o.bitrateChange != null) {
            TextView[] shown = new TextView[1];
            bitrateSlider = BitrateSlider.inline(context, o.bitrateKbps, text -> {
                if (shown[0] != null) shown[0].setText(text);
                // The bitrate row below shows the same rate: keep it in step with the slider.
                if (bitrateRow != null) bitrateRow.setValue(text);
            }, o.bitrateChange);
            shown[0] = view.addSliderCard(activity.getString(R.string.stream_bitrate), BitrateSlider.text(activity, o.bitrateKbps), bitrateSlider);
            bitrateShown = shown[0];
            bitrateSlider.setEnabled(!o.bitrateWaiting);
        }
        LinearLayout group = view.addRowGroup();
        String facts = o.connection == null ? null : o.connection.summary(activity);
        for (StreamMenuModel.Cell cell : status) {
            Entry entry = StreamMenuCatalog.find(offered, cell.id);
            String value = null;
            // The stream's facts stand on the PC's own row where there is one, else on the overlay's: once in the menu.
            if (entry.item == Item.NATIVE_PAGE || (entry.item == Item.OVERLAY_PAGE && !nativeOffered)) value = facts;
            // The rate-change row opens the frame-rate dialog; a host without it only has the bitrate row (nightjar #134).
            else if (entry.item == Item.RATE_CHANGE && o.streamFps > 0) value = o.streamFps + " FPS";
            else if (entry.item == Item.BITRATE && o.bitrateKbps > 0) value = BitrateSlider.text(activity, o.bitrateKbps);
            MenuRow row = view.addStatusRow(group, name(entry, labels), value,
                    cell.hotkey == null ? null : cell.hotkey.toUpperCase(Locale.ROOT));
            if (entry.item == Item.BITRATE) bitrateRow = row;
            host.put(entry, row);
            row.setOnClickListener(pressed -> host.choose(entry));
            cells[page.cards.indexOf(cell)] = row;
        }
    }

    /** The stream's bitrate while the page is open: the slider and the bitrate row show it, without sending anything. */
    void showBitrate(int kbps, boolean usable) {
        if (bitrateSlider == null || kbps <= 0) return;
        bitrateSlider.setEnabled(usable);
        BitrateSlider.moveTo(bitrateSlider, kbps);
        String text = BitrateSlider.text(activity, kbps);
        if (bitrateShown != null) bitrateShown.setText(text);
        if (bitrateRow != null) bitrateRow.setValue(text);
    }

    /** The encoder's load as the PC says it now. */
    void showEncoderLoad(String percent) {
        if (encoderLoad != null && percent != null) encoderLoad.setText(activity.getString(R.string.hardware_gpu_encoder, percent));
    }
}
