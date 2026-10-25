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

    /** A 1080p TV with overscan leaves a short grid: art turns landscape, and the small tiles stay the smaller. */
    @Test
    @Config(qualifiers = "w960dp-h540dp-television-xhdpi")
    public void smallTvTilesAreSmallerThanLargeOnesInAShortGrid() {
        try (var controller = Robolectric.buildActivity(com.limelight.ui.UiNavigationTest.Screen.class).setup()) {
            int[] artHeight = new int[2];
            int[] artWidth = new int[2];
            int[] columns = new int[2];
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
                // 700 px = 350 dp tall, the height a 1080p TV leaves for the grid below its header.
                grid.measure(android.view.View.MeasureSpec.makeMeasureSpec(1920, android.view.View.MeasureSpec.EXACTLY),
                        android.view.View.MeasureSpec.makeMeasureSpec(700, android.view.View.MeasureSpec.EXACTLY));
                grid.layout(0, 0, 1920, 700);
                assertTrue(grid.hasShortViewport());
                android.view.View cover = grid.getChildAt(0).findViewById(R.id.grid_image);
                int i = small ? 1 : 0;
                artHeight[i] = cover.getHeight();
                artWidth[i] = cover.getWidth();
                columns[i] = grid.getNumColumns();
                assertEquals(Math.round(cover.getWidth() * 2f / 3), cover.getHeight());
            }
            assertTrue(artHeight[1] < artHeight[0]);
            assertTrue(artWidth[1] < artWidth[0]);
            assertTrue(columns[1] > columns[0]);
        }
    }

    @Test
    public void rebindingARecycledCardClearsWhatTheLastCoverLeftOnIt() {
        try (var controller = Robolectric.buildActivity(com.limelight.ui.UiNavigationTest.Screen.class).setup()) {
            android.view.View card = android.view.LayoutInflater.from(controller.get()).inflate(R.layout.app_grid_item, null);
            android.widget.ImageView cover = card.findViewById(R.id.grid_image);
            cover.setScaleType(android.widget.ImageView.ScaleType.FIT_XY);
            cover.setScaleX(1.3f);
            cover.setTranslationY(40);
            cover.setAlpha(0.2f);
            cover.startAnimation(new android.view.animation.AlphaAnimation(0, 1));
            GenericGridAdapter.resetRecycledState(card, cover);
            assertEquals(android.widget.ImageView.ScaleType.CENTER_CROP, cover.getScaleType());
            assertEquals(1f, cover.getScaleX(), 0);
            assertEquals(0f, cover.getTranslationY(), 0);
            assertEquals(1f, cover.getAlpha(), 0);
            assertNull(cover.getAnimation());
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
