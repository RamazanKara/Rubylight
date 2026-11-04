package com.limelight.ui;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import android.app.Activity;
import android.app.Application;
import android.preference.PreferenceManager;
import android.text.TextUtils;
import android.widget.Button;
import android.widget.FrameLayout;
import android.widget.LinearLayout;

import com.limelight.R;

import org.junit.After;
import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;

/** A tile's label scrolls while its tile has focus and keeps the ellipsis otherwise, or always with Reduce motion. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class, qualifiers = "television")
public class MarqueeLabelTest {
    private Activity activity;
    private FrameLayout tile;
    private MarqueeLabel label;
    private Button other;

    @Before
    public void setUp() {
        ActivityController<Activity> controller = Robolectric.buildActivity(Activity.class);
        activity = controller.get();
        activity.setTheme(R.style.AppTheme);
        controller.create().start().resume();
        PreferenceManager.getDefaultSharedPreferences(activity).edit().clear().commit();
        LinearLayout root = new LinearLayout(activity);
        tile = new FrameLayout(activity);
        tile.setFocusable(true);
        label = new MarqueeLabel(activity);
        label.setText("A game whose name is far too long for its tile");
        tile.addView(label, new FrameLayout.LayoutParams(100, 40));
        root.addView(tile, new LinearLayout.LayoutParams(100, 100));
        other = new Button(activity);
        other.setFocusable(true);
        root.addView(other, new LinearLayout.LayoutParams(100, 100));
        activity.setContentView(root);
        controller.visible();
    }

    @After
    public void tearDown() {
        PreferenceManager.getDefaultSharedPreferences(activity).edit().clear().commit();
    }

    @Test
    public void scrollsOnlyWhileItsTileHasFocus() {
        assertTrue(other.requestFocus());
        assertEquals(TextUtils.TruncateAt.END, label.getEllipsize());
        assertFalse(label.isSelected());

        assertTrue(tile.requestFocus());
        assertEquals(TextUtils.TruncateAt.MARQUEE, label.getEllipsize());
        assertTrue(label.isSelected());
        // One line, so it never grows over the art.
        assertEquals(1, label.getMaxLines());

        assertTrue(other.requestFocus());
        assertEquals(TextUtils.TruncateAt.END, label.getEllipsize());
        assertFalse(label.isSelected());
    }

    /**
     * nightjar#7: a TV library's grid holds the focus itself, and a tile blocks focus from its children, so "the focus holds
     * this label" was true of every tile in view and they all scrolled. Only the selected tile does, and the selection moves.
     */
    @Test
    public void inAGridOnlyTheSelectedTileScrolls() {
        android.widget.GridView grid = new android.widget.GridView(activity);
        grid.setNumColumns(3);
        grid.setFocusable(true);
        grid.setAdapter(new android.widget.BaseAdapter() {
            @Override public int getCount() { return 3; }
            @Override public Object getItem(int position) { return position; }
            @Override public long getItemId(int position) { return position; }
            @Override public android.view.View getView(int position, android.view.View recycled, android.view.ViewGroup parent) {
                if (recycled != null) return recycled;
                FrameLayout cell = new FrameLayout(activity);
                cell.setDescendantFocusability(android.view.ViewGroup.FOCUS_BLOCK_DESCENDANTS);
                MarqueeLabel name = new MarqueeLabel(activity);
                name.setText("A game whose name is far too long for its tile");
                cell.addView(name, new FrameLayout.LayoutParams(100, 40));
                return cell;
            }
        });
        activity.setContentView(grid, new android.view.ViewGroup.LayoutParams(600, 300));
        TvScreensPreviewTest.leaveTouchMode(activity);
        layout(grid);
        assertTrue(grid.requestFocus());
        grid.setSelection(0);
        layout(grid);
        assertScrolling(grid, 0);

        grid.setSelection(2);
        layout(grid);
        assertScrolling(grid, 2);
    }

    private static void layout(android.view.View view) {
        org.robolectric.shadows.ShadowLooper.idleMainLooper();
        view.measure(android.view.View.MeasureSpec.makeMeasureSpec(600, android.view.View.MeasureSpec.EXACTLY),
                android.view.View.MeasureSpec.makeMeasureSpec(300, android.view.View.MeasureSpec.EXACTLY));
        view.layout(0, 0, 600, 300);
    }

    private static void assertScrolling(android.widget.GridView grid, int tile) {
        assertEquals(3, grid.getChildCount());
        for (int i = 0; i < 3; i++) {
            MarqueeLabel name = (MarqueeLabel) ((FrameLayout) grid.getChildAt(i)).getChildAt(0);
            assertEquals("tile " + i, i == tile, name.isSelected());
            assertEquals("tile " + i, i == tile ? TextUtils.TruncateAt.MARQUEE : TextUtils.TruncateAt.END, name.getEllipsize());
        }
    }

    @Test
    public void reduceMotionKeepsTheEllipsis() {
        assertTrue(other.requestFocus());
        PreferenceManager.getDefaultSharedPreferences(activity).edit()
                .putString(Motion.REDUCE_MOTION, Motion.REDUCE).commit();
        assertTrue(tile.requestFocus());
        assertEquals(TextUtils.TruncateAt.END, label.getEllipsize());
        assertFalse(label.isSelected());
    }

    @Test
    public void aLabelIsWithinTheTileThatHoldsItOnly() {
        assertTrue(MarqueeLabel.within(label, tile));
        assertTrue(MarqueeLabel.within(label, label));
        assertFalse(MarqueeLabel.within(label, other));
        assertFalse(MarqueeLabel.within(label, null));
    }
}
