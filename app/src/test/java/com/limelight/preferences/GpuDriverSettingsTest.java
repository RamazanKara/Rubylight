package com.limelight.preferences;

import android.app.Application;
import android.os.Looper;
import android.view.View;

import androidx.core.graphics.Insets;
import androidx.core.view.ViewCompat;
import androidx.core.view.WindowInsetsCompat;

import com.google.android.material.appbar.MaterialToolbar;
import com.limelight.R;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;

import static org.junit.Assert.*;
import static org.robolectric.Shadows.shadowOf;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 30, application = Application.class,
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class GpuDriverSettingsTest {
    @Test
    public void lateInsetsRelayoutToolbarAndContentAcrossRotation() {
        var controller = Robolectric.buildActivity(GpuDriverSettings.class).setup();
        try {
            View root = controller.get().findViewById(android.R.id.content);
            View content = controller.get().findViewById(R.id.gpu_driver_root);
            MaterialToolbar toolbar = controller.get().findViewById(R.id.gpu_driver_toolbar);
            for (int[] bounds : new int[][] {{480, 900, 0, 30, 0, 40}, {900, 480, 48, 24, 40, 0}}) {
                root.layout(0, 0, 0, 0);
                root.setPadding(0, 0, 0, 0);
                ViewCompat.dispatchApplyWindowInsets(root, new WindowInsetsCompat.Builder()
                        .setInsets(WindowInsetsCompat.Type.systemBars(), Insets.of(0, bounds[3], bounds[4], bounds[5]))
                        .setInsets(WindowInsetsCompat.Type.displayCutout(), Insets.of(bounds[2], 0, 0, 0)).build());
                layout(root, bounds[0], bounds[1]);
                shadowOf(Looper.getMainLooper()).idle();
                assertTrue("Late padding must request another layout", content.isLayoutRequested() ||
                        (content.getLeft() == bounds[2] && content.getTop() == bounds[3]));
                layout(root, bounds[0], bounds[1]);
                assertEquals(bounds[2], content.getLeft());
                assertEquals(bounds[3], content.getTop());
                assertEquals(bounds[0] - bounds[4], content.getRight());
                assertEquals(bounds[1] - bounds[5], content.getBottom());
                assertEquals(0, toolbar.getTop());
                assertEquals(content.getWidth(), toolbar.getWidth());
            }
            View navigation = null;
            for (int i = 0; i < toolbar.getChildCount(); i++) {
                View child = toolbar.getChildAt(i);
                if (controller.get().getString(R.string.settings_navigate_up).contentEquals(
                        child.getContentDescription() == null ? "" : child.getContentDescription())) navigation = child;
            }
            assertNotNull(navigation);
            navigation.performClick();
            assertTrue(controller.get().isFinishing());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    private static void layout(View view, int width, int height) {
        view.measure(View.MeasureSpec.makeMeasureSpec(width, View.MeasureSpec.EXACTLY),
                View.MeasureSpec.makeMeasureSpec(height, View.MeasureSpec.EXACTLY));
        view.layout(0, 0, width, height);
    }
}
