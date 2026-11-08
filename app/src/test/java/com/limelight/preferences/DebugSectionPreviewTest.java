package com.limelight.preferences;

import android.app.Application;
import android.graphics.Bitmap;
import android.graphics.Canvas;
import android.os.Looper;
import android.view.View;

import org.junit.Assume;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.Shadows;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.GraphicsMode;
import org.robolectric.shadows.ShadowLooper;

import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;
import java.util.concurrent.TimeUnit;

import static org.junit.Assert.assertTrue;

/** Optional phone preview of the Debug section, every row on one tall page. Set NIGHTJAR_PREVIEW_OUTPUT to capture PNGs. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 34, application = Application.class, qualifiers = "w411dp-h1800dp-port-xhdpi",
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
@GraphicsMode(GraphicsMode.Mode.NATIVE)
public class DebugSectionPreviewTest {
    private static final int W = 822, H = 3600;

    @Test public void debugDay() throws IOException {
        capture("day");
    }

    @Test @Config(qualifiers = "+night")
    public void debugNight() throws IOException {
        capture("night");
    }

    private void capture(String name) throws IOException {
        String output = System.getenv("NIGHTJAR_PREVIEW_OUTPUT");
        Assume.assumeTrue("previews are an explicitly requested local capture", output != null && !output.isBlank());
        File directory = new File(output);
        assertTrue(directory.isDirectory() || directory.mkdirs());
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup().visible()) {
            StreamSettings screen = controller.get();
            screen.showSection("debug");
            ShadowLooper.idleMainLooper(400, TimeUnit.MILLISECONDS);
            screen.getFragmentManager().executePendingTransactions();
            Shadows.shadowOf(Looper.getMainLooper()).idle();
            View root = screen.getWindow().getDecorView();
            for (int pass = 0; pass < 2; pass++) {
                root.measure(View.MeasureSpec.makeMeasureSpec(W, View.MeasureSpec.EXACTLY),
                        View.MeasureSpec.makeMeasureSpec(H, View.MeasureSpec.EXACTLY));
                root.layout(0, 0, W, H);
                ShadowLooper.idleMainLooper(400, TimeUnit.MILLISECONDS);
            }
            Bitmap bitmap = Bitmap.createBitmap(W, H, Bitmap.Config.ARGB_8888);
            try (FileOutputStream out = new FileOutputStream(new File(directory, "android-debug-" + name + ".png"))) {
                root.draw(new Canvas(bitmap));
                assertTrue(bitmap.compress(Bitmap.CompressFormat.PNG, 100, out));
            } finally {
                bitmap.recycle();
            }
        }
    }
}
