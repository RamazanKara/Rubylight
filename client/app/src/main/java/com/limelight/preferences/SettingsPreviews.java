package com.limelight.preferences;

import android.content.Context;
import android.content.SharedPreferences;
import android.os.SystemClock;
import android.preference.ListPreference;
import android.preference.Preference;
import android.preference.TwoStatePreference;
import android.text.TextUtils;
import android.view.LayoutInflater;
import android.view.View;
import android.view.ViewGroup;
import android.widget.LinearLayout;
import android.widget.ScrollView;
import android.widget.TextView;

import androidx.appcompat.app.AlertDialog;

import com.google.android.material.color.MaterialColors;
import com.google.android.material.dialog.MaterialAlertDialogBuilder;
import com.limelight.R;
import com.limelight.ui.Motion;
import com.limelight.ui.NavigationDialogBuilder;
import com.limelight.ui.VideoScaling;
import com.limelight.utils.UiHelper;

import java.util.HashMap;
import java.util.Map;

/**
 * The settings whose choices are easier to see than to read, and their preview cards (SettingsPreviewCard): Frame
 * pacing, HDR, Upscaling (with its sharpness) and Video size. Their rows use settings_preference_preview, which shows
 * the picture of the current choice; their pickers show one card per choice. The settings, their keys and their
 * values are unchanged: only the control is new.
 */
final class SettingsPreviews {
    static final String FRAME_PACING = "frame_pacing", HDR = "checkbox_enable_hdr", UPSCALING = "upscaling_mode",
            SHARPNESS = "upscaling_sharpness";
    private static final int DEFAULT_SHARPNESS = 50;
    /** A row bound again this soon after its choice changed replays the change: the list builds a new row each time. */
    private static final long REPLAY_MS = 300;
    private static final int WIDE_DP = 600;

    private SettingsPreviews() { }

    /** A picked choice's place in the picker. */
    interface OnPick {
        void picked(int which);
    }

    /** What a list's row showed last, so the row that replaces it can spring from there. */
    private static final class Shown {
        boolean known;
        float state, from;
        long changed;
    }

    /** The picture a setting carries, or SettingsPreviewCard.NONE. */
    static int kindOf(String key) {
        if (FRAME_PACING.equals(key)) return SettingsPreviewCard.PACING;
        if (HDR.equals(key)) return SettingsPreviewCard.TONE;
        if (UPSCALING.equals(key)) return SettingsPreviewCard.EDGE;
        if (VideoScaling.PREF.equals(key)) return SettingsPreviewCard.SCALING;
        return SettingsPreviewCard.NONE;
    }

    static boolean has(String key) {
        return kindOf(key) != SettingsPreviewCard.NONE;
    }

    private static int sharpness(SharedPreferences prefs) {
        try {
            return prefs == null ? DEFAULT_SHARPNESS : prefs.getInt(SHARPNESS, DEFAULT_SHARPNESS);
        } catch (ClassCastException e) {
            return DEFAULT_SHARPNESS;
        }
    }

    /** The picture's state for each of a list's values, or for each mode of Video size (which has no values). */
    static float[] states(String key, CharSequence[] values, int count, SharedPreferences prefs) {
        float[] states = new float[count];
        int kind = kindOf(key);
        for (int i = 0; i < count; i++) {
            String value = values == null ? null : values[i].toString();
            states[i] = kind == SettingsPreviewCard.PACING ? PreviewPacing.choice(value)
                    : kind == SettingsPreviewCard.EDGE ? PreviewEdge.softness(value, sharpness(prefs)) : i;
        }
        return states;
    }

    /** The picture's state for what the setting holds now. */
    static float stored(Preference preference) {
        SharedPreferences prefs = preference.getSharedPreferences();
        String key = preference.getKey();
        if (HDR.equals(key)) {
            return preference instanceof TwoStatePreference && ((TwoStatePreference) preference).isChecked()
                    ? PreviewTone.HDR : PreviewTone.SDR;
        }
        if (VideoScaling.PREF.equals(key)) {
            return prefs == null ? VideoScaling.FIT : StreamSettingsSearchAndSummaries.storedVideoScaling(prefs);
        }
        String value = preference instanceof ListPreference ? ((ListPreference) preference).getValue() : null;
        return FRAME_PACING.equals(key) ? PreviewPacing.choice(value) : PreviewEdge.softness(value, sharpness(prefs));
    }

    /** What the picture means, in words: the setting's own help. */
    static CharSequence help(Preference preference) {
        if (VideoScaling.PREF.equals(preference.getKey())) return preference.getContext().getText(R.string.video_scaling_help);
        if (preference instanceof MaterialSwitchPreference) return ((MaterialSwitchPreference) preference).getExplanation();
        return preference instanceof ListPreference ? ((ListPreference) preference).getDialogMessage() : null;
    }

    /** The words on a setting's cards, for search: a choice's name finds its setting. */
    static String keywords(Preference preference) {
        if (!has(preference.getKey())) return "";
        CharSequence[] names = VideoScaling.PREF.equals(preference.getKey())
                ? preference.getContext().getResources().getTextArray(R.array.video_scaling_modes)
                : preference instanceof ListPreference ? ((ListPreference) preference).getEntries() : null;
        return names == null ? "" : " " + TextUtils.join(" ", names);
    }

    /**
     * Fills a row's card with its setting's picture. {@code list} is the list the row is built for: it remembers what
     * each setting showed, so a changed choice springs from the old picture although its row is a new view.
     */
    static void bind(Preference preference, View row, ViewGroup list) {
        SettingsPreviewCard card = row == null ? null : row.findViewById(R.id.settings_preview_card);
        String key = preference.getKey();
        int kind = kindOf(key);
        if (card == null || kind == SettingsPreviewCard.NONE) return;
        View chevron = row.findViewById(R.id.settings_preview_chevron);
        if (chevron != null) chevron.setVisibility(preference instanceof TwoStatePreference ? View.GONE : View.VISIBLE);
        card.setContentDescription(said(row, help(preference)));
        card.setEnabled(preference.isEnabled());
        float state = stored(preference);
        Shown shown = remembered(list, key);
        if (shown != null) {
            long now = SystemClock.uptimeMillis();
            if (!shown.known) {
                shown.known = true;
                shown.state = shown.from = state;
            } else if (shown.state != state) {
                shown.from = shown.state;
                shown.state = state;
                shown.changed = now;
            }
            if (shown.from != state && now - shown.changed < REPLAY_MS) card.show(key, kind, shown.from);
        }
        card.show(key, kind, state);
    }

    /**
     * The card in words: its caption, the value in use, and then the setting's own help. A screen reader says a
     * group's description instead of the text inside it, so the help alone would leave the value unsaid.
     */
    private static CharSequence said(View row, CharSequence help) {
        TextView caption = row.findViewById(android.R.id.summary);
        CharSequence value = caption == null || caption.getVisibility() != View.VISIBLE ? null : caption.getText();
        if (TextUtils.isEmpty(value)) return help;
        return TextUtils.isEmpty(help) ? value : value + ". " + help;
    }

    @SuppressWarnings("unchecked")
    private static Shown remembered(ViewGroup list, String key) {
        if (list == null) return null;
        Object tag = list.getTag(R.id.settings_preview_card);
        Map<String, Shown> all = tag instanceof Map ? (Map<String, Shown>) tag : new HashMap<>();
        if (tag != all) list.setTag(R.id.settings_preview_card, all);
        Shown shown = all.get(key);
        if (shown == null) all.put(key, shown = new Shown());
        return shown;
    }

    /** Cards beside each other in a picker: two on a phone held upright, one row of up to four where there is room. */
    static int columns(int screenWidthDp, int count) {
        if (screenWidthDp < WIDE_DP) return Math.min(2, count);
        return count <= 4 ? count : 3;
    }

    /** A list setting's picker of cards: the list's entries, each with the picture of its value. */
    static AlertDialog pick(ListPreference preference, Runnable help, OnPick onPick) {
        CharSequence[] names = preference.getEntries();
        return pick(preference.getContext(), preference.getTitle(), kindOf(preference.getKey()), names,
                states(preference.getKey(), preference.getEntryValues(), names.length, preference.getSharedPreferences()),
                preference.findIndexOfValue(preference.getValue()), help, onPick);
    }

    /**
     * Shows a picker whose choices are preview cards: the chosen one in the amber container, focus on it for a pad or
     * a remote, Help beside Cancel as in the list pickers. A tap or Select picks and closes.
     */
    static AlertDialog pick(Context context, CharSequence title, int kind, CharSequence[] names, float[] states,
            int selected, Runnable help, OnPick onPick) {
        int columns = columns(context.getResources().getConfiguration().screenWidthDp, names.length);
        int edge = context.getResources().getDimensionPixelSize(R.dimen.space_m);
        int between = context.getResources().getDimensionPixelSize(R.dimen.space_xs);
        LinearLayout grid = new LinearLayout(context);
        grid.setId(R.id.settings_preview_grid);
        grid.setOrientation(LinearLayout.VERTICAL);
        grid.setPadding(edge, edge, edge, between);
        grid.setClipChildren(false);
        grid.setClipToPadding(false);
        LayoutInflater inflater = LayoutInflater.from(context);
        AlertDialog[] shown = new AlertDialog[1];
        SettingsPreviewCard first = null;
        for (int start = 0; start < names.length; start += columns) {
            LinearLayout line = new LinearLayout(context);
            line.setBaselineAligned(false);
            line.setClipChildren(false);
            for (int column = 0; column < columns; column++) {
                int index = start + column;
                if (index >= names.length) {
                    // Use MATCH_PARENT height so LinearLayout keeps allFillParent == true and does not collapse
                    // an incomplete trailing row to this spacer's 0px alternativeMaxHeight inside ScrollView.
                    line.addView(new View(context), new LinearLayout.LayoutParams(
                            0, ViewGroup.LayoutParams.MATCH_PARENT, 1f));
                    continue;
                }
                SettingsPreviewCard card = (SettingsPreviewCard) inflater.inflate(R.layout.settings_preview_choice, line, false);
                ((ViewGroup.MarginLayoutParams) card.getLayoutParams()).setMargins(between, between, between, between);
                TextView name = card.findViewById(R.id.settings_preview_name);
                name.setText(names[index]);
                card.show(null, kind, states[index]);
                if (index == selected) {
                    card.setChosen(true);
                    name.setTextColor(MaterialColors.getColor(context, R.attr.glassOnSelected, name.getCurrentTextColor()));
                    first = card;
                }
                card.setStateListAnimator(Motion.controlFocus(context));
                card.setOnClickListener(view -> {
                    onPick.picked(index);
                    if (shown[0] != null) shown[0].dismiss();
                });
                line.addView(card);
            }
            grid.addView(line, new LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT));
        }
        ScrollView scroll = new ScrollView(context);
        scroll.setClipToPadding(false);
        scroll.setVerticalScrollBarEnabled(true);
        scroll.addView(grid, new android.widget.FrameLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT));
        MaterialAlertDialogBuilder builder = new NavigationDialogBuilder(context).setTitle(title).setView(scroll)
                .setNegativeButton(android.R.string.cancel, null);
        if (help != null) builder.setNeutralButton(R.string.help, null);
        AlertDialog dialog = builder.create();
        shown[0] = dialog;
        UiHelper.showDialog(context, dialog);
        // Like the list pickers, Help explains the choices without closing them.
        if (help != null) dialog.getButton(AlertDialog.BUTTON_NEUTRAL).setOnClickListener(view -> help.run());
        if (first != null) {
            if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.O) {
                for (int which : new int[] {AlertDialog.BUTTON_NEGATIVE, AlertDialog.BUTTON_NEUTRAL, AlertDialog.BUTTON_POSITIVE}) {
                    View button = dialog.getButton(which);
                    if (button != null) button.setFocusedByDefault(false);
                }
            }
            View initialFocus = first;
            initialFocus.requestFocus();
            scroll.post(() -> {
                if (!initialFocus.isInTouchMode() && !initialFocus.hasFocus()) initialFocus.requestFocus();
            });
        }
        return dialog;
    }
}
