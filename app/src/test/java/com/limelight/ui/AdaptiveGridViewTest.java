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
}
