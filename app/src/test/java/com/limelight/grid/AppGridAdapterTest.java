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
                // The poster's root pads itself with its glow; the card inside it is what is cropped and rounded.
                android.view.View tile = grid.getChildAt(0);
                com.google.android.material.card.MaterialCardView card = tile.findViewById(R.id.grid_card);
                assertEquals(Math.round(16 * 2f), tile.getPaddingLeft());
                android.view.View cover = card.findViewById(R.id.grid_image);
                assertEquals(tile.getWidth() - 2 * tile.getPaddingLeft(), cover.getWidth());
                assertEquals(Math.round(cover.getWidth() * 3f / 2), cover.getHeight());
                assertTrue(card.getClipToOutline());
                assertFalse(card.getPreventCornerOverlap());
                assertEquals(0, card.getChildAt(0).getPaddingLeft());
            }
        }
    }

    /** A 1080p TV with overscan leaves a short grid: posters stay 2:3, and the small tiles stay the smaller. */
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
                assertEquals("a poster stays 2:3 in a short grid", Math.round(cover.getWidth() * 3f / 2), cover.getHeight());
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
            GenericGridAdapter.resetRecycledState(card, cover, true);
            assertEquals(android.widget.ImageView.ScaleType.CENTER_CROP, cover.getScaleType());
            assertEquals(1f, cover.getScaleX(), 0);
            assertEquals(0f, cover.getTranslationY(), 0);
            assertEquals(1f, cover.getAlpha(), 0);
            assertNull(cover.getAnimation());
        }
    }

    @Test
    public void rebindingAComputerCardKeepsItsOwnScaleType() {
        try (var controller = Robolectric.buildActivity(com.limelight.ui.UiNavigationTest.Screen.class).setup()) {
            android.view.View card = android.view.LayoutInflater.from(controller.get()).inflate(R.layout.pc_grid_item, null);
            android.widget.ImageView cover = card.findViewById(R.id.grid_image);
            android.widget.ImageView.ScaleType own = cover.getScaleType();
            GenericGridAdapter.resetRecycledState(card, cover, false);
            assertEquals(own, cover.getScaleType());
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

    /** A TV library of {@code count} large posters in an 864 dp wide grid that has the focus. */
    private static com.limelight.ui.AdaptiveGridView posterGrid(android.app.Activity screen, int count, int heightPx) {
        com.limelight.ui.AdaptiveGridView grid = new com.limelight.ui.AdaptiveGridView(screen);
        grid.setColumnPolicy(false, false);
        // The TV library's own padding (grid_padding_top and grid_padding_bottom at xhdpi); the last row ends above it.
        grid.setPadding(0, 32, 0, 64);
        grid.setClipToPadding(false);
        GenericGridAdapter<String> adapter = new GenericGridAdapter<>(screen, R.layout.app_grid_item) {
            @Override public void populateView(android.view.View parent, android.widget.ImageView image,
                    android.widget.ProgressBar progress, android.widget.TextView text, String item) { }
        };
        for (int i = 0; i < count; i++) adapter.itemList.add("Game " + i);
        grid.setAdapter(adapter);
        // The reveal after a key only runs on a grid that is attached.
        android.widget.FrameLayout container = new android.widget.FrameLayout(screen);
        container.addView(grid, new android.widget.FrameLayout.LayoutParams(1728, heightPx));
        screen.setContentView(container);
        layout(grid, heightPx);
        assertEquals(4, grid.getNumColumns());
        assertTrue(grid.requestFocusFromTouch());
        grid.setSelection(0);
        layout(grid, heightPx);
        return grid;
    }

    /** 1728 px = 864 dp: a 960 dp TV less its 24 dp overscan on each side. */
    private static void layout(android.view.View grid, int heightPx) {
        grid.measure(android.view.View.MeasureSpec.makeMeasureSpec(1728, android.view.View.MeasureSpec.EXACTLY),
                android.view.View.MeasureSpec.makeMeasureSpec(heightPx, android.view.View.MeasureSpec.EXACTLY));
        grid.layout(0, 0, 1728, heightPx);
    }

    private static void key(com.limelight.ui.AdaptiveGridView grid, int keyCode, int repeat) {
        long now = android.os.SystemClock.uptimeMillis();
        grid.onKeyDown(keyCode, new android.view.KeyEvent(now, now, android.view.KeyEvent.ACTION_DOWN, keyCode, repeat));
        // The grid scrolls a row into view with a short smooth scroll; let it finish.
        org.robolectric.shadows.ShadowLooper.idleMainLooper(1, java.util.concurrent.TimeUnit.SECONDS);
        layout(grid, grid.getHeight());
        org.robolectric.shadows.ShadowLooper.idleMainLooper(1, java.util.concurrent.TimeUnit.SECONDS);
    }

    @Test
    @Config(qualifiers = "w960dp-h540dp-television-xhdpi")
    public void theFocusedPosterIsWhollyInViewAfterFourDownPresses() {
        try (var controller = Robolectric.buildActivity(com.limelight.ui.UiNavigationTest.Screen.class).setup().visible()) {
            com.limelight.ui.AdaptiveGridView grid = posterGrid(controller.get(), 20, 700);
            for (int i = 0; i < 4; i++) {
                key(grid, android.view.KeyEvent.KEYCODE_DPAD_DOWN, 0);
                android.view.View card = grid.getSelectedView();
                assertEquals(4 * (i + 1), grid.getSelectedItemPosition());
                // The card grows by its focus scale around its centre: that growth has to fit as well.
                int growth = (int) Math.ceil(0.08f * card.getHeight() / 2);
                assertTrue("press " + (i + 1) + ": top " + card.getTop() + " growth " + growth, card.getTop() - growth >= 0);
                assertTrue("press " + (i + 1) + ": bottom " + card.getBottom() + " of " + grid.getHeight(),
                        card.getBottom() + growth <= grid.getHeight());
            }
        }
    }

    @Test
    @Config(qualifiers = "w960dp-h540dp-television-xhdpi")
    public void theGlowReachesItsFullPaddingBeyondThePoster() {
        try (var controller = Robolectric.buildActivity(com.limelight.ui.UiNavigationTest.Screen.class).setup().visible()) {
            for (int layoutId : new int[] {R.layout.app_grid_item, R.layout.app_grid_item_small}) {
                android.view.View root = android.view.LayoutInflater.from(controller.get()).inflate(layoutId, null);
                root.measure(android.view.View.MeasureSpec.makeMeasureSpec(432, android.view.View.MeasureSpec.EXACTLY),
                        android.view.View.MeasureSpec.makeMeasureSpec(0, android.view.View.MeasureSpec.UNSPECIFIED));
                root.layout(0, 0, root.getMeasuredWidth(), root.getMeasuredHeight());
                android.view.View glow = root.findViewById(R.id.grid_glow);
                android.view.View card = root.findViewById(R.id.grid_card);
                int reach = controller.get().getResources().getDimensionPixelSize(R.dimen.poster_glow);
                // A negative layout_margin is ignored by the framework: only the four sides separately reach out.
                assertEquals(card.getLeft() - reach, glow.getLeft());
                assertEquals(card.getTop() - reach, glow.getTop());
                assertEquals(card.getRight() + reach, glow.getRight());
                assertEquals(card.getBottom() + reach, glow.getBottom());
            }
        }
    }

    @Test
    @Config(qualifiers = "w960dp-h540dp-television-xhdpi")
    public void theRingIsDrawnAroundThePosterAndNeverOverItsArt() {
        try (var controller = Robolectric.buildActivity(com.limelight.ui.UiNavigationTest.Screen.class).setup().visible()) {
            for (int layoutId : new int[] {R.layout.app_grid_item, R.layout.app_grid_item_small}) {
                android.view.View root = android.view.LayoutInflater.from(controller.get()).inflate(layoutId, null);
                root.measure(android.view.View.MeasureSpec.makeMeasureSpec(432, android.view.View.MeasureSpec.EXACTLY),
                        android.view.View.MeasureSpec.makeMeasureSpec(0, android.view.View.MeasureSpec.UNSPECIFIED));
                root.layout(0, 0, root.getMeasuredWidth(), root.getMeasuredHeight());
                android.view.View ring = root.findViewById(R.id.grid_ring);
                android.view.View card = root.findViewById(R.id.grid_card);
                int reach = Math.round(5 * controller.get().getResources().getDisplayMetrics().density);
                assertSame("the ring is the poster's sibling, not a layer over its art", root, ring.getParent());
                assertEquals(card.getLeft() - reach, ring.getLeft());
                assertEquals(card.getTop() - reach, ring.getTop());
                assertEquals(card.getRight() + reach, ring.getRight());
                assertEquals(card.getBottom() + reach, ring.getBottom());
                assertTrue("inside the glow's padding", ring.getLeft() >= 0 && ring.getTop() >= 0);
                assertTrue("drawn after the poster, so over it", ((android.view.ViewGroup) root).indexOfChild(ring) > ((android.view.ViewGroup) root).indexOfChild(card));
            }
        }
    }

    @Test
    @Config(qualifiers = "w960dp-h540dp-television-xhdpi")
    public void aHeldKeyPutsEachCardStraightIntoItsFinalFocusState() {
        try (var controller = Robolectric.buildActivity(com.limelight.ui.UiNavigationTest.Screen.class).setup().visible()) {
            com.limelight.ui.AdaptiveGridView grid = posterGrid(controller.get(), 20, 700);
            android.view.View first = grid.getSelectedView();
            key(grid, android.view.KeyEvent.KEYCODE_DPAD_RIGHT, 1);
            assertEquals(1, grid.getSelectedItemPosition());
            assertEquals("the new card is already scaled", 1.08f, grid.getSelectedView().getScaleX(), 0.001f);
            assertEquals("the old card is already at rest", 1f, first.getScaleX(), 0.001f);
            assertEquals("the new card already shows its title", 1f, grid.getSelectedView().findViewById(R.id.grid_title).getAlpha(), 0f);
            assertEquals("the old card's title is already gone", 0f, first.findViewById(R.id.grid_title).getAlpha(), 0f);
            key(grid, android.view.KeyEvent.KEYCODE_DPAD_RIGHT, 2);
            assertEquals(2, grid.getSelectedItemPosition());
            assertEquals(1.08f, grid.getSelectedView().getScaleX(), 0.001f);
            assertEquals(1f, grid.getChildAt(1).getScaleX(), 0.001f);
        }
    }

    /** The key moves the selection in onKeyDown itself, and the launch fires in the key-up that follows it. */
    @Test
    @Config(qualifiers = "w960dp-h540dp-television-xhdpi")
    public void theSelectionMovesInsideOnKeyDownAndTheLaunchFiresInTheSameKeyUp() {
        try (var controller = Robolectric.buildActivity(com.limelight.ui.UiNavigationTest.Screen.class).setup().visible()) {
            com.limelight.ui.AdaptiveGridView grid = posterGrid(controller.get(), 20, 700);
            int[] launched = {-1};
            grid.setOnItemClickListener((parent, view, position, id) -> launched[0] = position);

            long now = android.os.SystemClock.uptimeMillis();
            grid.dispatchKeyEvent(new android.view.KeyEvent(now, now, android.view.KeyEvent.ACTION_DOWN,
                    android.view.KeyEvent.KEYCODE_DPAD_RIGHT, 0));
            assertEquals("the selection is already on the next card when the key-down returns", 1, grid.getSelectedItemPosition());
            grid.dispatchKeyEvent(new android.view.KeyEvent(now, now, android.view.KeyEvent.ACTION_UP,
                    android.view.KeyEvent.KEYCODE_DPAD_RIGHT, 0));
            assertEquals(-1, launched[0]);
            layout(grid, grid.getHeight());

            grid.dispatchKeyEvent(new android.view.KeyEvent(now, now, android.view.KeyEvent.ACTION_DOWN,
                    android.view.KeyEvent.KEYCODE_DPAD_CENTER, 0));
            assertEquals("a press does not launch", -1, launched[0]);
            grid.dispatchKeyEvent(new android.view.KeyEvent(now, now, android.view.KeyEvent.ACTION_UP,
                    android.view.KeyEvent.KEYCODE_DPAD_CENTER, 0));
            assertEquals("the key-up launches the selected card", 1, launched[0]);
        }
    }
}
