package com.limelight.ui;

import android.app.Application;
import android.view.View;
import android.widget.ArrayAdapter;
import android.widget.FrameLayout;

import com.limelight.R;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.annotation.Config;

import static org.junit.Assert.*;

@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class, qualifiers = "w393dp-h852dp-xhdpi",
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class AdaptiveGridViewTest {
    private static void measure(View view, int width, int height) {
        view.measure(View.MeasureSpec.makeMeasureSpec(width, View.MeasureSpec.EXACTLY),
                View.MeasureSpec.makeMeasureSpec(height, View.MeasureSpec.EXACTLY));
        view.layout(0, 0, width, height);
    }

    @Test
    public void rotatingWithoutAQueryKeepsTheLibraryCardsOnScreen() {
        try (var controller = Robolectric.buildActivity(UiNavigationTest.Screen.class).setup().visible()) {
            UiNavigationTest.Screen screen = controller.get();
            FrameLayout container = new FrameLayout(screen);
            screen.getLayoutInflater().inflate(R.layout.app_grid_view, container);
            AdaptiveGridView grid = container.findViewById(R.id.fragmentView);
            grid.setColumnPolicy(false, false);
            String[] apps = new String[12];
            for (int i = 0; i < apps.length; i++) apps[i] = "App " + i;
            grid.setAdapter(new ArrayAdapter<>(screen, android.R.layout.simple_list_item_1, apps));

            // 393 x 852 dp portrait at xhdpi, then the same phone in landscape.
            measure(container, 786, 1704);
            assertEquals(2, grid.getNumColumns());
            assertTrue(grid.getChildCount() > 0);
            assertFalse(grid.hasShortViewport());

            measure(container, 1704, 786);
            assertEquals(4, grid.getNumColumns());
            assertTrue("landscape grid lost its cards", grid.getChildCount() > 0);
            assertTrue(grid.hasShortViewport());
        }
    }

    @Test
    public void computerCardsStayInOneColumnOnPortraitPhones() {
        try (var controller = Robolectric.buildActivity(UiNavigationTest.Screen.class).setup()) {
            AdaptiveGridView grid = new AdaptiveGridView(controller.get());
            grid.setColumnPolicy(true, false);
            measure(grid, 786, 1704);
            assertEquals(1, grid.getNumColumns());
            measure(grid, 1704, 786);
            assertEquals(2, grid.getNumColumns());
        }
    }

    @Test
    public void theEndDirectionPastTheLastCardReachesTheScreensOwnButton() {
        try (var controller = Robolectric.buildActivity(UiNavigationTest.Screen.class).setup().visible()) {
            UiNavigationTest.Screen screen = controller.get();
            android.widget.LinearLayout container = new android.widget.LinearLayout(screen);
            AdaptiveGridView grid = new AdaptiveGridView(screen);
            android.widget.Button add = new android.widget.Button(screen);
            container.setOrientation(android.widget.LinearLayout.VERTICAL);
            container.addView(grid, new android.widget.LinearLayout.LayoutParams(1000, 600));
            container.addView(add);
            screen.setContentView(container);
            grid.setColumnPolicy(true, false);
            grid.setEndFocusTarget(add);
            grid.setAdapter(new ArrayAdapter<>(screen, android.R.layout.simple_list_item_1, new String[] {"A", "B", "C"}));
            measure(container, 1000, 800);
            assertTrue(grid.requestFocusFromTouch());
            measure(container, 1000, 800);
            grid.setSelection(2);
            measure(container, 1000, 800);
            assertEquals(2, grid.getSelectedItemPosition());
            grid.dispatchKeyEvent(new android.view.KeyEvent(android.view.KeyEvent.ACTION_DOWN, android.view.KeyEvent.KEYCODE_DPAD_RIGHT));
            assertTrue("the button takes focus", add.hasFocus());
        }
    }

    /** A one-column grid of fixed-height cards, to see where the selection ends up after a key. */
    private static AdaptiveGridView gridOf(UiNavigationTest.Screen screen, int viewportHeight, int... cardHeights) {
        FrameLayout container = new FrameLayout(screen);
        AdaptiveGridView grid = new AdaptiveGridView(screen);
        container.addView(grid, new FrameLayout.LayoutParams(1000, viewportHeight));
        screen.setContentView(container);
        grid.setColumnPolicy(true, false);
        grid.setAdapter(new android.widget.BaseAdapter() {
            @Override public int getCount() { return cardHeights.length; }
            @Override public Object getItem(int position) { return position; }
            @Override public long getItemId(int position) { return position; }
            @Override public View getView(int position, View convertView, android.view.ViewGroup parent) {
                View card = convertView != null ? convertView : new View(screen);
                card.setLayoutParams(new android.widget.AbsListView.LayoutParams(
                        android.view.ViewGroup.LayoutParams.MATCH_PARENT, cardHeights[position]));
                return card;
            }
        });
        measure(container, 1000, viewportHeight);
        assertEquals(1, grid.getNumColumns());
        return grid;
    }

    private static void press(AdaptiveGridView grid, int keyCode) {
        grid.onKeyDown(keyCode, new android.view.KeyEvent(android.view.KeyEvent.ACTION_DOWN, keyCode));
        org.robolectric.shadows.ShadowLooper.idleMainLooper();
        measure((View) grid.getParent(), 1000, grid.getHeight());
        org.robolectric.shadows.ShadowLooper.idleMainLooper();
    }

    @Test
    public void aCardSelectedByTheKeysEndsFullyInsideTheGridWithRoomForItsFocusRing() {
        try (var controller = Robolectric.buildActivity(UiNavigationTest.Screen.class).setup().visible()) {
            // 2 dp per px at xhdpi: the grid keeps 8 dp = 16 px around the card.
            AdaptiveGridView grid = gridOf(controller.get(), 500, 300, 300, 300, 300);
            int margin = 16;
            assertTrue(grid.requestFocusFromTouch());
            grid.setSelection(0);
            measure((View) grid.getParent(), 1000, 500);
            press(grid, android.view.KeyEvent.KEYCODE_DPAD_DOWN);
            View selected = grid.getSelectedView();
            assertEquals(1, grid.getSelectedItemPosition());
            assertTrue("bottom " + selected.getBottom() + " + margin <= " + grid.getHeight(),
                    selected.getBottom() + margin <= grid.getHeight());
            press(grid, android.view.KeyEvent.KEYCODE_DPAD_DOWN);
            selected = grid.getSelectedView();
            assertEquals(2, grid.getSelectedItemPosition());
            assertTrue(selected.getBottom() + margin <= grid.getHeight());
            press(grid, android.view.KeyEvent.KEYCODE_DPAD_UP);
            selected = grid.getSelectedView();
            assertEquals(1, grid.getSelectedItemPosition());
            assertTrue("top " + selected.getTop(), selected.getTop() >= margin);
        }
    }

    @Test
    public void aCardTallerThanTheGridAlignsToItsTop() {
        try (var controller = Robolectric.buildActivity(UiNavigationTest.Screen.class).setup().visible()) {
            AdaptiveGridView grid = gridOf(controller.get(), 500, 200, 900);
            assertTrue(grid.requestFocusFromTouch());
            grid.setSelection(0);
            measure((View) grid.getParent(), 1000, 500);
            press(grid, android.view.KeyEvent.KEYCODE_DPAD_DOWN);
            assertEquals(1, grid.getSelectedItemPosition());
            assertEquals("aligned to the top with the focus margin", 16, grid.getSelectedView().getTop());
        }
    }
}
