package com.limelight.preferences;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

import android.app.Application;
import android.preference.Preference;
import android.preference.PreferenceManager;
import android.view.KeyEvent;
import android.view.View;
import android.widget.ListView;

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
        // Up to the top: Search, then the rows in their TV order.
        for (int i = 0; i < list.getCount() && list.getSelectedItemPosition() != 0; i++) press(screen, KeyEvent.KEYCODE_DPAD_UP);
        assertEquals(StreamSettings.SEARCH_ROW_KEY, keyAt(list, list.getSelectedItemPosition()));
        List<String> visited = new ArrayList<>();
        visited.add(keyAt(list, list.getSelectedItemPosition()));
        for (int i = 1; i < list.getCount(); i++) {
            int before = list.getSelectedItemPosition();
            press(screen, KeyEvent.KEYCODE_DPAD_DOWN);
            assertEquals("Down from " + keyAt(list, before) + " moves one row", before + 1, list.getSelectedItemPosition());
            assertTrue(list.hasFocus());
            visited.add(keyAt(list, list.getSelectedItemPosition()));
        }
        assertEquals(Arrays.asList(StreamSettings.SEARCH_ROW_KEY, "input", "stream_presets", "video", "display",
                "audio", "streaming", "app", "advanced"), visited);
        for (int i = list.getCount() - 2; i >= 0; i--) {
            press(screen, KeyEvent.KEYCODE_DPAD_UP);
            assertEquals("Up moves one row", i, list.getSelectedItemPosition());
        }
    }

    /** TV run on a TCL C6K: after a few presses the search row sat half under the subtitle. No rail row is ever cut at the top. */
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
     * TV run on a TCL C6K: with search results on the right the rail still marked Controls. The search row is marked
     * (grey pill and amber bar), and Left from the results comes back to it.
     */
    @Test
    public void searchResultsMarkTheSearchRowAndLeftReturnsToIt() {
        StreamSettings screen = open();
        ListView list = sections(screen);
        int search = StreamSettings.RootFragment.positionOf(list, StreamSettings.SEARCH_ROW_KEY);
        assertEquals(0, search);
        assertEquals("input", keyAt(list, list.getCheckedItemPosition()));
        press(screen, KeyEvent.KEYCODE_DPAD_UP);
        press(screen, KeyEvent.KEYCODE_DPAD_CENTER);
        androidx.appcompat.widget.SearchView field = screen.findViewById(R.id.settings_search);
        assertTrue("Select on the search row opens the field", field.getVisibility() == View.VISIBLE);
        field.setQuery("controller", true);
        settle(screen);
        list = sections(screen);
        assertEquals("the rail marks the search row", StreamSettings.SEARCH_ROW_KEY,
                keyAt(list, list.getCheckedItemPosition()));
        SettingsRail rail = org.robolectric.util.ReflectionHelpers.getField(screen, "rail");
        assertEquals("the amber bar is on the search row", search, rail.position());
        ListView results = (ListView) screen.getFragmentManager().findFragmentById(R.id.stream_settings).getView()
                .findViewById(android.R.id.list);
        results.requestFocus();
        settle(screen);
        press(screen, KeyEvent.KEYCODE_DPAD_LEFT);
        assertTrue(list.hasFocus());
        assertEquals("Left lands on the marked row", StreamSettings.SEARCH_ROW_KEY,
                keyAt(list, list.getSelectedItemPosition()));
    }

    @Test
    public void onASectionTheRingAndTheShownSectionAreTheSameRow() {
        StreamSettings screen = open();
        ListView list = sections(screen);
        // Display: past Presets, and not the section a TV opens on.
        for (int i = 0; i < list.getCount() && !"display".equals(keyAt(list, list.getSelectedItemPosition())); i++) {
            press(screen, KeyEvent.KEYCODE_DPAD_DOWN);
        }
        String selected = keyAt(list, list.getSelectedItemPosition());
        assertEquals("display", selected);
        // The pane follows the selected row after its pause, and the grey pill follows the pane.
        ShadowLooper.idleMainLooper(1, TimeUnit.SECONDS);
        settle(screen);
        assertEquals(selected, keyAt(list, list.getCheckedItemPosition()));
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
        assertTrue("Presets " + presetRow.getHeight() + " px against Video " + videoRow.getHeight() + " px",
                presetRow.getHeight() <= videoRow.getHeight());
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

    /**
     * TV run on a TCL C6K: the preset list marked its D-pad row with a grey block. The row takes the amber outline the
     * Settings lists use, and nothing fills it.
     */
    @Test
    public void thePresetListMarksItsRowWithTheAmberOutline() {
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
        int amber = com.google.android.material.color.MaterialColors.getColor(chooser.getContext(),
                androidx.appcompat.R.attr.colorPrimary, 0);
        assertEquals("the outline is amber", amber, row.getPixel(w / 2, Math.round(3 * density)));
        assertEquals("no grey block inside it", 0, android.graphics.Color.alpha(row.getPixel(w / 2, h / 2)));
        chooser.dismiss();
    }
}
