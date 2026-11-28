package com.limelight.binding.input.pad;

import android.content.Context;
import android.content.res.ColorStateList;
import android.graphics.Color;
import android.graphics.Typeface;
import android.graphics.drawable.GradientDrawable;
import android.graphics.drawable.StateListDrawable;
import android.text.Editable;
import android.text.InputFilter;
import android.text.TextWatcher;
import android.util.TypedValue;
import android.view.Gravity;
import android.view.View;
import android.view.ViewGroup;
import android.widget.CheckBox;
import android.widget.EditText;
import android.widget.LinearLayout;
import android.widget.ScrollView;
import android.widget.TextView;

import com.limelight.R;
import com.limelight.nvstream.input.MouseButtonPacket;
import com.limelight.ui.NavigationDialogBuilder;

import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.Locale;

/**
 * Searchable dialog sheet for picking a {@link PadCustomButton} binding: FPS quick-pick chips,
 * full Windows {@code VK_*} 0x01..0xFF catalog with live search, mouse buttons and scroll wheel,
 * multi-key chord builder (up to 4 VKs), and custom short label with {@code >= 48dp} D-pad targets.
 */
final class PadKeyPickerSheet {
    interface OnPickedListener {
        void onPicked(PadCustomButton updated);
    }

    static final class Entry {
        final PadCustomButton.BindingKind kind;
        final int code;
        final String label;
        final String searchText;

        Entry(PadCustomButton.BindingKind kind, int code, String label, String aliases) {
            this.kind = kind;
            this.code = code;
            this.label = label;
            String hex = kind == PadCustomButton.BindingKind.KEY
                    ? String.format(Locale.US, " 0x%02x vk_%02x", code, code) : "";
            this.searchText = (label + " " + aliases + hex).toLowerCase(Locale.ROOT);
        }
    }

    private static final List<Entry> KEY_CATALOG = buildKeyCatalog();

    private PadKeyPickerSheet() {}

    static List<Entry> keyCatalog() {
        return KEY_CATALOG;
    }

    /** Filters the full {@code VK_*} catalog by name, alias, or hex code {@code 0xNN}. */
    static List<Entry> filterKeyEntries(String rawQuery) {
        String q = rawQuery == null ? "" : rawQuery.trim().toLowerCase(Locale.ROOT);
        if (q.isEmpty()) return KEY_CATALOG;
        List<Entry> matches = new ArrayList<>();
        for (Entry entry : KEY_CATALOG) {
            if (entry.searchText.contains(q)) {
                matches.add(entry);
            }
        }
        int parsedHex = parseHexVk(q);
        if (parsedHex > 0) {
            boolean found = false;
            for (Entry entry : matches) {
                if (entry.code == parsedHex) {
                    found = true;
                    break;
                }
            }
            if (!found) {
                matches.add(new Entry(PadCustomButton.BindingKind.KEY, parsedHex,
                        PadCustomButton.vkShortLabel(parsedHex), "vk custom hex"));
            }
        }
        return matches;
    }

    static void show(Context context, PadCustomButton source, OnPickedListener listener) {
        if (context == null || source == null || listener == null) return;
        PadCustomButton working = source.copy();
        float density = context.getResources().getDisplayMetrics().density;
        int pad = dp(density, 14);
        int gap = dp(density, 8);
        int minTouch = dp(density, 48);

        ScrollView scroll = new ScrollView(context);
        LinearLayout root = new LinearLayout(context);
        root.setOrientation(LinearLayout.VERTICAL);
        root.setPadding(pad, dp(density, 8), pad, pad);
        scroll.addView(root, new ViewGroup.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT));

        TextView currentSummary = makeSectionTitle(context, density,
                context.getString(R.string.pad_picker_current, working.defaultBindingLabel()));
        currentSummary.setTextSize(TypedValue.COMPLEX_UNIT_SP, 15);
        root.addView(currentSummary);

        EditText labelInput = new EditText(context);
        labelInput.setHint(R.string.pad_picker_label_hint);
        labelInput.setSingleLine(true);
        labelInput.setMinHeight(minTouch);
        labelInput.setFilters(new InputFilter[] {
                new InputFilter.LengthFilter(PadCustomButton.MAX_LABEL_LENGTH)
        });
        labelInput.setText(working.label);
        root.addView(labelInput, marginParams(0, gap, 0, gap));

        final Runnable[] refreshUi = new Runnable[1];

        root.addView(makeSectionTitle(context, density, context.getString(R.string.pad_picker_quick_fps)));
        LinearLayout quickWrap = new LinearLayout(context);
        quickWrap.setOrientation(LinearLayout.VERTICAL);
        root.addView(quickWrap, marginParams(0, dp(density, 4), 0, gap));
        populateChips(context, quickWrap, density, quickEntries(context), entry -> {
            applyEntry(working, entry, false);
            refreshUi[0].run();
        });

        root.addView(makeSectionTitle(context, density, context.getString(R.string.pad_picker_mouse)));
        LinearLayout mouseWrap = new LinearLayout(context);
        mouseWrap.setOrientation(LinearLayout.VERTICAL);
        root.addView(mouseWrap, marginParams(0, dp(density, 4), 0, gap));
        populateChips(context, mouseWrap, density, mouseEntries(context), entry -> {
            applyEntry(working, entry, false);
            refreshUi[0].run();
        });

        LinearLayout chordRow = new LinearLayout(context);
        chordRow.setOrientation(LinearLayout.HORIZONTAL);
        chordRow.setGravity(Gravity.CENTER_VERTICAL);
        CheckBox chordToggle = new CheckBox(context);
        chordToggle.setText(R.string.pad_picker_chord);
        chordToggle.setMinHeight(minTouch);
        chordToggle.setChecked(working.bindingKind == PadCustomButton.BindingKind.KEY_CHORD);
        LinearLayout.LayoutParams checkLp = new LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f);
        chordRow.addView(chordToggle, checkLp);

        TextView clearChord = makeChip(context, density, context.getString(R.string.pad_picker_clear_chord), false);
        clearChord.setOnClickListener(v -> {
            working.setKey(0x43);
            chordToggle.setChecked(false);
            refreshUi[0].run();
        });
        chordRow.addView(clearChord);
        root.addView(chordRow, marginParams(0, 0, 0, dp(density, 4)));

        root.addView(makeSectionTitle(context, density, context.getString(R.string.pad_picker_keyboard)));
        EditText searchInput = new EditText(context);
        searchInput.setHint(R.string.pad_picker_search_hint);
        searchInput.setSingleLine(true);
        searchInput.setMinHeight(minTouch);
        root.addView(searchInput, marginParams(0, dp(density, 4), 0, gap));

        LinearLayout keyList = new LinearLayout(context);
        keyList.setOrientation(LinearLayout.VERTICAL);
        root.addView(keyList, new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT));

        Runnable rebuildKeys = () -> populateChips(context, keyList, density,
                filterKeyEntries(searchInput.getText().toString()), entry -> {
                    applyEntry(working, entry, chordToggle.isChecked());
                    refreshUi[0].run();
                });

        refreshUi[0] = () -> {
            currentSummary.setText(context.getString(
                    R.string.pad_picker_current, working.defaultBindingLabel()));
            labelInput.setHint(working.defaultBindingLabel());
        };
        chordToggle.setOnCheckedChangeListener((btn, checked) -> {
            if (!checked && working.bindingKind == PadCustomButton.BindingKind.KEY_CHORD) {
                int[] ks = working.keys();
                working.setKey(ks.length > 0 ? ks[0] : 0x43);
                refreshUi[0].run();
            }
        });
        searchInput.addTextChangedListener(new TextWatcher() {
            @Override
            public void beforeTextChanged(CharSequence s, int start, int count, int after) {}

            @Override
            public void onTextChanged(CharSequence s, int start, int before, int count) {}

            @Override
            public void afterTextChanged(Editable s) {
                rebuildKeys.run();
            }
        });

        rebuildKeys.run();
        refreshUi[0].run();

        new NavigationDialogBuilder(context)
                .setTitle(R.string.pad_picker_title)
                .setView(scroll)
                .setNegativeButton(android.R.string.cancel, null)
                .setPositiveButton(R.string.pad_picker_apply, (dialog, which) -> {
                    working.setLabel(labelInput.getText().toString());
                    listener.onPicked(working);
                })
                .create()
                .show();
    }

    static void applyEntry(PadCustomButton target, Entry entry, boolean appendToChord) {
        if (target == null || entry == null) return;
        switch (entry.kind) {
            case MOUSE_BUTTON:
                target.setMouseButton(entry.code);
                return;
            case MOUSE_SCROLL:
                target.setScrollDirection(entry.code);
                return;
            case KEY:
            default:
                if (!appendToChord) {
                    target.setKey(entry.code);
                    return;
                }
                int[] existing = target.bindingKind == PadCustomButton.BindingKind.KEY_CHORD
                        ? target.keys() : new int[0];
                for (int vk : existing) {
                    if (vk == entry.code) return;
                }
                if (existing.length >= PadCustomButton.MAX_CHORD_KEYS) return;
                int[] next = new int[existing.length + 1];
                System.arraycopy(existing, 0, next, 0, existing.length);
                next[existing.length] = entry.code;
                target.setChord(next);
        }
    }

    private static List<Entry> quickEntries(Context c) {
        List<Entry> out = new ArrayList<>();
        out.add(new Entry(PadCustomButton.BindingKind.MOUSE_BUTTON, MouseButtonPacket.BUTTON_LEFT,
                c.getString(R.string.pad_picker_fire), "fire lmb left"));
        out.add(new Entry(PadCustomButton.BindingKind.MOUSE_BUTTON, MouseButtonPacket.BUTTON_RIGHT,
                c.getString(R.string.pad_picker_scope), "scope ads rmb right"));
        out.add(new Entry(PadCustomButton.BindingKind.KEY, 0x43,
                c.getString(R.string.pad_picker_crouch), "crouch c"));
        out.add(new Entry(PadCustomButton.BindingKind.KEY, 0x52,
                c.getString(R.string.pad_picker_reload), "reload r"));
        out.add(new Entry(PadCustomButton.BindingKind.KEY, 0x51,
                c.getString(R.string.pad_picker_peek_l), "peek lean left q"));
        out.add(new Entry(PadCustomButton.BindingKind.KEY, 0x45,
                c.getString(R.string.pad_picker_peek_r), "peek lean right e"));
        out.add(new Entry(PadCustomButton.BindingKind.KEY, 0x20,
                c.getString(R.string.pad_picker_jump), "jump space"));
        out.add(new Entry(PadCustomButton.BindingKind.KEY, 0xA0,
                c.getString(R.string.pad_picker_sprint), "sprint lshift shift"));
        out.add(new Entry(PadCustomButton.BindingKind.KEY, 0x46,
                c.getString(R.string.pad_picker_use), "use interact f"));
        out.add(new Entry(PadCustomButton.BindingKind.KEY, 0x09,
                c.getString(R.string.pad_picker_tab), "tab scoreboard"));
        out.add(new Entry(PadCustomButton.BindingKind.KEY, 0x1B,
                c.getString(R.string.pad_picker_esc), "esc escape menu"));
        out.add(new Entry(PadCustomButton.BindingKind.KEY, 0x31, "1", "weapon 1"));
        out.add(new Entry(PadCustomButton.BindingKind.KEY, 0x32, "2", "weapon 2"));
        out.add(new Entry(PadCustomButton.BindingKind.KEY, 0x33, "3", "weapon 3"));
        out.add(new Entry(PadCustomButton.BindingKind.KEY, 0x34, "4", "weapon 4"));
        return out;
    }

    private static List<Entry> mouseEntries(Context c) {
        List<Entry> out = new ArrayList<>();
        out.add(new Entry(PadCustomButton.BindingKind.MOUSE_BUTTON, MouseButtonPacket.BUTTON_LEFT, "LMB", "left"));
        out.add(new Entry(PadCustomButton.BindingKind.MOUSE_BUTTON, MouseButtonPacket.BUTTON_RIGHT, "RMB", "right"));
        out.add(new Entry(PadCustomButton.BindingKind.MOUSE_BUTTON, MouseButtonPacket.BUTTON_MIDDLE, "MMB", "middle"));
        out.add(new Entry(PadCustomButton.BindingKind.MOUSE_BUTTON, MouseButtonPacket.BUTTON_X1, "X1", "side back"));
        out.add(new Entry(PadCustomButton.BindingKind.MOUSE_BUTTON, MouseButtonPacket.BUTTON_X2, "X2", "side forward"));
        out.add(new Entry(PadCustomButton.BindingKind.MOUSE_SCROLL, 1,
                c.getString(R.string.pad_picker_scroll_up), "wheel up"));
        out.add(new Entry(PadCustomButton.BindingKind.MOUSE_SCROLL, -1,
                c.getString(R.string.pad_picker_scroll_down), "wheel down"));
        return out;
    }

    private interface EntryClick {
        void onClick(Entry entry);
    }

    private static void populateChips(Context context, LinearLayout container, float density,
                                      List<Entry> entries, EntryClick click) {
        container.removeAllViews();
        int perRow = 3;
        int gap = dp(density, 6);
        LinearLayout row = null;
        for (int i = 0; i < entries.size(); i++) {
            if (i % perRow == 0) {
                row = new LinearLayout(context);
                row.setOrientation(LinearLayout.HORIZONTAL);
                container.addView(row, marginParams(0, 0, 0, gap));
            }
            Entry entry = entries.get(i);
            TextView chip = makeChip(context, density, entry.label, false);
            chip.setOnClickListener(v -> click.onClick(entry));
            LinearLayout.LayoutParams lp = new LinearLayout.LayoutParams(
                    0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f);
            if (i % perRow < perRow - 1) lp.setMargins(0, 0, gap, 0);
            row.addView(chip, lp);
        }
        int rem = entries.size() % perRow;
        if (row != null && rem != 0) {
            for (int i = rem; i < perRow; i++) {
                View spacer = new View(context);
                row.addView(spacer, new LinearLayout.LayoutParams(0, 1, 1f));
            }
        }
    }

    private static TextView makeSectionTitle(Context context, float density, String text) {
        TextView tv = new TextView(context);
        tv.setText(text);
        tv.setTypeface(Typeface.DEFAULT_BOLD);
        tv.setTextSize(TypedValue.COMPLEX_UNIT_SP, 12);
        tv.setPadding(0, dp(density, 6), 0, dp(density, 4));
        return tv;
    }

    private static TextView makeChip(Context context, float density, String text, boolean selected) {
        TextView chip = new TextView(context);
        chip.setText(text);
        chip.setGravity(Gravity.CENTER);
        chip.setSingleLine(true);
        chip.setTextSize(TypedValue.COMPLEX_UNIT_SP, 12);
        chip.setMinWidth(dp(density, 48));
        chip.setMinHeight(dp(density, 48));
        int hPad = dp(density, 8);
        int vPad = dp(density, 6);
        chip.setPadding(hPad, vPad, hPad, vPad);
        chip.setClickable(true);
        chip.setFocusable(true);
        chip.setBackground(chipBackground(density, selected));
        chip.setTextColor(ColorStateList.valueOf(resolveTextColor(context)));
        return chip;
    }

    private static StateListDrawable chipBackground(float density, boolean selected) {
        StateListDrawable states = new StateListDrawable();
        float r = dp(density, 10);
        GradientDrawable focused = roundedRect(r, Color.argb(68, 96, 165, 250),
                dp(density, 2), Color.argb(240, 147, 197, 253));
        GradientDrawable pressed = roundedRect(r, Color.argb(96, 96, 165, 250),
                dp(density, 1), Color.argb(200, 147, 197, 253));
        GradientDrawable normal = roundedRect(r,
                selected ? Color.argb(64, 96, 165, 250) : Color.argb(28, 255, 255, 255),
                dp(density, 1), Color.argb(72, 255, 255, 255));
        states.addState(new int[] {android.R.attr.state_pressed}, pressed);
        states.addState(new int[] {android.R.attr.state_focused}, focused);
        states.addState(new int[0], normal);
        return states;
    }

    private static GradientDrawable roundedRect(float radius, int fill, int strokeW, int stroke) {
        GradientDrawable d = new GradientDrawable();
        d.setCornerRadius(radius);
        d.setColor(fill);
        d.setStroke(strokeW, stroke);
        return d;
    }

    private static int resolveTextColor(Context context) {
        TypedValue tv = new TypedValue();
        if (context.getTheme().resolveAttribute(android.R.attr.textColorPrimary, tv, true)
                && tv.type >= TypedValue.TYPE_FIRST_COLOR_INT
                && tv.type <= TypedValue.TYPE_LAST_COLOR_INT) {
            return tv.data;
        }
        return Color.WHITE;
    }

    private static LinearLayout.LayoutParams marginParams(int l, int t, int r, int b) {
        LinearLayout.LayoutParams lp = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        lp.setMargins(l, t, r, b);
        return lp;
    }

    private static int dp(float density, int dp) {
        return Math.round(dp * Math.max(1f, density));
    }

    private static int parseHexVk(String query) {
        String s = query;
        if (s.startsWith("vk_")) s = s.substring(3);
        if (s.startsWith("0x")) s = s.substring(2);
        else return -1;
        try {
            int code = Integer.parseInt(s, 16);
            return code >= 1 && code <= 0xFF ? code : -1;
        } catch (NumberFormatException ignored) {
            return -1;
        }
    }

    private static List<Entry> buildKeyCatalog() {
        List<Entry> list = new ArrayList<>();
        addVk(list, 0x1B, "Esc", "escape menu");
        addVk(list, 0x09, "Tab", "scoreboard");
        addVk(list, 0x20, "Space", "jump spacebar");
        addVk(list, 0xA0, "LShift", "left shift sprint");
        addVk(list, 0xA1, "RShift", "right shift");
        addVk(list, 0xA2, "LCtrl", "left control ctrl crouch");
        addVk(list, 0xA3, "RCtrl", "right control ctrl");
        addVk(list, 0xA4, "LAlt", "left alt menu");
        addVk(list, 0xA5, "RAlt", "right alt altgr");
        addVk(list, 0x5B, "LWin", "left windows meta super");
        addVk(list, 0x5C, "RWin", "right windows meta super");
        addVk(list, 0x0D, "Enter", "return");
        addVk(list, 0x08, "Backspace", "back");
        addVk(list, 0x14, "CapsLock", "caps");
        addVk(list, 0x2D, "Insert", "ins");
        addVk(list, 0x2E, "Delete", "del");
        addVk(list, 0x24, "Home", "home");
        addVk(list, 0x23, "End", "end");
        addVk(list, 0x21, "PgUp", "page up");
        addVk(list, 0x22, "PgDn", "page down");
        addVk(list, 0x25, "Left", "arrow left");
        addVk(list, 0x26, "Up", "arrow up");
        addVk(list, 0x27, "Right", "arrow right");
        addVk(list, 0x28, "Down", "arrow down");
        addVk(list, 0x2C, "PrtSc", "print screen screenshot");
        addVk(list, 0x13, "Pause", "break");
        addVk(list, 0x90, "NumLock", "num");
        addVk(list, 0x91, "ScrollLock", "scr");
        addVk(list, 0x5D, "Menu", "apps context");
        for (char c = 'A'; c <= 'Z'; c++) {
            addVk(list, c, String.valueOf(c), "letter " + c);
        }
        for (char d = '0'; d <= '9'; d++) {
            addVk(list, d, String.valueOf(d), "digit number " + d);
        }
        for (int f = 1; f <= 24; f++) {
            addVk(list, 0x6F + f, "F" + f, "function f" + f);
        }
        for (int n = 0; n <= 9; n++) {
            addVk(list, 0x60 + n, "Num" + n, "numpad " + n);
        }
        addVk(list, 0x6A, "Num*", "numpad multiply");
        addVk(list, 0x6B, "Num+", "numpad add plus");
        addVk(list, 0x6D, "Num-", "numpad subtract minus");
        addVk(list, 0x6E, "Num.", "numpad decimal dot");
        addVk(list, 0x6F, "Num/", "numpad divide slash");
        addVk(list, 0xC0, "` (~)", "tilde backtick grave console");
        addVk(list, 0xBD, "- (_)", "minus dash underscore");
        addVk(list, 0xBB, "= (+)", "equals plus");
        addVk(list, 0xDB, "[", "left bracket");
        addVk(list, 0xDD, "]", "right bracket");
        addVk(list, 0xDC, "\\", "backslash pipe");
        addVk(list, 0xBA, ";", "semicolon");
        addVk(list, 0xDE, "'", "apostrophe quote");
        addVk(list, 0xBC, ",", "comma");
        addVk(list, 0xBE, ".", "period dot");
        addVk(list, 0xBF, "/", "slash question");
        addVk(list, 0xAD, "Mute", "audio volume mute");
        addVk(list, 0xAE, "Vol-", "audio volume down");
        addVk(list, 0xAF, "Vol+", "audio volume up");
        addVk(list, 0xB3, "Play/Pause", "media");
        return Collections.unmodifiableList(list);
    }

    private static void addVk(List<Entry> list, int vk, String label, String aliases) {
        list.add(new Entry(PadCustomButton.BindingKind.KEY, vk, label, aliases));
    }
}
