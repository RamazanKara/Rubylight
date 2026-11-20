package com.limelight.ui;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import android.app.Activity;
import android.content.Context;
import android.view.KeyEvent;
import android.view.SoundEffectConstants;
import android.view.View;
import android.view.ViewGroup;
import android.widget.FrameLayout;
import android.widget.LinearLayout;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;

import java.util.ArrayList;
import java.util.List;

/**
 * nightjar#7: in the stream menu a D-pad's Left and Right played the selection sound and Up and Down did not. A sheet that
 * scrolls moves focus up and down itself, before the window that plays the sound sees the key.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29)
public class SheetNavigationSoundTest {
    private static final List<Integer> SOUNDS = new ArrayList<>();

    /** A row that records the sounds played for it. */
    private static final class Row extends View {
        Row(Context context) {
            super(context);
            setFocusable(true);
            setFocusableInTouchMode(true);
        }

        @Override
        public void playSoundEffect(int soundConstant) {
            SOUNDS.add(soundConstant);
        }
    }

    private static boolean press(View scroll, int key) {
        boolean handled = scroll.dispatchKeyEvent(new KeyEvent(KeyEvent.ACTION_DOWN, key));
        scroll.dispatchKeyEvent(new KeyEvent(KeyEvent.ACTION_UP, key));
        return handled;
    }

    @Test
    public void upAndDownBetweenRowsOfAScrollingSheetPlayTheNavigationSound() {
        SOUNDS.clear();
        try (var controller = Robolectric.buildActivity(Activity.class).setup().visible()) {
            Activity activity = controller.get();
            ActionSheet.StillScroll scroll = new ActionSheet.StillScroll(activity);
            LinearLayout rows = new LinearLayout(activity);
            rows.setOrientation(LinearLayout.VERTICAL);
            Row[] row = new Row[4];
            for (int i = 0; i < row.length; i++) {
                row[i] = new Row(activity);
                rows.addView(row[i], new LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, 200));
            }
            scroll.addView(rows, new ViewGroup.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT));
            FrameLayout root = new FrameLayout(activity);
            root.addView(scroll, new FrameLayout.LayoutParams(600, 500));
            activity.setContentView(root);
            root.measure(View.MeasureSpec.makeMeasureSpec(600, View.MeasureSpec.EXACTLY),
                    View.MeasureSpec.makeMeasureSpec(500, View.MeasureSpec.EXACTLY));
            root.layout(0, 0, 600, 500);
            assertTrue("the rows are taller than the sheet, so it scrolls", scroll.canScrollVertically(1));
            assertTrue(row[0].requestFocus());

            assertTrue("the sheet moves Down itself", press(scroll, KeyEvent.KEYCODE_DPAD_DOWN));
            assertTrue(row[1].isFocused());
            assertTrue(press(scroll, KeyEvent.KEYCODE_DPAD_UP));
            assertTrue(row[0].isFocused());
            assertEquals(List.of(SoundEffectConstants.NAVIGATION_DOWN, SoundEffectConstants.NAVIGATION_UP), SOUNDS);

            // Left and Right are the window's, which plays their sound itself: the sheet leaves them alone.
            assertFalse(press(scroll, KeyEvent.KEYCODE_DPAD_LEFT));
            assertFalse(press(scroll, KeyEvent.KEYCODE_DPAD_RIGHT));
            // Up from the first row has nowhere to go inside the sheet: no move, no sound.
            press(scroll, KeyEvent.KEYCODE_DPAD_UP);
            assertTrue(row[0].isFocused());
            assertEquals(2, SOUNDS.size());
        }
    }
}
