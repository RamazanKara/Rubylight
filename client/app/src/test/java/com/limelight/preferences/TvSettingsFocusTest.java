package com.limelight.preferences;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertNotSame;
import static org.junit.Assert.assertSame;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

import android.app.Application;
import android.graphics.drawable.Drawable;
import android.preference.Preference;
import android.preference.PreferenceManager;
import android.view.KeyEvent;
import android.view.View;
import android.view.ViewGroup;
import android.widget.ListView;
import android.widget.TextView;

import androidx.appcompat.app.AlertDialog;

import com.limelight.R;
import com.limelight.ui.TvScreensPreviewTest;

import org.junit.After;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.GraphicsMode;
import org.robolectric.shadows.ShadowDialog;
import org.robolectric.shadows.ShadowLooper;

import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;
import java.util.concurrent.TimeUnit;

/**
 * TV Settings' rail under the D-pad (nightjar#7, reported by @VirtualGIT20): each Up or Down press moves exactly one
 * row, Presets included, and the ring and the shown section agree on the section rows.
 */
@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 34, application = Application.class, qualifiers = "w960dp-h540dp-land-television-xhdpi",
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
// Real text metrics: the rows' heights decide whether the list moves its selection or scrolls inside a row.
@GraphicsMode(GraphicsMode.Mode.NATIVE)
public class TvSettingsFocusTest {
    private ActivityController<StreamSettings> controller;

    @After
    public void tearDown() {
        if (controller != null) controller.pause().stop().destroy();
        PreferenceManager.getDefaultSharedPreferences(RuntimeEnvironment.getApplication()).edit().clear().commit();
    }

    private StreamSettings open() {
        controller = Robolectric.buildActivity(StreamSettings.class).setup().visible();
        StreamSettings screen = controller.get();
        // A TV has no touch mode; Robolectric starts in it, where the D-pad would not move a list's selection.
        TvScreensPreviewTest.leaveTouchMode(screen);
        settle(screen);
        return screen;
    }

    private static void settle(StreamSettings screen) {
        ShadowLooper.idleMainLooper(300, TimeUnit.MILLISECONDS);
        screen.getFragmentManager().executePendingTransactions();
        View root = screen.getWindow().getDecorView();
        int width = screen.getResources().getDisplayMetrics().widthPixels;
        int height = screen.getResources().getDisplayMetrics().heightPixels;
        root.measure(View.MeasureSpec.makeMeasureSpec(width, View.MeasureSpec.EXACTLY),
                View.MeasureSpec.makeMeasureSpec(height, View.MeasureSpec.EXACTLY));
        root.layout(0, 0, width, height);
        // A device draws after every layout, and its pre-draw pass may still move a list (SettingsRail.snapTopRow);
        // this manual layout has no frame after it, so run that pass here until a frame would draw. A device measures
        // before it lays out again, which reads a padding the pass changed.
        for (int i = 0; i < 5 && root.getViewTreeObserver().dispatchOnPreDraw(); i++) {
            root.measure(View.MeasureSpec.makeMeasureSpec(width, View.MeasureSpec.EXACTLY),
                    View.MeasureSpec.makeMeasureSpec(height, View.MeasureSpec.EXACTLY));
            root.layout(0, 0, width, height);
        }
        ShadowLooper.idleMainLooper(300, TimeUnit.MILLISECONDS);
    }

    private static ListView sections(StreamSettings screen) {
        return (ListView) screen.getFragmentManager().findFragmentById(R.id.settings_sections).getView()
                .findViewById(android.R.id.list);
    }

    private static String keyAt(ListView list, int position) {
        Object item = position < 0 ? null : list.getItemAtPosition(position);
        return item instanceof Preference ? ((Preference) item).getKey() : null;
    }

    private static void press(StreamSettings screen, int keyCode) {
        TvScreensPreviewTest.key(screen, keyCode);
        settle(screen);
    }

    @Test
    public void everyDownPressMovesTheRailOneRowPresetsIncluded() {
        StreamSettings screen = open();
        ListView list = sections(screen);
        assertTrue("the rail has the first focus", list.hasFocus());
        // The rail opens on Controls in its place among the sections, as on every screen; Up climbs a row at a time to its
        // first row, which stands whole; then Down goes through the rows in that one order.
        int start = list.getSelectedItemPosition();
        assertEquals("input", keyAt(list, start));
        for (int i = start - 1; i >= 0; i--) {
            press(screen, KeyEvent.KEYCODE_DPAD_UP);
            assertEquals("Up moves one row", i, list.getSelectedItemPosition());
        }
        assertEquals(0, list.getFirstVisiblePosition());
        assertEquals(list.getListPaddingTop(), list.getChildAt(0).getTop());
        List<String> visited = new ArrayList<>();
        visited.add(keyAt(list, list.getSelectedItemPosition()));
        for (int i = 1; i < list.getCount(); i++) {
            int before = list.getSelectedItemPosition();
            press(screen, KeyEvent.KEYCODE_DPAD_DOWN);
            assertEquals("Down from " + keyAt(list, before) + " moves one row", before + 1, list.getSelectedItemPosition());
            assertTrue(list.hasFocus());
            visited.add(keyAt(list, list.getSelectedItemPosition()));
        }
        assertEquals(Arrays.asList("stream_presets", "stream_profiles", "video", "display", "input",
                "audio", "streaming", "app", "advanced", "debug"), visited);
        for (int i = list.getCount() - 2; i >= 0; i--) {
            press(screen, KeyEvent.KEYCODE_DPAD_UP);
            assertEquals("Up moves one row", i, list.getSelectedItemPosition());
        }
    }

    /** TV run on a TCL C6K: after a few presses a row sat half under the subtitle. No rail row is ever cut at the top. */
    @Test
    public void noRailRowIsLeftHalfScrolledUnderTheSubtitle() {
        assertNoRailRowCut();
    }

    /**
     * TV run on a TCL C6K in German: on Erweitert, the last row, its summary's second line pushed Audio half under the
     * subtitle, and the list could not scroll past its end to take Audio away.
     */
    @Test
    @Config(qualifiers = "de-w960dp-h540dp-land-television-xhdpi")
    public void theLastRailRowLeavesNoRowCutAtTheTopInGerman() {
        assertNoRailRowCut();
    }

    private void assertNoRailRowCut() {
        StreamSettings screen = open();
        ListView list = sections(screen);
        for (int i = 0; i < list.getCount() && list.getSelectedItemPosition() != 0; i++) press(screen, KeyEvent.KEYCODE_DPAD_UP);
        int padding = list.getPaddingBottom();
        List<String> cut = new ArrayList<>();
        int[] keys = new int[2 * list.getCount()];
        for (int i = 0; i < keys.length; i++) keys[i] = i < list.getCount() ? KeyEvent.KEYCODE_DPAD_DOWN : KeyEvent.KEYCODE_DPAD_UP;
        for (int key : keys) {
            press(screen, key);
            View first = null;
            // A row ending in the selector's padding shows only its pill's transparent inset: it is gone, not cut.
            for (int c = 0; c < list.getChildCount() && first == null; c++) {
                if (list.getChildAt(c).getBottom() > list.getListPaddingTop()) first = list.getChildAt(c);
            }
            View selected = list.getSelectedView();
            String at = keyAt(list, list.getSelectedItemPosition());
            if (first.getTop() < list.getPaddingTop()) cut.add(at + ": " + keyAt(list, list.getPositionForView(first)) + " at " + first.getTop());
            assertTrue("the selected row " + at + " is whole", selected.getTop() >= 0 && selected.getBottom() <= list.getHeight());
        }
        assertTrue("rows cut at the top: " + cut, cut.isEmpty());
        assertEquals("the room made past the last row is given back", padding, list.getPaddingBottom());
    }

    /**
     * TV run on a TCL C6K: with search results on the right the rail still marked Controls. Results are no section, so no
     * row is marked, and Left from the results comes back to the rail.
     */
    @Test
    public void searchResultsMarkNoRailRowAndLeftReturnsToTheRail() {
        StreamSettings screen = open();
        ListView list = sections(screen);
        assertEquals("input", keyAt(list, list.getCheckedItemPosition()));
        openSearchFromTheCapsule(screen);
        androidx.appcompat.widget.SearchView field = screen.findViewById(R.id.settings_search);
        assertEquals(View.VISIBLE, field.getVisibility());
        field.setQuery("controller", true);
        settle(screen);
        list = sections(screen);
        assertEquals("no row stands for the results", ListView.INVALID_POSITION, list.getCheckedItemPosition());
        SettingsRail rail = org.robolectric.util.ReflectionHelpers.getField(screen, "rail");
        assertEquals("no pill", -1, rail.position());
        ListView results = (ListView) screen.getFragmentManager().findFragmentById(R.id.stream_settings).getView()
                .findViewById(android.R.id.list);
        results.requestFocus();
        settle(screen);
        press(screen, KeyEvent.KEYCODE_DPAD_LEFT);
        assertTrue("Left lands on the rail", list.hasFocus());
    }

    /** Search is reached by keys from the top row: Up from the rail to the tabs, then Right along them to the search. */
    private static void openSearchFromTheCapsule(StreamSettings screen) {
        View search = screen.findViewById(R.id.settings_search_button);
        StringBuilder walk = new StringBuilder();
        for (int i = 0; i < 12 && !search.hasFocus(); i++) {
            ViewGroup tabs = screen.findViewById(R.id.shell_bar);
            press(screen, tabs.hasFocus() ? KeyEvent.KEYCODE_DPAD_RIGHT : KeyEvent.KEYCODE_DPAD_UP);
            View focus = screen.getWindow().getDecorView().findFocus();
            walk.append(focus == null ? "none" : focus.getId() == View.NO_ID ? focus.getClass().getSimpleName()
                    : screen.getResources().getResourceEntryName(focus.getId())).append(' ');
        }
        assertTrue("the keys reach the capsule's search: " + walk, search.hasFocus());
        press(screen, KeyEvent.KEYCODE_DPAD_CENTER);
    }

    @Test
    public void onASectionTheRingAndTheShownSectionAreTheSameRow() {
        StreamSettings screen = open();
        ListView list = sections(screen);
        // Display: past Presets, and not the section a TV opens on (Controls, the row below it).
        goToRow(screen, "display");
        String selected = keyAt(list, list.getSelectedItemPosition());
        assertEquals("display", selected);
        // The pane follows the selected row after its pause, and the mark follows the pane.
        ShadowLooper.idleMainLooper(1, TimeUnit.SECONDS);
        settle(screen);
        assertEquals(selected, keyAt(list, list.getCheckedItemPosition()));
    }

    /**
     * Presets and Profiles have no page to show, so the pane keeps its section while one has focus, and so does the pill:
     * the ring says where focus is, the pill what the pane shows. The pill leaving made the pane nameless.
     */
    @Test
    public void thePillStaysOnTheSectionShownWhileARowWithoutAPageHasFocus() {
        StreamSettings screen = open();
        ListView list = sections(screen);
        View bar = screen.findViewById(R.id.settings_rail_bar);
        int opened = StreamSettings.RootFragment.positionOf(list, "input");
        assertEquals("the section it opens on is marked", opened, list.getCheckedItemPosition());
        // Video, the section beside Profiles: the way up to Presets passes no other section, whose preview would load.
        goToRow(screen, "video");
        ShadowLooper.idleMainLooper(1, TimeUnit.SECONDS);
        settle(screen);
        int shown = StreamSettings.RootFragment.positionOf(list, "video");
        android.app.Fragment pane = screen.getFragmentManager().findFragmentById(R.id.stream_settings);
        assertEquals("the section shown is marked", shown, list.getCheckedItemPosition());
        assertEquals(View.VISIBLE, bar.getVisibility());

        for (String key : new String[] {"stream_presets", "stream_profiles"}) {
            goToRow(screen, key);
            assertEquals(key + ": the section shown stays marked", shown, list.getCheckedItemPosition());
            assertEquals(key + ": the pill stays", View.VISIBLE, bar.getVisibility());
            assertSame(key + ": the pane stays", pane, screen.getFragmentManager().findFragmentById(R.id.stream_settings));
        }

        // Focus on the pane's section again.
        goToRow(screen, "video");
        assertEquals(shown, list.getCheckedItemPosition());
        assertEquals(View.VISIBLE, bar.getVisibility());
        assertSame(pane, screen.getFragmentManager().findFragmentById(R.id.stream_settings));

        // Right from a row without a page: focus is in the pane, whose section is still marked.
        goToRow(screen, "stream_profiles");
        press(screen, KeyEvent.KEYCODE_DPAD_RIGHT);
        assertFalse(list.hasFocus());
        assertEquals(shown, list.getCheckedItemPosition());
        assertEquals(View.VISIBLE, bar.getVisibility());
    }

    /** Passing a section on the way to Presets must not load it under the Presets row: its preview is dropped. */
    @Test
    public void reachingARowWithoutAPageDropsThePreviewOnItsWay() {
        StreamSettings screen = open();
        android.app.Fragment pane = screen.getFragmentManager().findFragmentById(R.id.stream_settings);
        screen.sectionRowSelected("video");
        screen.sectionRowSelected("stream_profiles");
        ShadowLooper.idleMainLooper(1, TimeUnit.SECONDS);
        settle(screen);
        assertSame(pane, screen.getFragmentManager().findFragmentById(R.id.stream_settings));
        // Without the row in between, the same preview does load: the pause above is long enough to tell.
        screen.sectionRowSelected("video");
        ShadowLooper.idleMainLooper(1, TimeUnit.SECONDS);
        settle(screen);
        assertNotSame(pane, screen.getFragmentManager().findFragmentById(R.id.stream_settings));
    }

    /** The window gave first focus to the tab row's scroller, which shows no ring: a real TV opened with no row marked. */
    @Test
    public void theFirstFocusIsTakenBackFromTheTabsButNotFromARow() {
        StreamSettings screen = open();
        assertTrue(StreamSettings.yieldsFirstFocus(screen, screen.findViewById(R.id.shell_bar)));
        assertTrue(StreamSettings.yieldsFirstFocus(screen, screen.findViewById(R.id.shell_home)));
        assertTrue(StreamSettings.yieldsFirstFocus(screen, null));
        assertFalse(StreamSettings.yieldsFirstFocus(screen, sections(screen)));
    }

    @Test
    public void thePresetsRowIsAsCompactAsASectionAndSelectOpensTheList() {
        StreamSettings screen = open();
        ListView list = sections(screen);
        int presets = StreamSettings.RootFragment.positionOf(list, "stream_presets");
        int video = StreamSettings.RootFragment.positionOf(list, "video");
        View presetRow = list.getChildAt(presets - list.getFirstVisiblePosition());
        View videoRow = list.getChildAt(video - list.getFirstVisiblePosition());
        assertNotNull(presetRow);
        assertNotNull(videoRow);
        assertNull("no chips on the rail", presetRow.findViewById(R.id.preset_chips));
        // Its second line names the preset in use, and the row stays a row: taller than a section by at most that line.
        TextView value = presetRow.findViewById(R.id.preset_value);
        assertEquals(View.VISIBLE, value.getVisibility());
        assertFalse("the preset in use is named", value.getText().toString().isEmpty());
        assertTrue("Presets " + presetRow.getHeight() + " px against Video " + videoRow.getHeight() + " px",
                presetRow.getHeight() <= videoRow.getHeight() + value.getHeight());
        while (list.getSelectedItemPosition() != presets) {
            press(screen, list.getSelectedItemPosition() < presets ? KeyEvent.KEYCODE_DPAD_DOWN : KeyEvent.KEYCODE_DPAD_UP);
        }
        press(screen, KeyEvent.KEYCODE_DPAD_CENTER);
        AlertDialog chooser = (AlertDialog) ShadowDialog.getLatestDialog();
        assertNotNull("Select on Presets opens the preset list", chooser);
        assertTrue(chooser.isShowing());
        assertEquals(screen.getResources().getStringArray(R.array.stream_preset_names).length,
                chooser.getListView().getCount());
        chooser.dismiss();
    }

    private static void goToRow(StreamSettings screen, String key) {
        ListView list = sections(screen);
        int target = StreamSettings.RootFragment.positionOf(list, key);
        for (int i = 0; i < 2 * list.getCount() && list.getSelectedItemPosition() != target; i++) {
            press(screen, list.getSelectedItemPosition() < target ? KeyEvent.KEYCODE_DPAD_DOWN : KeyEvent.KEYCODE_DPAD_UP);
        }
        assertEquals(key, keyAt(list, list.getSelectedItemPosition()));
    }

    /**
     * nightjar#7 (Shield TV): with nothing typed the field stayed open under the bar when the D-pad went back to the rail,
     * so the screen showed search twice. An empty field closes as focus leaves it.
     */
    @Test
    public void anEmptySearchFieldClosesWhenFocusLeavesIt() {
        StreamSettings screen = open();
        openSearchFromTheCapsule(screen);
        androidx.appcompat.widget.SearchView field = screen.findViewById(R.id.settings_search);
        assertEquals("Select on the capsule's search opens the field", View.VISIBLE, field.getVisibility());
        assertTrue(field.hasFocus());
        press(screen, KeyEvent.KEYCODE_DPAD_DOWN);
        assertFalse("focus left the field", field.hasFocus());
        assertEquals("the empty field closed", View.GONE, field.getVisibility());
        assertTrue("focus is on the rail", sections(screen).hasFocus());
    }

    /**
     * nightjar#7: picking a section ends the search and closes its field, and the capsule's search brings it back as
     * it was left, its query and its results. A field with a query stays open while focus is elsewhere: that is the
     * search still on screen.
     */
    @Test
    public void theCapsuleSearchBringsBackTheQueryAndResultsASectionLeft() {
        StreamSettings screen = open();
        openSearchFromTheCapsule(screen);
        androidx.appcompat.widget.SearchView field = screen.findViewById(R.id.settings_search);
        field.setQuery("controller", false);
        settle(screen);
        sections(screen).requestFocus();
        settle(screen);
        assertEquals("a field with a query stays while the search shows", View.VISIBLE, field.getVisibility());
        goToRow(screen, "video");
        press(screen, KeyEvent.KEYCODE_DPAD_CENTER);
        assertEquals("a section ends the search and closes the field", View.GONE, field.getVisibility());
        ListView list = sections(screen);
        assertEquals("video", keyAt(list, list.getCheckedItemPosition()));
        press(screen, KeyEvent.KEYCODE_DPAD_LEFT);
        openSearchFromTheCapsule(screen);
        assertEquals(View.VISIBLE, field.getVisibility());
        assertEquals("the query is back", "controller", field.getQuery().toString());
        assertEquals("and its results", "controller", org.robolectric.util.ReflectionHelpers.getField(screen, "searchQuery"));
        list = sections(screen);
        assertEquals("no row stands for the results", ListView.INVALID_POSITION, list.getCheckedItemPosition());
        TextView context = screen.findViewById(R.id.settings_context);
        String none = screen.getResources().getQuantityString(R.plurals.settings_search_count, 0, 0);
        assertFalse("results are listed: " + context.getText(), none.contentEquals(context.getText()));
    }

    private static android.graphics.Bitmap render(Drawable drawable, StreamSettings screen, int[] from, int[] to) {
        Drawable copy = drawable.getConstantState().newDrawable(screen.getResources(), screen.getTheme()).mutate();
        float density = screen.getResources().getDisplayMetrics().density;
        int w = Math.round(400 * density), h = Math.round(56 * density);
        copy.setBounds(0, 0, w, h);
        copy.setState(from);
        copy.jumpToCurrentState();
        // No jump here: a fade would still draw the state left.
        if (to != null) copy.setState(to);
        android.graphics.Bitmap bitmap = android.graphics.Bitmap.createBitmap(w, h, android.graphics.Bitmap.Config.ARGB_8888);
        copy.draw(new android.graphics.Canvas(bitmap));
        return bitmap;
    }

    private static final int[] FOCUSED = {android.R.attr.state_enabled, android.R.attr.state_focused};
    private static final int[] IDLE = {android.R.attr.state_enabled};

    /**
     * nightjar#7 (Shield TV): the ring was on one rail row and a grey fill on another (the shown section), and the right
     * pane's focused row was grey and ringed, so grey meant focus there and the shown section here. Focus is now one
     * look in both panes, fill and amber ring; the shown section has no fill, only the amber bar, icon and title.
     */
    @Test
    public void focusIsFillAndRingInBothPanesAndTheShownSectionIsNotFilled() {
        StreamSettings screen = open();
        // Presets is no section: the pane keeps showing Input, the section a TV opens on.
        goToRow(screen, "stream_presets");
        ListView list = sections(screen);
        float density = screen.getResources().getDisplayMetrics().density;
        int ring = com.google.android.material.color.MaterialColors.getColor(screen, R.attr.glassRing, 0);
        int ink = com.google.android.material.color.MaterialColors.getColor(screen, R.attr.glassOnSelected, 0);
        ListView pane = (ListView) screen.getFragmentManager().findFragmentById(R.id.stream_settings).getView()
                .findViewById(android.R.id.list);
        android.graphics.Bitmap rail = render(list.getSelector(), screen, FOCUSED, null);
        android.graphics.Bitmap row = render(pane.getSelector(), screen, FOCUSED, null);
        int w = rail.getWidth(), h = rail.getHeight();
        assertEquals("the rail's ring is the page's", ring, rail.getPixel(w / 2, 1));
        assertTrue("the rail's focused row is filled", android.graphics.Color.alpha(rail.getPixel(w / 2, h / 2)) > 0);
        assertEquals("with the right pane's fill", row.getPixel(w / 2, h / 2), rail.getPixel(w / 2, h / 2));

        // Focus in the right pane: the rail shows only the mark of the section it opened.
        goToRow(screen, "input");
        press(screen, KeyEvent.KEYCODE_DPAD_RIGHT);
        assertFalse(list.hasFocus());
        int input = StreamSettings.RootFragment.positionOf(list, "input");
        assertEquals(input, list.getCheckedItemPosition());
        View shown = list.getChildAt(input - list.getFirstVisiblePosition());
        View other = list.getChildAt(StreamSettings.RootFragment.positionOf(list, "video") - list.getFirstVisiblePosition());
        assertTrue(shown.isActivated());
        android.graphics.Bitmap pill = render(shown.getBackground(), screen, shown.getDrawableState(), null);
        assertEquals("no fill on the shown section", 0, android.graphics.Color.alpha(pill.getPixel(pill.getWidth() / 2, pill.getHeight() / 2)));
        assertEquals("its title takes the selected fill's ink", ink, ((TextView) shown.findViewById(android.R.id.title)).getCurrentTextColor());
        android.widget.ImageView icon = shown.findViewById(android.R.id.icon);
        assertEquals("and its icon", ink, androidx.core.widget.ImageViewCompat.getImageTintList(icon)
                .getColorForState(icon.getDrawableState(), 0));
        assertTrue("another section's title does not",
                ink != ((TextView) other.findViewById(android.R.id.title)).getCurrentTextColor());
    }

    /** nightjar#7: the ring faded out of the row it left while it was already on the next, two rings at once. It goes at once. */
    @Test
    public void theRingLeavesARowAtOnceInBothPanes() {
        StreamSettings screen = open();
        ListView list = sections(screen);
        ListView pane = (ListView) screen.getFragmentManager().findFragmentById(R.id.stream_settings).getView()
                .findViewById(android.R.id.list);
        for (Drawable selector : new Drawable[] {list.getSelector(), pane.getSelector()}) {
            android.graphics.Bitmap left = render(selector, screen, FOCUSED, IDLE);
            for (int y = 0; y < left.getHeight(); y++) {
                for (int x = 0; x < left.getWidth(); x++) {
                    if (android.graphics.Color.alpha(left.getPixel(x, y)) != 0) {
                        throw new AssertionError("the ring is still drawn at " + x + "," + y);
                    }
                }
            }
        }
    }

    /**
     * nightjar#7 (Shield TV): Right into a section put its first row at the pane's top with its group's gap scrolled away, so
     * the list's top fade lay over the row and its ring had no top edge. The list stays at its top and the whole ring shows.
     */
    @Test
    public void enteringASectionShowsTheFirstRowsWholeRing() {
        StreamSettings screen = open();
        for (String section : StreamSettings.SECTIONS.keySet()) {
            goToRow(screen, section);
            // The pane follows the selected row after its pause.
            ShadowLooper.idleMainLooper(1, TimeUnit.SECONDS);
            settle(screen);
            press(screen, KeyEvent.KEYCODE_DPAD_RIGHT);
            ListView pane = (ListView) screen.getFragmentManager().findFragmentById(R.id.stream_settings).getView()
                    .findViewById(android.R.id.list);
            assertTrue(section + ": Right enters the pane", pane.hasFocus());
            int first = PaneFocus.target(-1, pane.getCount(), pane.getAdapter()::isEnabled);
            assertEquals(section + ": on its first row", first, pane.getSelectedItemPosition());
            assertEquals(section + ": nothing above the first row is scrolled away", 0, pane.getFirstVisiblePosition());
            assertEquals(section + ": its group's gap shows", pane.getListPaddingTop(), pane.getChildAt(0).getTop());
            // The ring is the list's selector, drawn over the row widened by the selector's padding.
            View row = pane.getSelectedView();
            android.graphics.Rect padding = new android.graphics.Rect();
            pane.getSelector().getPadding(padding);
            int ringTop = row.getTop() - padding.top;
            int ringBottom = row.getBottom() + padding.bottom;
            // The list shows its bounds, or what is inside its padding when it clips there, and its top fade takes the first
            // strength × length pixels of that.
            int top = pane.getClipToPadding() ? pane.getPaddingTop() : 0;
            int bottom = pane.getHeight() - (pane.getClipToPadding() ? pane.getPaddingBottom() : 0);
            float strength = org.robolectric.util.ReflectionHelpers.<Float>callInstanceMethod(pane, "getTopFadingEdgeStrength");
            float faded = top + Math.max(0f, Math.min(1f, strength)) * pane.getVerticalFadingEdgeLength();
            assertTrue(section + ": the ring (" + ringTop + " to " + ringBottom + ") is inside the list (" + top + " to " + bottom + ")",
                    ringTop >= top && ringBottom <= bottom);
            assertTrue(section + ": the ring's top edge at " + ringTop + " is below the top fade, which ends at " + faded,
                    ringTop >= faded);
            press(screen, KeyEvent.KEYCODE_DPAD_LEFT);
            assertTrue(section + ": Left returns to the rail", sections(screen).hasFocus());
        }
    }

    /**
     * TV run on a TCL C6K: the preset list marked its D-pad row with a grey block. The row takes the amber outline the
     * Settings lists use, and nothing fills it.
     */
    @Test
    public void thePresetListMarksItsRowWithTheRingOverAQuietTint() {
        StreamSettings screen = open();
        ListView list = sections(screen);
        int presets = StreamSettings.RootFragment.positionOf(list, "stream_presets");
        while (list.getSelectedItemPosition() != presets) {
            press(screen, list.getSelectedItemPosition() < presets ? KeyEvent.KEYCODE_DPAD_DOWN : KeyEvent.KEYCODE_DPAD_UP);
        }
        press(screen, KeyEvent.KEYCODE_DPAD_CENTER);
        AlertDialog chooser = (AlertDialog) ShadowDialog.getLatestDialog();
        android.graphics.drawable.Drawable selector = chooser.getListView().getSelector().getConstantState()
                .newDrawable(chooser.getContext().getResources(), chooser.getContext().getTheme()).mutate();
        float density = screen.getResources().getDisplayMetrics().density;
        int w = Math.round(400 * density), h = Math.round(48 * density);
        selector.setBounds(0, 0, w, h);
        selector.setState(new int[] {android.R.attr.state_enabled, android.R.attr.state_focused});
        selector.jumpToCurrentState();
        android.graphics.Bitmap row = android.graphics.Bitmap.createBitmap(w, h, android.graphics.Bitmap.Config.ARGB_8888);
        selector.draw(new android.graphics.Canvas(row));
        int ring = com.google.android.material.color.MaterialColors.getColor(chooser.getContext(), R.attr.glassRing, 0);
        assertEquals("the outline is the glass ring", ring | 0xFF000000, row.getPixel(w / 2, Math.round(density)) | 0xFF000000);
        assertTrue("no grey block inside it, a quiet tint", android.graphics.Color.alpha(row.getPixel(w / 2, h / 2)) < 0x40);
        chooser.dismiss();
    }

    /**
     * nightjar#7 (TCL C6K): the right pane kept a deep foot it did not clip to. A ListView fills that foot scrolling
     * down and drops its rows scrolling up, so the bottom row showed one way and left a blank band the other. The pane
     * now ends on the screen's edge, as a handheld's rail does; the rail keeps its foot for the expanding summary.
     */
    @Test
    public void theRightPaneEndsOnTheScreensEdgeAndTheRailKeepsItsFoot() {
        StreamSettings screen = open();
        ListView pane = (ListView) screen.getFragmentManager().findFragmentById(R.id.stream_settings).getView()
                .findViewById(android.R.id.list);
        int small = screen.getResources().getDimensionPixelSize(R.dimen.space_xs);
        assertEquals("the right pane's foot", small, pane.getPaddingBottom());
        assertTrue("the rail keeps its foot", sections(screen).getPaddingBottom() > small);
    }
}
