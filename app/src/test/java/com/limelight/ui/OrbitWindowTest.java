package com.limelight.ui;

import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import android.app.Activity;
import android.view.View;

import com.limelight.R;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;

/** The orbit stops turning while its window is away, as behind a stopped activity. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 34)
public class OrbitWindowTest {
    @Test
    public void aHiddenWindowStopsTheTurn() {
        try (var controller = Robolectric.buildActivity(Activity.class).setup().visible()) {
            Activity activity = controller.get();
            activity.setTheme(R.style.AppTheme);
            OrbitView orbit = new OrbitView(activity);
            activity.setContentView(orbit);
            orbit.setMoving(true);
            assertTrue(orbit.isTurning());
            orbit.dispatchWindowVisibilityChanged(View.GONE);
            assertFalse("nobody sees it", orbit.isTurning());
            orbit.dispatchWindowVisibilityChanged(View.VISIBLE);
            assertTrue(orbit.isTurning());
        }
    }
}
