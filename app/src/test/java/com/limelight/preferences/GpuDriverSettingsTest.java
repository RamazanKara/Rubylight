package com.limelight.preferences;

import android.app.Application;
import android.content.Intent;
import android.os.Looper;
import android.view.View;
import android.widget.LinearLayout;

import androidx.core.graphics.Insets;
import androidx.core.view.ViewCompat;
import androidx.core.view.WindowInsetsCompat;

import com.google.android.material.appbar.MaterialToolbar;
import com.google.android.material.button.MaterialButton;
import com.limelight.R;
import com.limelight.binding.video.GpuDriverStore;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.Implementation;
import org.robolectric.annotation.Implements;
import org.robolectric.shadows.ShadowToast;

import static org.junit.Assert.*;
import static org.robolectric.Shadows.shadowOf;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 30, application = Application.class,
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class GpuDriverSettingsTest {
    @Implements(value = GpuDriverStore.class, isInAndroidSdk = false)
    public static class SupportedDriverStore {
        @Implementation
        protected static boolean isSupported() {
            return true;
        }
    }

    @Test
    @Config(shadows = {com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class, SupportedDriverStore.class},
            instrumentedPackages = {"com.limelight.nvstream.jni", "com.limelight.binding.video"})
    public void missingDocumentPickerKeepsSettingsOpenAndExplainsHowToImport() {
        var controller = Robolectric.buildActivity(GpuDriverSettings.class).setup();
        shadowOf(RuntimeEnvironment.getApplication()).checkActivities(true);
        try {
            MaterialButton button = importButton(controller.get());
            assertTrue(button.isEnabled());
            button.performClick();
            assertFalse(controller.get().isFinishing());
            assertEquals(controller.get().getString(R.string.gpu_driver_no_picker), ShadowToast.getTextOfLatestToast());
            assertNull(shadowOf(controller.get()).getNextStartedActivityForResult());
        } finally {
            shadowOf(RuntimeEnvironment.getApplication()).checkActivities(false);
            controller.pause().stop().destroy();
        }
    }

    @Test
    @Config(shadows = {com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class, SupportedDriverStore.class},
            instrumentedPackages = {"com.limelight.nvstream.jni", "com.limelight.binding.video"})
    public void importOpensTheZipDocumentPickerWhenAvailable() {
        var controller = Robolectric.buildActivity(GpuDriverSettings.class).setup();
        try {
            importButton(controller.get()).performClick();
            Intent picker = shadowOf(controller.get()).getNextStartedActivityForResult().intent;
            assertEquals(Intent.ACTION_OPEN_DOCUMENT, picker.getAction());
            assertTrue(picker.hasCategory(Intent.CATEGORY_OPENABLE));
            assertEquals("application/zip", picker.getType());
            assertArrayEquals(new String[] {"application/zip", "application/x-zip-compressed", "application/octet-stream"},
                    picker.getStringArrayExtra(Intent.EXTRA_MIME_TYPES));
        } finally {
            controller.pause().stop().destroy();
        }
    }

    private static MaterialButton importButton(GpuDriverSettings screen) {
        LinearLayout content = screen.findViewById(R.id.gpu_driver_content);
        for (int i = 0; i < content.getChildCount(); i++) {
            View child = content.getChildAt(i);
            if (child instanceof MaterialButton && screen.getString(R.string.gpu_driver_import)
                    .contentEquals(((MaterialButton) child).getText())) return (MaterialButton) child;
        }
        throw new AssertionError("Import driver button is missing");
    }

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
