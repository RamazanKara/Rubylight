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
