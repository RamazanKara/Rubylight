package com.limelight.preferences;

import android.app.Activity;
import android.app.Application;

import com.limelight.R;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;

import static org.junit.Assert.*;

/** Scrolled, a settings list fades out at its top as it does into the page's foot (Nightjar #61), never cut off under the title. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 34, application = Application.class)
public class GroupedListViewTest {
    @Test
    public void theTopFadesAndTheBottomIsLeftToThePage() {
        try (var controller = Robolectric.buildActivity(Activity.class).setup()) {
            Activity activity = controller.get();
            activity.setTheme(R.style.StreamTheme);
            GroupedListView list = new GroupedListView(activity);
            assertTrue(list.isVerticalFadingEdgeEnabled());
            assertEquals(activity.getResources().getDimensionPixelSize(R.dimen.settings_top_fade), list.getVerticalFadingEdgeLength());
            assertEquals(0f, list.getBottomFadingEdgeStrength(), 0f);
        }
    }
}
