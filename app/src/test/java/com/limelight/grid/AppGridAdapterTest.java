package com.limelight.grid;

import com.limelight.R;
import com.limelight.nvstream.http.NvApp;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;
import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 28, application = android.app.Application.class)
public class AppGridAdapterTest {
    @Test
    @Config(qualifiers = "w960dp-h540dp-television-xhdpi")
    public void smallTvCoversKeepTheirAspectRatioAndFillTheRoundedCard() {
        try (var controller = Robolectric.buildActivity(com.limelight.ui.UiNavigationTest.Screen.class).setup()) {
            for (boolean small : new boolean[] {false, true}) {
                com.limelight.ui.AdaptiveGridView grid = new com.limelight.ui.AdaptiveGridView(controller.get());
                grid.setColumnPolicy(false, small);
                GenericGridAdapter<String> adapter = new GenericGridAdapter<>(controller.get(),
                        small ? R.layout.app_grid_item_small : R.layout.app_grid_item) {
                    @Override public void populateView(android.view.View parent, android.widget.ImageView image,
                            android.widget.ProgressBar progress, android.widget.TextView text, String item) { }
                };
                adapter.itemList.add("Game");
                grid.setAdapter(adapter);
                grid.measure(android.view.View.MeasureSpec.makeMeasureSpec(1920, android.view.View.MeasureSpec.EXACTLY),
                        android.view.View.MeasureSpec.makeMeasureSpec(1080, android.view.View.MeasureSpec.EXACTLY));
                grid.layout(0, 0, 1920, 1080);
                assertEquals(small ? 6 : 4, grid.getNumColumns());
                com.google.android.material.card.MaterialCardView card =
                        (com.google.android.material.card.MaterialCardView) grid.getChildAt(0);
                android.view.View cover = card.findViewById(R.id.grid_image);
                assertEquals(Math.round(cover.getWidth() * 4f / 3), cover.getHeight());
                assertTrue(card.getClipToOutline());
                assertFalse(card.getPreventCornerOverlap());
                assertEquals(0, card.getChildAt(0).getPaddingLeft());
            }
        }
    }

    @Test
    public void remoteTilesDescribeTheirRole() {
        NvApp monitor = new NvApp("Monitor", 2147483505, false);
        NvApp input = new NvApp("Input", 2147483506, false);
        assertEquals(R.string.role_view_only, AppGridAdapter.roleLabel(monitor));
        assertEquals(R.string.role_input_only, AppGridAdapter.roleLabel(input));
        assertEquals(0, AppGridAdapter.roleLabel(new NvApp("Game", 10, false)));
    }

    @Test
    public void endRoleTilesDescribeTheActionRatherThanAStream() {
        assertEquals(R.string.stream_end_monitor,
                AppGridAdapter.roleLabel(new NvApp("End monitor", 2147483502, false)));
        assertEquals(R.string.stream_end_input,
                AppGridAdapter.roleLabel(new NvApp("End input", 2147483503, false)));
    }
}
