package com.limelight.preferences;

import android.app.Activity;
import android.content.ActivityNotFoundException;
import android.content.Context;
import android.content.Intent;
import android.content.SharedPreferences;
import android.content.res.XmlResourceParser;
import android.preference.ListPreference;
import android.preference.Preference;
import android.preference.PreferenceCategory;
import android.preference.PreferenceGroup;
import android.preference.PreferenceManager;
import android.text.TextUtils;
import android.view.LayoutInflater;
import android.view.View;
import android.view.ViewGroup;
import android.widget.ListView;
import android.widget.TextView;
import android.widget.Toast;

import androidx.core.view.ViewCompat;

import com.limelight.LimeLog;
import com.limelight.R;
import com.limelight.binding.input.touch.TouchMode;
import com.limelight.binding.video.MediaCodecDecoderRenderer;
import com.limelight.ui.AppTheme;
import com.limelight.ui.GlassForm;
import com.limelight.ui.GlassHost;
import com.limelight.ui.NightSkyView;
import com.limelight.ui.ShellBar;
import com.limelight.ui.VideoScaling;

import org.xmlpull.v1.XmlPullParser;
import org.xmlpull.v1.XmlPullParserException;

import java.io.File;
import java.io.FileInputStream;
import java.io.IOException;
import java.io.OutputStream;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;

/**
 * Moonlit Glass chrome, header/chips layout, preference styling, latency export, and reset
 * helpers for {@link StreamSettings}.
 */
final class StreamSettingsChrome {
    // Kept by a reset: the language is applied by the OS, the others are actions
    private static final List<String> RESET_EXCLUDED_KEYS = Arrays.asList(
            PreferenceConfiguration.LANGUAGE_PREF_STRING, "export_latency_csv", "about_app", "check_updates_now", "gpu_driver");

    private final StreamSettings activity;
    com.limelight.console.PageChrome chrome;
    /** What a list keeps free under its last row: the phone's tab bar. */
    int listFoot;

    StreamSettingsChrome(StreamSettings activity) {
        this.activity = activity;
    }

    /** The chrome's search takes the place of the bar's menu item; the toolbar only carries the back arrow and a title. */
    void setUpGlass() {
        GlassHost host = activity.findViewById(R.id.glass_host);
        Context full = activity.fullSizeContext();
        LayoutInflater.from(full).inflate(R.layout.view_settings_chrome, host, true);
        ShellBar.install(activity, ShellBar.Destination.SETTINGS);
        View search = activity.findViewById(R.id.settings_search_button);
        search.setOnClickListener(v -> activity.openSearch());
        GlassForm form = GlassForm.of(activity);
        if (form == GlassForm.PHONE) {
            android.content.res.Resources res = full.getResources();
            ((ViewGroup) search.getParent()).removeView(search);
            int size = res.getDimensionPixelSize(R.dimen.nj_size_tab_bar_height);
            android.widget.LinearLayout.LayoutParams params = new android.widget.LinearLayout.LayoutParams(size, size);
            params.setMarginStart(res.getDimensionPixelSize(R.dimen.space_s));
            ((ViewGroup) activity.findViewById(R.id.settings_bottom_row)).addView(search, params);
            activity.findViewById(R.id.settings_chrome).setVisibility(View.GONE);
        }
        NightSkyView sky = activity.findViewById(R.id.settings_sky);
        // A rail and a pane fill the screen: the moon would stand behind the rows' text. A phone's stands over its title.
        if (form != GlassForm.PHONE) sky.setMoonShown(false);
        sky.setMoonAtEnd();
        host.addOnLayoutChangeListener((v, l, t, r, b, ol, ot, or, ob) -> {
            if (r - l != or - ol || b - t != ob - ot) host.setAmbient(sky.bake(r - l, b - t), false, 0);
        });
        chrome = new com.limelight.console.PageChrome(activity, host, R.id.settings_tabs, R.id.settings_top_row,
                R.id.settings_bottom_row, R.id.settings_foot_fade, this::layOutUnderChrome);
        if (form != GlassForm.PHONE) ShellBar.linkTopRow(activity, R.id.settings_search_button);
    }

    /** The panes take what the chrome and the window leave. The chrome is at full size, the panes may be compact. */
    private void layOutUnderChrome(int left, int top, int right, int bottom) {
        android.content.res.Resources res = activity.fullSizeContext().getResources();
        float density = res.getDisplayMetrics().density;
        GlassForm form = chrome != null ? chrome.form() : GlassForm.of(activity);
        ((NightSkyView) activity.findViewById(R.id.settings_sky)).setSafeTop(top);
        View frame = activity.findViewById(R.id.settings_frame);
        if (form == GlassForm.PHONE) {
            frame.setPadding(left, top + Math.round(24 * density), right, 0);
            View bar = activity.findViewById(R.id.settings_bottom_row);
            int edge = res.getDimensionPixelSize(R.dimen.space_m);
            bar.setPadding(left + edge, 0, right + edge, bottom + res.getDimensionPixelSize(R.dimen.space_s));
            listFoot = bottom + res.getDimensionPixelSize(R.dimen.nj_size_tab_bar_height) + 3 * edge;
            View fade = activity.findViewById(R.id.settings_foot_fade);
            ViewGroup.LayoutParams size = fade.getLayoutParams();
            size.height = res.getDimensionPixelSize(R.dimen.home_foot_fade) + bottom;
            fade.setLayoutParams(size);
        } else {
            boolean television = form == GlassForm.TELEVISION;
            int inset = television ? 0 : Math.round(24 * density);
            int rowTop = television ? top : top + res.getDimensionPixelSize(R.dimen.space_m);
            activity.findViewById(R.id.settings_top_row).setPaddingRelative(left + inset, rowTop, right + inset, 0);
            int side = res.getDimensionPixelSize(R.dimen.shell_overscan_horizontal);
            int vertical = res.getDimensionPixelSize(R.dimen.shell_overscan_vertical);
            // The group's edge stands under the tabs' edge: the list keeps its gutter, so the frame gives it back.
            int gutter = activity.getResources().getDimensionPixelSize(R.dimen.screen_gutter);
            frame.setPaddingRelative(Math.max(0, left + side + inset - gutter),
                    rowTop + vertical + res.getDimensionPixelSize(R.dimen.glass_capsule) + res.getDimensionPixelSize(R.dimen.space_m),
                    right + side + inset, 0);
            listFoot = bottom + vertical;
        }
        applyListFoot(activity.listOf(R.id.stream_settings));
        applyListFoot(activity.listOf(R.id.settings_sections));
    }

    private void applyListFoot(ListView list) {
        if (list == null) return;
        // A handheld's rail ends on the screen's edge: nothing floats under it, and every row can fit.
        GlassForm form = chrome != null ? chrome.form() : GlassForm.of(activity);
        boolean handheldRail = list == activity.listOf(R.id.settings_sections) && form == GlassForm.HANDHELD;
        boolean pane = activity.twoPane && list == activity.listOf(R.id.stream_settings);
        int foot = handheldRail ? activity.getResources().getDimensionPixelSize(R.dimen.space_xs) : contentFoot(pane, form);
        list.setPadding(list.getPaddingLeft(), list.getPaddingTop(), list.getPaddingRight(), foot);
    }

    /**
     * What a list keeps under its last row. The right pane beside the rail ends on the screen's edge too: a ListView
     * fills its padding while scrolling down (it does not clip to it) but drops the rows there on the way up, so a deep
     * foot showed a row going down and a blank band, with the group's card cut, coming back up.
     */
    int contentFoot(boolean rightPane, GlassForm form) {
        // A TV's pane only: a handheld's foot keeps the system inset, or its last row ends under the navigation bar.
        if (rightPane && form == GlassForm.TELEVISION) return activity.getResources().getDimensionPixelSize(R.dimen.space_xs);
        return listFoot;
    }

    void updateHeader() {
        boolean overview = activity.section == null || activity.twoPane;
        CharSequence title = activity.getString(!activity.searchQuery.isEmpty() ? R.string.settings_search_results :
                overview ? R.string.settings : StreamSettings.sectionTitle(activity.section));
        if (activity.searchQuery.isEmpty()) {
            activity.contextLabel.setText(overview ? R.string.settings_global_context : R.string.settings_changes_saved);
        } else {
            android.app.Fragment fragment = activity.getFragmentManager().findFragmentById(R.id.stream_settings);
            if (fragment instanceof StreamSettings.SettingsFragment) {
                int count = ((StreamSettings.SettingsFragment) fragment).displayedResultCount;
                activity.contextLabel.setText(activity.getResources().getQuantityString(R.plurals.settings_search_count, count, count));
            }
        }
        // The shell bar belongs to the list of sections, and its Settings tab is the title there; a section or the search
        // results keep the back arrow and their title.
        boolean atRoot = overview && activity.searchQuery.isEmpty();
        activity.toolbar.setTitle(atRoot ? null : title);
        activity.setTitle(title);
        activity.toolbar.setNavigationContentDescription(activity.returnSearchQuery != null ? R.string.settings_back_to_search :
                overview ? R.string.settings_navigate_up : R.string.settings_back_to_overview);
        // The words under the title count search results; everything else speaks for itself.
        activity.contextLabel.setVisibility(activity.searchQuery.isEmpty() ? View.GONE : View.VISIBLE);
        TextView large = activity.findViewById(R.id.settings_title);
        if (activity.twoPane) {
            large.setText(activity.searchQuery.isEmpty() && activity.section != null
                    ? StreamSettings.sectionTitle(activity.section) : R.string.settings_search_results);
        }
        updateNavigation();
        updateChips();
    }

    /** Where a phone's section chips stand and which is selected. */
    void updateChips() {
        ViewGroup chips = activity.findViewById(R.id.settings_chips);
        if (chips == null) return;
        boolean shown = activity.section != null && activity.searchQuery.isEmpty();
        activity.findViewById(R.id.settings_chips_scroll).setVisibility(shown ? View.VISIBLE : View.GONE);
        if (!shown) return;
        List<String> sections = visibleSections();
        List<Object> built = new ArrayList<>();
        for (int i = 0; i < chips.getChildCount(); i++) built.add(chips.getChildAt(i).getTag());
        // Debug comes and goes with Show debug settings: the chips follow.
        if (!built.equals(new ArrayList<Object>(sections))) chips.removeAllViews();
        if (chips.getChildCount() == 0) {
            LayoutInflater inflater = LayoutInflater.from(activity);
            for (String key : sections) {
                TextView chip = (TextView) inflater.inflate(R.layout.settings_chip, chips, false);
                chip.setText(StreamSettings.sectionTitle(key));
                chip.setTag(key);
                chip.setOnClickListener(v -> {
                    if (!key.equals(activity.section)) activity.showSection(key);
                });
                chips.addView(chip);
            }
        }
        for (int i = 0; i < chips.getChildCount(); i++) {
            View chip = chips.getChildAt(i);
            boolean selected = chip.getTag().equals(activity.section);
            chip.setSelected(selected);
            ViewCompat.setStateDescription(chip, selected ? activity.getString(R.string.shell_current_page) : null);
            if (selected) chip.post(() -> chip.requestRectangleOnScreen(new android.graphics.Rect(0, 0, chip.getWidth(), chip.getHeight()), true));
        }
    }

    /** The sections a chip stands for, as the list of sections has them. */
    private List<String> visibleSections() {
        List<String> sections = new ArrayList<>(StreamSettings.RAIL_SECTIONS);
        if (!activity.debugListed()) sections.remove("debug");
        return sections;
    }

    /**
     * Opened from the shell bar, the list of sections is a top-level page like PCs and Home, so it has no back arrow;
     * a section, the search field or its results bring the arrow back. Opened any other way, the arrow always shows.
     */
    void updateNavigation() {
        boolean topLevel = activity.fromShell && activity.searchQuery.isEmpty() && activity.returnSearchQuery == null
                && activity.searchView.getVisibility() != View.VISIBLE && (activity.section == null || activity.twoPane);
        // A top-level page needs no bar at all: its tab names it, and a phone's large title stands in its place.
        activity.toolbar.setVisibility(topLevel ? View.GONE : View.VISIBLE);
        if (!activity.twoPane) {
            activity.findViewById(R.id.settings_title).setVisibility(
                    activity.section == null && activity.searchQuery.isEmpty() ? View.VISIBLE : View.GONE);
        }
        if (topLevel == (activity.toolbar.getNavigationIcon() == null)) return;
        activity.toolbar.setNavigationIcon(topLevel ? null
                : androidx.appcompat.content.res.AppCompatResources.getDrawable(activity.toolbar.getContext(), R.drawable.ic_arrow_back));
    }

    void linkListsToSearch() {
        linkListToSearch(activity.listOf(R.id.settings_sections), activity.searchView);
        linkListToSearch(activity.listOf(R.id.stream_settings), activity.searchView);
    }

    /**
     * An empty search field closes once focus leaves it: open beside the capsule's search it showed search twice while
     * nothing was searched (nightjar#7). A field with a query stays; it is the search on screen.
     */
    void closeEmptySearch() {
        if (!activity.searchQuery.isEmpty() || activity.searchView.getVisibility() != View.VISIBLE || activity.searchView.hasFocus()) {
            return;
        }
        activity.searchView.setVisibility(View.GONE);
        updateNavigation();
        linkListsToSearch();
    }

    void exportLatencyCsv() {
        if (!new File(activity.getFilesDir(), MediaCodecDecoderRenderer.LATENCY_CSV_NAME).isFile()) {
            Toast.makeText(activity, R.string.latency_csv_empty, Toast.LENGTH_LONG).show();
            return;
        }
        Intent intent = new Intent(Intent.ACTION_CREATE_DOCUMENT);
        intent.addCategory(Intent.CATEGORY_OPENABLE);
        intent.setType("text/csv");
        intent.putExtra(Intent.EXTRA_TITLE, MediaCodecDecoderRenderer.LATENCY_CSV_EXPORT_NAME);
        try {
            activity.startActivityForResult(intent, StreamSettings.EXPORT_LATENCY_REQUEST);
        } catch (ActivityNotFoundException e) {
            Toast.makeText(activity, R.string.latency_csv_no_picker, Toast.LENGTH_LONG).show();
        }
    }

    void handleExportLatencyResult(int requestCode, int resultCode, Intent data) {
        if (requestCode != StreamSettings.EXPORT_LATENCY_REQUEST || resultCode != Activity.RESULT_OK
                || data == null || data.getData() == null) {
            return;
        }
        new Thread(new Runnable() {
            @Override
            public void run() {
                int message = R.string.latency_csv_saved;
                try (FileInputStream input = new FileInputStream(new File(activity.getFilesDir(), MediaCodecDecoderRenderer.LATENCY_CSV_NAME));
                     OutputStream output = activity.getContentResolver().openOutputStream(data.getData(), "wt")) {
                    if (output == null) {
                        throw new IOException("Document provider did not open the CSV");
                    }
                    byte[] buffer = new byte[16384];
                    int count;
                    while ((count = input.read(buffer)) != -1) {
                        output.write(buffer, 0, count);
                    }
                } catch (IOException | SecurityException e) {
                    LimeLog.warning("Unable to export latency CSV: " + e);
                    message = R.string.latency_csv_failed;
                }
                final int resultMessage = message;
                activity.runOnUiThread(new Runnable() {
                    @Override
                    public void run() {
                        Toast.makeText(activity, resultMessage, Toast.LENGTH_LONG).show();
                    }
                });
            }
        }, "Latency CSV export").start();
    }

    // Removes every setting shown in these screens, then restores the XML defaults
    static void resetAllSettings(Context context) {
        SharedPreferences prefs = PreferenceManager.getDefaultSharedPreferences(context);
        SharedPreferences.Editor editor = prefs.edit();
        try (XmlResourceParser parser = context.getResources().getXml(R.xml.preferences)) {
            for (int event = parser.getEventType(); event != XmlPullParser.END_DOCUMENT; event = parser.next()) {
                if (event == XmlPullParser.START_TAG) {
                    String key = parser.getAttributeValue("http://schemas.android.com/apk/res/android", "key");
                    if (key != null && !RESET_EXCLUDED_KEYS.contains(key)) {
                        editor.remove(key);
                    }
                }
            }
        } catch (XmlPullParserException | IOException e) {
            LimeLog.warning("Unable to read preference keys: " + e);
            return;
        }
        // Stored by the frame rate, overlay, theme, touch mode, video scaling and paired-switch pickers, which
        // have no row of their own for them.
        editor.remove("checkbox_native_touch")
                .remove(TouchMode.PREF)
                .remove(TouchMode.TRACKPAD_PREF)
                .remove(VideoScaling.PREF)
                .remove(VideoScaling.STRETCH_PREF)
                .remove(PreferenceConfiguration.UNLOCK_FPS_STRING)
                .remove(PreferenceConfiguration.OVERLAY_MODE_PREF_STRING)
                .remove(AppTheme.RETIRED_OLED_BLACK)
                // Display arrangement's switch (DisplayArrangementChoice).
                .remove(DisplayArrangementChoice.VIRTUAL_DISPLAY_PREF)
                .remove(AppTheme.RETIRED_ALWAYS_DARK)
                // The second switches of the rows that are one choice now (SwitchPairChoice).
                .remove(PreferenceConfiguration.BACK_OPENS_MENU_PREF_STRING)
                .remove(PreferenceConfiguration.BACK_AS_GUIDE_PREF_STRING)
                .remove(PreferenceConfiguration.SYSTEM_KEYS_FULLSCREEN_ONLY_PREF_STRING)
                .remove(PreferenceConfiguration.ROAMING_METERED_PREF_STRING)
                // Lower bitrate's switches before it was one value (AutoBitrateChoice).
                .remove(AutoBitrateChoice.RETIRED_BUSY)
                .remove(AutoBitrateChoice.RETIRED_LOST)
                .remove(AutoBitrateChoice.RETIRED_KEEP)
                // The switches Vibration, Trackpad acceleration and Glide after a flick folded into their lists (FoldedChoice).
                .remove(FoldedChoice.VIBRATION.switchKey)
                .remove(FoldedChoice.ACCELERATION.switchKey)
                .remove(FoldedChoice.MOMENTUM.switchKey)
                .remove(FoldedChoice.RETIRED_ACCELERATION_V1)
                // Stored by the one switch that tells the PC about this device's network, beside its row's key.
                .remove(PreferenceConfiguration.TELL_HOST_LINK_SPEED_PREF_STRING)
                .remove(PreferenceConfiguration.BITRATE_USER_SET_PREF_STRING)
                .remove(PreferenceConfiguration.BITRATE_PREF_OLD_STRING)
                .remove(PreferenceConfiguration.PREFS_VERSION_PREF_STRING)
                .remove(PreferenceConfiguration.CODEC_LOW_LATENCY_CORRUPTION_OFF_PREF_STRING)
                // Stored by the Sharper colored text, Controllers and When Nightjar opens pickers.
                .remove(PreferenceConfiguration.YUV444_PREF_STRING)
                .remove(PreferenceConfiguration.PACKED_444_PREF_STRING)
                .remove(PreferenceConfiguration.SEND_CONTROLLERS_PREF_STRING)
                .remove(PreferenceConfiguration.MULTI_CONTROLLER_PREF_STRING)
                .remove(com.limelight.console.HomeSettings.PREF_KEY)
                // Stored by the Frame pacing, Variable refresh rate and Lower refresh rate pickers beside their row's key.
                .remove(PreferenceConfiguration.HOST_FRAME_TIMES_PREF_STRING)
                .remove(SettingRules.DROP_LATE_FRAMES)
                .remove(PreferenceConfiguration.REAL_VRR_PREF_STRING)
                .remove(PreferenceConfiguration.CONTENT_RATE_FRAME_RATE_PREF_STRING)
                .commit();
        PreferenceManager.setDefaultValues(context, R.xml.preferences, true);
        PreferenceConfiguration.migrateToNativeDefaults(context);
        FoldedChoice.migrate(prefs);
        // The XML default just came back: where Android's low-latency mode can corrupt the picture it starts off again.
        PreferenceConfiguration.migrateCodecLowLatencyCorruption(prefs);
    }

    /** Whether this is TV Settings with its rail, whose group headings are settings_category_tv. */
    static boolean hasRail(Activity activity) {
        return activity != null && activity.findViewById(R.id.settings_rail) != null;
    }

    static void stylePreferences(PreferenceGroup group, Activity activity) {
        stylePreferences(group, hasRail(activity) ? R.layout.settings_category_tv : R.layout.settings_category);
    }

    /** By form, not by a resource qualifier: a TV is past w840dp, which outranks television in resource matching. */
    static int sectionLayout(Context context) {
        return GlassForm.of(context) == GlassForm.TELEVISION ? R.layout.settings_section_tv : R.layout.settings_section;
    }

    /** A row that holds one value says it at its end, whatever opens to change it. */
    static boolean showsValueAtEnd(Preference preference) {
        if (preference instanceof ListPreference || preference instanceof SeekBarPreference) return true;
        String key = preference.getKey();
        return TouchMode.PREF.equals(key) || VideoScaling.PREF.equals(key)
                || OverlayChoice.ENABLED_KEY.equals(key) || AppTheme.THEME.equals(key) || "gpu_driver".equals(key)
                || SwitchPairChoice.forKey(key) != null;
    }

    static void stylePreferences(PreferenceGroup group, int categoryLayout) {
        for (int i = 0; i < group.getPreferenceCount(); i++) {
            Preference preference = group.getPreference(i);
            if (preference instanceof PresetPreference) continue;
            if (preference instanceof PreferenceCategory) {
                // A group without a title continues the section heading above it.
                preference.setLayoutResource(TextUtils.isEmpty(preference.getTitle()) ?
                        R.layout.settings_category_spacer : categoryLayout);
            } else {
                preference.setLayoutResource(showsValueAtEnd(preference) ? R.layout.settings_preference_value : R.layout.settings_preference);
            }
            if (preference instanceof PreferenceGroup) {
                stylePreferences((PreferenceGroup) preference, categoryLayout);
            }
        }
    }

    /**
     * UP from the list goes to the search field only while it is shown; hidden, it would be a dead end on
     * API below 26, so the default search finds the shell bar instead.
     */
    static void linkListToSearch(View list, View search) {
        if (list == null) return;
        list.setNextFocusUpId(search != null && search.getVisibility() == View.VISIBLE ? R.id.settings_search : View.NO_ID);
    }

    private static boolean isWithin(ViewGroup ancestor, View view) {
        for (android.view.ViewParent p = view.getParent(); p != null; p = p.getParent()) {
            if (p == ancestor) return true;
        }
        return false;
    }

    /** Whether the first focus may be taken from this view: nothing holds it, or only the bars do, which the window gave it to. */
    static boolean yieldsFirstFocus(Activity activity, View owner) {
        if (owner == null) return true;
        for (int id : new int[] {R.id.settings_toolbar, R.id.settings_chrome}) {
            View bar = activity.findViewById(id);
            if (bar instanceof ViewGroup && isWithin((ViewGroup) bar, owner)) return true;
        }
        return false;
    }

    static void stylePreferenceList(View view, Activity activity, boolean contentPane) {
        ListView list = GroupedListView.adopt(view);
        com.limelight.ui.UiNavigation.bindPreferencesWhenReady(list);
        View search = activity.findViewById(R.id.settings_search);
        linkListToSearch(list, search);
        // The first focus lands on content, never on the toolbar; with two panes only one list takes it.
        if (((StreamSettings) activity).takesFocus(contentPane) && (search == null || !search.hasFocus())) {
            list.requestFocus();
            // The window may hand focus to the first focusable (the toolbar) once it is laid out; take it back.
            list.post(() -> {
                // A pane that was opened to be entered is not attached when it asks, so it did not get focus.
                if (contentPane && !list.hasFocus() && list.isAttachedToWindow()) {
                    ((StreamSettings) activity).focusContentPane(list);
                    return;
                }
                View owner = activity.findViewById(android.R.id.content).findFocus();
                if (!list.hasFocus() && yieldsFirstFocus(activity, owner) && list.isAttachedToWindow()) list.requestFocus();
            });
        }
        list.setDivider(null);
        list.setDividerHeight(0);
        list.setSelector(R.drawable.settings_row_focus);
        // The selector's inset puts rows on their group's edge; under the last one, what the chrome keeps.
        StreamSettings settings = (StreamSettings) activity;
        GlassForm form = settings.ui.chrome != null ? settings.ui.chrome.form() : GlassForm.of(activity);
        int foot = settings.ui.listFoot > 0 ? settings.ui.contentFoot(contentPane && settings.twoPane, form) : 0;
        list.setPadding(0, 0, 0, foot > 0 ? foot : view.getResources().getDimensionPixelSize(R.dimen.grid_padding_bottom));
        list.setClipToPadding(false);
    }

    // "Automatic (recommended)" reads as "Automatic" in a one-line summary
    static CharSequence entryFor(Context context, int names, int values, String value) {
        String[] valueArray = context.getResources().getStringArray(values);
        for (int i = 0; i < valueArray.length; i++) {
            if (valueArray[i].equals(value)) {
                return context.getResources().getStringArray(names)[i].replaceFirst(" \\(.*\\)$", "");
            }
        }
        return value;
    }
}
