package com.limelight.ui;

import android.app.Application;
import android.app.Dialog;
import android.graphics.Bitmap;
import android.graphics.Canvas;
import android.os.Looper;
import android.view.View;
import android.view.ViewGroup;
import android.widget.TextView;

import com.limelight.demo.DemoInputWorkspaceActivity;

import org.junit.Assume;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.Shadows;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.GraphicsMode;
import org.robolectric.shadows.ShadowDialog;

import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;

import static org.junit.Assert.*;

/** Optional, native Android view previews. Set NIGHTJAR_PREVIEW_OUTPUT to capture PNGs. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 34, application = Application.class, qualifiers = "w960dp-h600dp-land-mdpi")
@GraphicsMode(GraphicsMode.Mode.NATIVE)
public class DesktopControlsRenderTest {
    @Test public void landscapeModesAndCustomization() throws IOException {
        File directory = outputDirectory();
        try (ActivityController<DemoInputWorkspaceActivity> controller =
                     Robolectric.buildActivity(DemoInputWorkspaceActivity.class).setup()) {
            DemoInputWorkspaceActivity activity = controller.get();
            View root = activity.findViewById(android.R.id.content);
            for (DesktopControlsPanel.Mode mode : DesktopControlsPanel.Mode.values()) {
                activity.controls().setMode(mode);
                assertEquals(mode, activity.controls().getMode());
                capture(root, 960, 600, new File(directory,
                        "native-landscape-" + mode.name().toLowerCase(java.util.Locale.ROOT) + ".png"));
            }
            activity.controls().setMode(DesktopControlsPanel.Mode.KEYBOARD);
            View function = labeled(activity.controls(), "Function");
            assertNotNull(function);
            function.performClick();
            capture(root, 960, 600, new File(directory, "native-landscape-function.png"));
            activity.controls().setMode(DesktopControlsPanel.Mode.COMBINED);
            activity.controls().showCustomization();
            Shadows.shadowOf(Looper.getMainLooper()).idle();
            Dialog dialog = ShadowDialog.getLatestDialog();
            assertNotNull("Customization opens its real dialog", dialog);
            assertTrue(dialog.isShowing());
            capture(dialog.getWindow().getDecorView(), 640, 600,
                    new File(directory, "native-landscape-customize.png"));
            dialog.dismiss();
        }
    }

    @Test
    @Config(qualifiers = "w412dp-h915dp-port-mdpi")
    public void portraitKeyboardAndCombined() throws IOException {
        File directory = outputDirectory();
        try (ActivityController<DemoInputWorkspaceActivity> controller =
                     Robolectric.buildActivity(DemoInputWorkspaceActivity.class).setup()) {
            DemoInputWorkspaceActivity activity = controller.get();
            View root = activity.findViewById(android.R.id.content);
            activity.controls().setMode(DesktopControlsPanel.Mode.KEYBOARD);
            capture(root, 412, 915, new File(directory, "native-portrait-keyboard.png"));
            activity.controls().setMode(DesktopControlsPanel.Mode.COMBINED);
            capture(root, 412, 915, new File(directory, "native-portrait-combined.png"));
        }
    }

    private static File outputDirectory() {
        String output = System.getenv("NIGHTJAR_PREVIEW_OUTPUT");
        Assume.assumeTrue("Native previews are an explicitly requested local capture", output != null && !output.isBlank());
        File directory = new File(output);
        assertTrue(directory.isDirectory() || directory.mkdirs());
        return directory;
    }

    private static View labeled(View view, String text) {
        if (view instanceof TextView && text.contentEquals(((TextView) view).getText())) return view;
        if (view instanceof ViewGroup) {
            ViewGroup group = (ViewGroup) view;
            for (int i = 0; i < group.getChildCount(); i++) {
                View found = labeled(group.getChildAt(i), text);
                if (found != null) return found;
            }
        }
        return null;
    }

    private static void capture(View view, int width, int height, File destination) throws IOException {
        Shadows.shadowOf(Looper.getMainLooper()).idle();
        view.measure(View.MeasureSpec.makeMeasureSpec(width, View.MeasureSpec.EXACTLY),
                View.MeasureSpec.makeMeasureSpec(height, View.MeasureSpec.EXACTLY));
        view.layout(0, 0, width, height);
        assertEquals(width, view.getWidth());
        assertEquals(height, view.getHeight());
        Bitmap bitmap = Bitmap.createBitmap(width, height, Bitmap.Config.ARGB_8888);
        try (FileOutputStream output = new FileOutputStream(destination)) {
            view.draw(new Canvas(bitmap));
            assertTrue(bitmap.compress(Bitmap.CompressFormat.PNG, 100, output));
        } finally {
            bitmap.recycle();
        }
        assertTrue("A nonempty PNG was written", destination.length() > 1000);
    }
}
