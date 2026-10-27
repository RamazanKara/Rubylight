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
}
