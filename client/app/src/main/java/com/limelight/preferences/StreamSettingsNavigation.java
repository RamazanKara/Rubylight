package com.limelight.preferences;

import android.content.Context;
import android.content.Intent;
import android.content.SharedPreferences;
import android.os.Build;
import android.os.Bundle;
import android.os.Handler;
import android.view.Display;
import android.view.KeyEvent;
import android.view.View;
import android.widget.ListView;

import androidx.appcompat.widget.SearchView;

import com.limelight.PcView;
import com.limelight.R;
import com.limelight.ui.ShellBar;

import java.util.ArrayList;
import java.util.HashMap;
import java.util.HashSet;
import java.util.Map;
import java.util.Set;

/**
 * Two-pane focus coordination, TV rail selection/preview, search query state, back-stack
 * navigation, and fragment transaction orchestration for {@link StreamSettings}.
 */
final class StreamSettingsNavigation {
    private static final String MORE_STATE_PREFS = "SettingsScreenState";
    private static final String MORE_STATE_OPEN = "more_open";

    private final StreamSettings activity;
    PreferenceConfiguration previousPrefs;
    /** The shell root when Settings opened, and whether the bar opened it; see finishSettings. */
    ShellBar.Destination startRoot;
    int previousDisplayPixelCount;
    /** The two-pane screen shows a section before one is picked; this one is only that default, not the user's choice. */
    private boolean sectionDefaulted;
    /** The next right-pane list takes focus: a section was opened by the D-pad, or a result or deep link picked a row. */
    boolean enterPane;
    private boolean firstFocusPlaced;
    private boolean swallowReturnKeyUp;
    private boolean swallowEnterKeyUp;
    /** The row each section's pane had selected when focus last left it, so entering it again returns there. */
    private final Map<String, Integer> paneSelection = new HashMap<>();
    String returnSearchSection;
    /** The query search showed last, which opening search again brings back with its results; empty once cleared. */
    String lastSearchQuery = "";
    private boolean changingQuery;
    boolean stateSaved;
    boolean renderPending;
    private boolean searchPending;
    private final Handler navigationHandler = new Handler();
    private final Runnable searchUpdate = this::renderSearch;
    private final Runnable sectionPreview = this::runSectionPreview;

    StreamSettingsNavigation(StreamSettings activity) {
        this.activity = activity;
    }

    private void runSectionPreview() {
        String key = activity.previewSection;
        activity.previewSection = null;
        if (key != null && activity.searchQuery.isEmpty()) showSection(key);
    }

    /** A section or overview row behind More settings is about to show (a link, a search result): the fold opens for this visit. */
    void unfoldRailFor(String row) {
        if (row != null && StreamSettings.RAIL_FOLDED.contains(row)) activity.moreOpen.add(StreamSettings.RAIL);
    }

    /** More settings was chosen: the rows below it show or go, and the choice is kept for the next visit. */
    void toggleRail() {
        boolean open = !activity.moreOpen.remove(StreamSettings.RAIL);
        if (open) activity.moreOpen.add(StreamSettings.RAIL);
        rememberMore(StreamSettings.RAIL, open);
        // The pane cannot keep showing a section whose row just went: it goes back to the first one.
        if (!open && activity.twoPane && activity.searchQuery.isEmpty()
                && activity.section != null && StreamSettings.RAIL_FOLDED.contains(activity.section)) {
            showSection(defaultSection());
            return;
        }
        applyRailFold();
        markSection();
    }

    void applyRailFold() {
        android.app.Fragment fragment = activity.getFragmentManager()
                .findFragmentById(activity.twoPane ? R.id.settings_sections : R.id.stream_settings);
        if (fragment instanceof StreamSettings.RootFragment) ((StreamSettings.RootFragment) fragment).applyFold();
    }

    /** The section the right pane shows before one is picked: Recommended, the first row of the list; a TV opens on Controls in its place. */
    private String defaultSection() {
        return StreamSettings.isTelevision(activity) ? "input" : "simple";
    }

    void reloadSettings() {
        navigationHandler.removeCallbacks(searchUpdate);
        navigationHandler.removeCallbacks(sectionPreview);
        activity.previewSection = null;
        searchPending = false;
        if (activity.isFinishing() || activity.isDestroyed()) return;
        if (stateSaved) {
            renderPending = true;
            return;
        }
        activity.section = StreamSettings.normalizeSection(activity.section);
        if (activity.twoPane && activity.section == null) {
            activity.section = defaultSection();
            sectionDefaulted = true;
        }
        // A section behind More settings shows with its row: the fold is open while it does, and on a phone when Back
        // returns from it to the list.
        if (activity.searchQuery.isEmpty()) unfoldRailFor(activity.section);
        applyRailFold();
        Display.Mode mode = activity.getWindowManager().getDefaultDisplay().getMode();
        previousDisplayPixelCount = mode.getPhysicalWidth() * mode.getPhysicalHeight();
        android.app.FragmentTransaction transaction = activity.getFragmentManager().beginTransaction();
        if (activity.twoPane && activity.getFragmentManager().findFragmentById(R.id.settings_sections) == null) {
            transaction.add(R.id.settings_sections, new StreamSettings.RootFragment());
        }
        // Search results count as one page, so typing does not bring the pane in again on every key.
        if (activity.rail != null) activity.rail.enterPane(activity.searchQuery.isEmpty() ? String.valueOf(activity.section) : "");
        transaction.replace(R.id.stream_settings, activity.section == null && activity.searchQuery.isEmpty() ?
                new StreamSettings.RootFragment() : StreamSettings.SettingsFragment.forSection(activity.section)).commit();
        activity.ui.updateHeader();
        markSection();
        // Rows More settings just unfolded reach the list with its next pass (PreferenceGroupAdapter queues it): the
        // section's row is marked once it is there.
        ListView sections = listOf(R.id.settings_sections);
        if (activity.twoPane && sections != null) {
            sections.post(() -> { if (!activity.isFinishing() && !activity.isDestroyed()) markRail(); });
        }
    }

    /** The preference list of the fragment in a pane, or null while it has no view. */
    ListView listOf(int container) {
        android.app.Fragment fragment = activity.getFragmentManager().findFragmentById(container);
        View view = fragment == null ? null : fragment.getView();
        return view == null ? null : view.findViewById(android.R.id.list);
    }

    /** The list of sections: the left pane, or the whole screen while a phone shows the overview. */
    ListView sectionList() {
        return listOf(activity.twoPane ? R.id.settings_sections : R.id.stream_settings);
    }

    /** The view holding focus in this screen's tree, which is set before the window reports focus. */
    private View focusOwner() {
        return activity.findViewById(android.R.id.content).findFocus();
    }

    private boolean inContentPane(View view) {
        for (android.view.ViewParent parent = view == null ? null : view.getParent(); parent != null; parent = parent.getParent()) {
            if (parent instanceof View && ((View) parent).getId() == R.id.stream_settings) return true;
        }
        return false;
    }

    /** Marks the section the right pane shows in the list of sections. */
    void markSection() {
        placeFirstFocus();
        markRail();
    }

    /** Marks the shown section's row in the list beside it, and moves TV's bar to it. */
    private void markRail() {
        String row = markedRow();
        android.app.Fragment fragment = activity.getFragmentManager().findFragmentById(R.id.settings_sections);
        if (activity.twoPane && fragment instanceof StreamSettings.RootFragment) {
            ((StreamSettings.RootFragment) fragment).mark(row);
        }
        if (activity.rail != null) {
            ListView list = listOf(R.id.settings_sections);
            activity.rail.setList(list);
            activity.rail.mark(list == null ? -1 : StreamSettings.RootFragment.positionOf(list, row));
        }
    }

    /** With the D-pad, the list of sections starts on the section shown, not on its first row: one row is highlighted from the start. */
    void placeFirstFocus() {
        ListView list = activity.twoPane && !firstFocusPlaced ? listOf(R.id.settings_sections) : null;
        // Not before the list has focus: a selected row moves a list that overflows, and nothing needs it yet.
        if (list == null || list.getAdapter() == null || !list.hasFocus()) return;
        int position = StreamSettings.RootFragment.positionOf(list, markedRow());
        // In touch mode nothing is selected: the first D-pad press, which leaves it, places it (onCreate's listener).
        // The section shown is no longer the rail's first row, so the list's own pick on leaving would be another.
        if (position < 0 || list.isInTouchMode()) return;
        firstFocusPlaced = true;
        if (list.getChildCount() > 0) selectInPlace(list, position);
        else androidx.core.view.OneShotPreDrawListener.add(list, () -> selectInPlace(list, position));
    }

    /**
     * Selects a row without moving it: setSelection puts it at the list's top, so a rail that fits its screen opened scrolled,
     * Presets cut in half, with Video below it selected. A row off screen still comes into view.
     */
    static void selectInPlace(ListView list, int position) {
        View row = list.getChildAt(position - list.getFirstVisiblePosition());
        if (row == null || row.getTop() < list.getListPaddingTop() || row.getBottom() > list.getHeight() - list.getListPaddingBottom()) {
            list.setSelection(position);
        } else {
            list.setSelectionFromTop(position, row.getTop() - list.getListPaddingTop());
        }
    }

    /**
     * The section the right pane shows: search results are no section, so no row stands for them. A row with no page of its
     * own (Presets, Profiles) can hold the ring beside this mark: the ring says where focus is, the pill what is on the right.
     */
    String markedRow() {
        return activity.searchQuery.isEmpty() ? activity.section : null;
    }

    /** Moves focus from the content pane back to the section it shows. */
    private void focusSectionList() {
        ListView list = sectionList();
        if (list == null) return;
        ListView content = activity.twoPane ? listOf(R.id.stream_settings) : null;
        // Search results are not the section's rows: their position is not the one to come back to.
        if (content != null && activity.section != null && activity.section.equals(markedRow()) && content.hasFocus()) {
            paneSelection.put(activity.section, content.getSelectedItemPosition());
        }
        markSection();
        int position = StreamSettings.RootFragment.positionOf(list, markedRow());
        if (position >= 0) list.setSelection(position);
        list.requestFocus();
    }

    /** Moves focus into the right pane, onto the row it was left on or else its first focusable row. */
    void focusContentPane(ListView list) {
        list.requestFocus();
        android.widget.ListAdapter adapter = list.getAdapter();
        if (adapter == null) return;
        Integer remembered = activity.section == null ? null : paneSelection.get(activity.section);
        int position = PaneFocus.target(remembered == null ? -1 : remembered, adapter.getCount(), adapter::isEnabled);
        // setSelection puts the row at the list's top, and so does the list as it takes focus. For the first row that scrolled
        // its group's gap out of sight, and the list, now scrolled, laid its top fade over the row and took the top edge of its
        // ring (nightjar#7, Shield TV). The row stands the fade's length down instead, as the D-pad leaves a row, so the first
        // row stays under its gap with the list at its top. This comes after requestFocus, which places the row too.
        if (position >= 0) list.setSelectionFromTop(position, list.getVerticalFadingEdgeLength());
    }

    /** Called when a section row is selected: the right pane follows after a short pause, so a fast scroll does not rebuild it per row. */
    void previewSection(String key) {
        if (!activity.twoPane || !activity.searchQuery.isEmpty()) return;
        navigationHandler.removeCallbacks(sectionPreview);
        // Back on the section already shown: nothing is pending any more.
        activity.previewSection = key.equals(activity.section) ? null : key;
        if (activity.previewSection != null) navigationHandler.postDelayed(sectionPreview, 100);
    }

    /**
     * The D-pad moved to a row of the list of sections. A row without a page drops a preview still on its way, or the pane
     * would change under it. While a preview is on its way the mark waits for it, and moves with the pane.
     */
    void sectionRowSelected(String key) {
        if (StreamSettings.SECTIONS.containsKey(key)) {
            previewSection(key);
        } else {
            navigationHandler.removeCallbacks(sectionPreview);
            activity.previewSection = null;
        }
        if (activity.previewSection == null) markSection();
    }

    /** Opens a section from its row; {@code enter} moves focus into it, which is what Center does after the D-pad. */
    void activateSection(String key, boolean enter) {
        navigationHandler.removeCallbacks(sectionPreview);
        activity.previewSection = null;
        if (activity.twoPane && enter) {
            enterPane = true;
            if (key.equals(activity.section) && activity.searchQuery.isEmpty()) {
                ListView list = listOf(R.id.stream_settings);
                if (list != null) {
                    enterPane = false;
                    focusContentPane(list);
                }
                return;
            }
        }
        showSection(key);
    }

    /** Called by a content list when it is built: whether it takes focus (see {@link #enterPane}). */
    boolean takesFocus(boolean contentPane) {
        if (!activity.twoPane) return StreamSettings.isTelevision(activity);
        if (!contentPane) return StreamSettings.isTelevision(activity) && !enterPane;
        boolean enter = enterPane;
        enterPane = false;
        return enter;
    }

    void renderSearch() {
        searchPending = false;
        if (activity.isFinishing() || activity.isDestroyed()) return;
        if (stateSaved) {
            renderPending = true;
            return;
        }
        android.app.Fragment fragment = activity.getFragmentManager().findFragmentById(R.id.stream_settings);
        if (fragment instanceof StreamSettings.SettingsFragment && !activity.searchQuery.isEmpty()) {
            ((StreamSettings.SettingsFragment) fragment).filterPreferences();
            activity.ui.updateHeader();
            // The pane turned from a section into results in place: on a TV the search row takes the mark.
            markSection();
        } else {
            reloadSettings();
        }
    }

    void setSearchQuery(String query) {
        if (!query.isEmpty()) lastSearchQuery = query;
        changingQuery = true;
        activity.searchQuery = query;
        activity.searchView.setQuery(query, false);
        changingQuery = false;
        activity.searchView.clearFocus();
        activity.searchView.setVisibility(query.isEmpty() ? View.GONE : View.VISIBLE);
        reloadSettings();
    }

    /**
     * Shows the search field under the top app bar and focuses it; search stays out of the way until asked for. A search
     * left for a section or by Back comes back as it was, its query and its results.
     */
    void openSearch() {
        if (activity.searchQuery.isEmpty() && !lastSearchQuery.isEmpty()) {
            activity.returnSearchQuery = null;
            returnSearchSection = null;
            setSearchQuery(lastSearchQuery);
        }
        activity.searchView.setVisibility(View.VISIBLE);
        activity.ui.updateNavigation();
        activity.ui.linkListsToSearch();
        activity.searchView.setIconified(false);
        activity.searchView.requestFocus();
    }

    void openSearchResult(String targetSection, String key) {
        activity.returnSearchQuery = activity.searchQuery;
        returnSearchSection = savedSection();
        // The right pane always shows a section; a result that lives in the list of sections (the presets) keeps the one it had.
        if (targetSection != null || !activity.twoPane) {
            activity.section = targetSection;
            sectionDefaulted = false;
        } else if (activity.section == null) {
            activity.section = defaultSection();
            sectionDefaulted = true;
        }
        activity.searchResultKey = key;
        if (StreamSettings.LOW_LATENCY_CUSTOMIZED.contains(key)) activity.customizeOpen = true;
        if (SettingRules.WHEN_ROWS.contains(key)) activity.revealed.add(key);
        unfoldFor(targetSection, key);
        // A result that is a row of the overview itself (the presets) shows there.
        if (targetSection == null) unfoldRailFor(key);
        enterPane = activity.focusRow = activity.twoPane && inContentPane(focusOwner());
        setSearchQuery("");
    }

    void bindSearchView(SearchView searchView) {
        searchView.setVisibility(activity.searchQuery.isEmpty() ? View.GONE : View.VISIBLE);
        searchView.setIconifiedByDefault(false);
        searchView.setQueryHint(activity.getString(R.string.settings_search_hint));
        searchView.setQuery(activity.searchQuery, false);
        searchView.clearFocus();
        searchView.setOnQueryTextListener(new SearchView.OnQueryTextListener() {
            @Override public boolean onQueryTextSubmit(String query) {
                navigationHandler.removeCallbacks(searchUpdate);
                renderSearch();
                searchView.clearFocus();
                return true;
            }

            @Override public boolean onQueryTextChange(String query) {
                if (changingQuery) return true;
                activity.searchQuery = query.trim();
                lastSearchQuery = activity.searchQuery;
                activity.returnSearchQuery = null;
                returnSearchSection = null;
                navigationHandler.removeCallbacks(searchUpdate);
                searchPending = true;
                navigationHandler.postDelayed(searchUpdate, 120);
                return true;
            }
        });
        // Once the focus has moved: while it moves, the field still counts as holding it.
        searchView.setOnQueryTextFocusChangeListener((view, focused) -> {
            if (!focused) searchView.post(activity.ui::closeEmptySearch);
        });
    }

    /** The sections whose More row the user left open, kept between visits apart from the settings themselves. */
    Set<String> rememberedMore() {
        return activity.getSharedPreferences(MORE_STATE_PREFS, Context.MODE_PRIVATE)
                .getStringSet(MORE_STATE_OPEN, new HashSet<>());
    }

    /** Only the user's own choice is kept: a search result or link unfolding a section lasts for this visit. */
    void rememberMore(String section, boolean open) {
        rememberMore(activity, section, open);
    }

    static void rememberMore(Context context, String section, boolean open) {
        SharedPreferences state = context.getSharedPreferences(MORE_STATE_PREFS, Context.MODE_PRIVATE);
        Set<String> remembered = new HashSet<>(state.getStringSet(MORE_STATE_OPEN, new HashSet<>()));
        if (open) remembered.add(section);
        else remembered.remove(section);
        state.edit().putStringSet(MORE_STATE_OPEN, remembered).apply();
    }

    /** A search result or a link that lands on a folded row opens its section's More row first. */
    void unfoldFor(String keySection, String key) {
        if (keySection != null && StreamSettings.foldsInSection(activity, keySection, key)) {
            activity.moreOpen.add(keySection);
        }
    }

    /** The section the user picked: not the one a wide screen shows before any is, which a narrow screen would open inside. */
    String savedSection() {
        return sectionDefaulted ? null : activity.section;
    }

    void showSection(String newSection) {
        activity.section = newSection;
        sectionDefaulted = false;
        if (activity.twoPane && !activity.searchQuery.isEmpty()) {
            // Picking a section ends the search; setSearchQuery shows the section.
            activity.returnSearchQuery = null;
            returnSearchSection = null;
            setSearchQuery("");
            return;
        }
        reloadSettings();
    }

    void navigateBack() {
        if (activity.searchQuery.isEmpty() && activity.searchView.getVisibility() == View.VISIBLE) {
            activity.searchView.clearFocus();
            activity.searchView.setVisibility(View.GONE);
            activity.ui.updateNavigation();
            activity.ui.linkListsToSearch();
            // Back to what opened it. By touch this asks for nothing: the button takes no focus there.
            activity.findViewById(R.id.settings_search_button).requestFocus();
        } else if (!activity.searchQuery.isEmpty()) {
            setSearchQuery("");
        } else if (activity.returnSearchQuery != null) {
            String query = activity.returnSearchQuery;
            activity.section = returnSearchSection;
            sectionDefaulted = false;
            activity.returnSearchQuery = null;
            returnSearchSection = null;
            setSearchQuery(query);
        } else if (activity.twoPane && inContentPane(focusOwner())) {
            focusSectionList();
        } else if (activity.section != null && !activity.twoPane) {
            showSection(null);
        } else {
            finishSettings();
        }
    }

    /** A recreated activity gets back the fragments of the layout it had; the other layout has no place for them. */
    void dropMisplacedPanes() {
        android.app.FragmentManager fragments = activity.getFragmentManager();
        android.app.Fragment sections = fragments.findFragmentById(R.id.settings_sections);
        android.app.Fragment content = fragments.findFragmentById(R.id.stream_settings);
        android.app.FragmentTransaction transaction = fragments.beginTransaction();
        boolean changed = false;
        if (!activity.twoPane && sections != null) {
            transaction.remove(sections);
            changed = true;
        }
        // A wide screen's section that nobody picked would open inside it on a narrow one; the list shows instead.
        boolean unpicked = !activity.twoPane && activity.section == null && activity.searchQuery.isEmpty()
                && content instanceof StreamSettings.SettingsFragment;
        if (unpicked || (activity.twoPane && content instanceof StreamSettings.RootFragment)) {
            transaction.remove(content);
            changed = true;
        }
        if (changed) {
            transaction.commitAllowingStateLoss();
            fragments.executePendingTransactions();
        }
    }

    void flushPendingSearchBeforeSave() {
        // Save the screen for the latest query, including a last keystroke before rotation.
        navigationHandler.removeCallbacks(searchUpdate);
        if (searchPending) renderSearch();
        activity.getFragmentManager().executePendingTransactions();
        stateSaved = true;
    }

    void saveInstanceState(Bundle outState, String stateSection, String stateCustomize, String stateMoreOpen) {
        outState.putString(stateSection, savedSection());
        outState.putString("search", activity.searchQuery);
        outState.putString("return_search", activity.returnSearchQuery);
        outState.putString("return_section", returnSearchSection);
        outState.putString("last_search", lastSearchQuery);
        outState.putString("search_result", activity.searchResultKey);
        outState.putBoolean(stateCustomize, activity.customizeOpen);
        outState.putStringArrayList(stateMoreOpen, new ArrayList<>(activity.moreOpen));
        outState.putStringArrayList("revealed", new ArrayList<>(activity.revealed));
    }

    void onDestroy() {
        navigationHandler.removeCallbacksAndMessages(null);
        if (activity.rail != null) activity.rail.detach();
    }

    boolean handlePaneKeyBeforeSuper(KeyEvent event) {
        // Right (Left in RTL) from the list of sections enters the pane on a known row: the focus search would
        // pick whichever row is nearest, and the key's release must not reach the row that gets focus.
        int towardPane = activity.getResources().getConfiguration().getLayoutDirection() == View.LAYOUT_DIRECTION_RTL ?
                KeyEvent.KEYCODE_DPAD_LEFT : KeyEvent.KEYCODE_DPAD_RIGHT;
        if (activity.twoPane && event.getKeyCode() == towardPane && event.hasNoModifiers()) {
            if (event.getAction() == KeyEvent.ACTION_DOWN) {
                View owner = focusOwner();
                ListView sections = sectionList();
                ListView content = listOf(R.id.stream_settings);
                if (owner != null && sections != null && content != null && owner == sections && activity.searchQuery.isEmpty()) {
                    swallowEnterKeyUp = true;
                    if (event.getRepeatCount() == 0) {
                        // The pane follows the selected row after a pause: a press inside it opens the row's section.
                        if (activity.previewSection != null && !activity.previewSection.equals(activity.section)) activateSection(activity.previewSection, true);
                        else focusContentPane(content);
                    }
                    return true;
                }
            } else if (event.getAction() == KeyEvent.ACTION_UP && swallowEnterKeyUp) {
                swallowEnterKeyUp = false;
                return true;
            }
        }
        return false;
    }

    boolean handlePaneKeyAfterSuper(KeyEvent event) {
        // Left (Right in RTL) from the content pane returns to the list; rows that use the key, such as a slider, ran first.
        int towardList = activity.getResources().getConfiguration().getLayoutDirection() == View.LAYOUT_DIRECTION_RTL ?
                KeyEvent.KEYCODE_DPAD_RIGHT : KeyEvent.KEYCODE_DPAD_LEFT;
        if (activity.twoPane && event.getKeyCode() == towardList) {
            if (event.getAction() == KeyEvent.ACTION_DOWN && inContentPane(focusOwner())) {
                swallowReturnKeyUp = true;
                if (event.getRepeatCount() == 0) focusSectionList();
                return true;
            }
            if (event.getAction() == KeyEvent.ACTION_UP && swallowReturnKeyUp) {
                swallowReturnKeyUp = false;
                return true;
            }
        }
        return false;
    }

    private void finishSettings() {
        // Language changes are handled via configuration changes in Android 13+,
        // so manual activity relaunching is no longer required.
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU) {
            PreferenceConfiguration newPrefs = PreferenceConfiguration.readPreferences(activity);
            if (!newPrefs.language.equals(previousPrefs.language)) {
                activity.finish();
                // Restart the PC view to apply UI changes
                Intent intent = new Intent(activity, PcView.class);
                intent.setFlags(Intent.FLAG_ACTIVITY_CLEAR_TASK | Intent.FLAG_ACTIVITY_NEW_TASK);
                activity.startActivity(intent, null);
                return;
            }
        }
        // Turning Home on or off here moves the root; restart the task on the new one so Back does not
        // land on a screen that is no longer the first.
        ShellBar.Destination root = ShellBar.root(activity);
        if (root != startRoot) {
            activity.finish();
            activity.startActivity(ShellBar.intentFor(activity, root)
                    .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK | Intent.FLAG_ACTIVITY_CLEAR_TASK));
            return;
        }
        if (activity.fromShell) {
            // Back from Settings opened by the bar goes to the root: Home, or the list of PCs when the app opens there.
            ShellBar.leave(activity, ShellBar.Destination.SETTINGS);
        } else {
            activity.finish();
        }
    }
}
