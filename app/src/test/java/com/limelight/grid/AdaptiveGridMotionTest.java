package com.limelight.grid;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertTrue;

import android.app.Application;
import android.preference.PreferenceManager;
import android.provider.Settings;
import android.view.KeyEvent;
import android.view.View;
import android.widget.FrameLayout;
import android.widget.ImageView;
import android.widget.ProgressBar;
import android.widget.TextView;

import com.limelight.R;
import com.limelight.ui.AdaptiveGridView;
import com.limelight.ui.Motion;
import com.limelight.ui.UiNavigationTest;

import org.junit.After;
import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowLooper;

import java.util.concurrent.TimeUnit;

/** The TV library's row glide and the first cards rising in: both full on a press, absent on a held key and with Reduce motion. */
@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class, qualifiers = "w960dp-h540dp-television-xhdpi")
public class AdaptiveGridMotionTest {
    private ActivityController<UiNavigationTest.Screen> controller;
    private AdaptiveGridView grid;
    private int height;

    @Before
    public void reset() {
        Settings.Global.putFloat(RuntimeEnvironment.getApplication().getContentResolver(),
                Settings.Global.ANIMATOR_DURATION_SCALE, 1f);
        org.robolectric.util.ReflectionHelpers.callStaticMethod(android.animation.ValueAnimator.class, "setDurationScale",
                org.robolectric.util.ReflectionHelpers.ClassParameter.from(float.class, 1f));
        PreferenceManager.getDefaultSharedPreferences(RuntimeEnvironment.getApplication()).edit().clear().commit();
    }

    @After
    public void close() {
        if (controller != null) controller.close();
    }

    private void open(int heightPx) {
        height = heightPx;
        controller = Robolectric.buildActivity(UiNavigationTest.Screen.class).setup().visible();
        UiNavigationTest.Screen screen = controller.get();
        grid = new AdaptiveGridView(screen);
        grid.setColumnPolicy(false, false);
        grid.setPadding(0, 32, 0, 64);
        grid.setClipToPadding(false);
        GenericGridAdapter<String> adapter = new GenericGridAdapter<>(screen, R.layout.app_grid_item) {
            @Override public void populateView(View parent, ImageView image, ProgressBar progress, TextView text, String item) { }
        };
        for (int i = 0; i < 20; i++) adapter.itemList.add("Game " + i);
        grid.setAdapter(adapter);
        FrameLayout container = new FrameLayout(screen);
        container.addView(grid, new FrameLayout.LayoutParams(1728, heightPx));
        screen.setContentView(container);
        layout();
        grid.requestFocusFromTouch();
        grid.setSelection(0);
        layout();
        pass(1000);
    }

    private void layout() {
        grid.measure(View.MeasureSpec.makeMeasureSpec(1728, View.MeasureSpec.EXACTLY),
                View.MeasureSpec.makeMeasureSpec(height, View.MeasureSpec.EXACTLY));
        grid.layout(0, 0, 1728, height);
    }

    private static void pass(long ms) {
        ShadowLooper.idleMainLooper(ms, TimeUnit.MILLISECONDS);
    }

    private void press(int keyCode, int repeat) {
        long now = android.os.SystemClock.uptimeMillis();
        grid.onKeyDown(keyCode, new KeyEvent(now, now, KeyEvent.ACTION_DOWN, keyCode, repeat));
        layout();
        // The reveal that scrolls the row into view runs from the main looper, a moment after the key.
        pass(1);
    }

    private float largestOffset() {
        float largest = 0;
        for (int i = 0; i < grid.getChildCount(); i++) largest = Math.max(largest, Math.abs(grid.getChildAt(i).getTranslationY()));
        return largest;
    }

    private static void reduceMotion() {
        PreferenceManager.getDefaultSharedPreferences(RuntimeEnvironment.getApplication()).edit()
                .putString(Motion.REDUCE_MOTION, Motion.REDUCE).commit();
    }

    @Test
    public void aRowChangeGlidesTheCardsFromTheirOldPlaceAndLandsAtRest() {
        open(700);
        press(KeyEvent.KEYCODE_DPAD_DOWN, 0);
        assertEquals("the second row is selected", 4, grid.getSelectedItemPosition());
        assertEquals("one glide for the row change", 1, grid.glideCount());
        pass(400);
        assertEquals("and the glide ends exactly in place", 0f, largestOffset(), 0f);
    }

    @Test
    public void aHeldKeyDoesNotGlide() {
        open(700);
        press(KeyEvent.KEYCODE_DPAD_DOWN, 0);
        press(KeyEvent.KEYCODE_DPAD_DOWN, 1);
        assertEquals(8, grid.getSelectedItemPosition());
        assertEquals("only the first press glides; a held key puts every card straight in place", 1, grid.glideCount());
        assertEquals(0f, largestOffset(), 0f);
    }

    @Test
    public void withReducedMotionARowChangeDoesNotGlide() {
        reduceMotion();
        open(700);
        press(KeyEvent.KEYCODE_DPAD_DOWN, 0);
        assertEquals(4, grid.getSelectedItemPosition());
        assertEquals(0, grid.glideCount());
        assertEquals(0f, largestOffset(), 0f);
    }

    @Test
    public void aTouchScrollDoesNotGlide() {
        open(700);
        pass(1000);
        grid.scrollListBy(300);
        layout();
        pass(1);
        assertEquals(0, grid.glideCount());
        assertEquals(0f, largestOffset(), 0f);
    }

    @Test
    public void aRowChangeWithinTheViewDoesNotMoveAnything() {
        open(700);
        press(KeyEvent.KEYCODE_DPAD_RIGHT, 0);
        assertEquals(1, grid.getSelectedItemPosition());
        assertEquals(0, grid.glideCount());
        assertEquals(0f, largestOffset(), 0f);
    }

    @Test
    public void theFirstCardsRiseInStaggeredAndAtMostTenOfThem() {
        open(3000);
        assertTrue("a tall grid shows more than ten cards", grid.getChildCount() > 10);
        grid.playEnter();
        grid.getViewTreeObserver().dispatchOnPreDraw();
        float rise = 24 * controller.get().getResources().getDisplayMetrics().density;
        for (int i = 0; i < grid.getChildCount(); i++) {
            View card = grid.getChildAt(i);
            if (i < 10) {
                assertEquals("card " + i + " starts invisible", 0f, card.getAlpha(), 0f);
                assertEquals("and 24 dp below", rise, card.getTranslationY(), 0.5f);
            } else {
                assertEquals("card " + i + " is past the cap and stays put", 1f, card.getAlpha(), 0f);
            }
        }
        pass(1000);
        for (int i = 0; i < grid.getChildCount(); i++) {
            assertEquals(1f, grid.getChildAt(i).getAlpha(), 0f);
            assertEquals(0f, grid.getChildAt(i).getTranslationY(), 0f);
        }
    }

    @Test
    public void aKeyEndsTheRiseAtOnce() {
        open(3000);
        grid.playEnter();
        grid.getViewTreeObserver().dispatchOnPreDraw();
        assertEquals(0f, grid.getChildAt(3).getAlpha(), 0f);
        long now = android.os.SystemClock.uptimeMillis();
        grid.onKeyDown(KeyEvent.KEYCODE_DPAD_RIGHT, new KeyEvent(now, now, KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_DPAD_RIGHT, 0));
        for (int i = 0; i < 10; i++) {
            assertEquals(1f, grid.getChildAt(i).getAlpha(), 0f);
            assertEquals(0f, grid.getChildAt(i).getTranslationY(), 0f);
        }
    }

    @Test
    public void withReducedMotionTheCardsAreThereFromTheStart() {
        reduceMotion();
        open(3000);
        grid.playEnter();
        grid.getViewTreeObserver().dispatchOnPreDraw();
        for (int i = 0; i < grid.getChildCount(); i++) {
            assertEquals(1f, grid.getChildAt(i).getAlpha(), 0f);
            assertEquals(0f, grid.getChildAt(i).getTranslationY(), 0f);
        }
    }
}
