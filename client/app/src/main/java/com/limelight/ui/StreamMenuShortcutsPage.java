package com.limelight.ui;

import android.app.Activity;
import android.widget.TextView;

import com.google.android.material.button.MaterialButton;
import com.limelight.R;
import com.limelight.binding.input.KeyboardShortcuts;

import java.util.ArrayList;
import java.util.Collections;
import java.util.HashMap;
import java.util.HashSet;
import java.util.List;
import java.util.Map;
import java.util.Set;

/**
 * Shortcuts page and inline shortcut editor for {@link StreamMenu}, rendering saved and built-in
 * chords, the key-cap picker, and chord validation against {@link StreamMenuModel}.
 */
final class StreamMenuShortcutsPage {
    private final Activity activity;
    private final ActionSheet sheet;
    private final StreamMenu.Shortcuts shortcuts;
    private final Runnable onBack;
    private final Runnable onOpenShortcuts;
    private final Runnable onOpenEditor;

    // The shortcut being edited (null for a new one), and the editor's name and keys in their saved names.
    private KeyboardShortcuts.Shortcut editing;
    private String draftName = "";
    private final List<String> draftKeys = new ArrayList<>();
    private TextView draftChord, draftError;
    private final Map<String, MaterialButton> keyCaps = new HashMap<>();

    StreamMenuShortcutsPage(Activity activity, ActionSheet sheet, StreamMenu.Shortcuts shortcuts,
                            Runnable onBack, Runnable onOpenShortcuts, Runnable onOpenEditor) {
        this.activity = activity;
        this.sheet = sheet;
        this.shortcuts = shortcuts;
        this.onBack = onBack;
        this.onOpenShortcuts = onOpenShortcuts;
        this.onOpenEditor = onOpenEditor;
    }

    void clearViews() {
        keyCaps.clear();
        draftChord = null;
        draftError = null;
    }

    int editorHeading() {
        return editing == null ? R.string.keyboard_shortcuts_add : R.string.keyboard_shortcuts_edit;
    }

    void edit(KeyboardShortcuts.Shortcut shortcut) {
        editing = shortcut;
        draftName = shortcut == null ? "" : shortcut.name;
        draftKeys.clear();
        if (shortcut != null) Collections.addAll(draftKeys, names(shortcut.keys()));
        onOpenEditor.run();
    }

    // The shortcuts page: the user's, in their order, then the built-ins not saved under a name of their own, Add, then a
    // row to edit each. The user's come first so a new built-in never pushes them along (nightjar#61).
    TextView renderShortcuts() {
        TextView failure = null;
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
            MaterialButton tile = tiles.addTile(R.drawable.ic_keyboard, activity.getString(StreamMenuCatalog.builtInLabel(builtIn.id)), builtIn.label, null, null);
            tile.setOnClickListener(view -> shortcuts.send(chord));
            if (tiles.getChildCount() == 1) sheet.setFirstAction(tile);
        }
        if (saved.size() < KeyboardShortcuts.MAX_SHORTCUTS) {
            MaterialButton add = tiles.addTile(R.drawable.ic_plus, activity.getString(R.string.keyboard_shortcuts_add), null, null, null);
            add.setOnClickListener(view -> edit(null));
            if (tiles.getChildCount() == 1) sheet.setFirstAction(add);
        }
        if (saved.isEmpty()) return failure;
        sheet.addSection(R.string.desktop_controls_edit_shortcuts);
        for (KeyboardShortcuts.Shortcut shortcut : saved) {
            MaterialButton row = sheet.addAction(R.drawable.ic_edit, shortcut.name, chordText(shortcut.keys()));
            row.setOnClickListener(view -> edit(shortcut));
        }
        return failure;
    }

    // The editor: a name (the keys' text when left empty), the chord so far, the key picker, then Save, Cancel and Delete.
    TextView renderEditor() {
        TextView failure = null;
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
        Set<String> offered = new HashSet<>();
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
        cancel.setOnClickListener(view -> onBack.run());
        if (editing != null) {
            KeyboardShortcuts.Shortcut target = editing;
            MaterialButton delete = sheet.addDestructiveAction(R.drawable.ic_delete, activity.getString(R.string.keyboard_shortcuts_delete));
            delete.setOnClickListener(view -> {
                List<KeyboardShortcuts.Shortcut> list = new ArrayList<>(shortcuts.saved());
                list.removeIf(s -> s.id.equals(target.id));
                shortcuts.save(list);
                onOpenShortcuts.run();
            });
        }
        return failure;
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
            onOpenShortcuts.run();
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

    /** The keys under a shortcut's name, or null when the name already says them (a shortcut saved unnamed). */
    static String keysUnder(KeyboardShortcuts.Shortcut shortcut) {
        String keys = chordText(shortcut.keys());
        return keys.equals(shortcut.name) ? null : keys;
    }

    // The keys' text the model gives, as Ctrl + Shift + Esc; the saved names when it cannot.
    static String chordText(int[] keys) {
        String[] names = names(keys);
        try {
            return StreamMenuModel.chordLabel(names);
        } catch (StreamMenuModel.Refused e) {
            return String.join(" + ", names);
        }
    }

    static String[] names(int[] keys) {
        return KeyboardShortcuts.format(keys).split(" \\+ ");
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
}
