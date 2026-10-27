package com.limelight.ui;

import android.app.Activity;
import android.app.Application;
import android.graphics.Bitmap;
import android.graphics.Canvas;
import android.os.Looper;
import android.view.KeyEvent;
import android.view.View;
import android.widget.ListView;

import com.limelight.R;
import com.limelight.preferences.StreamSettings;

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
import org.robolectric.util.ReflectionHelpers;

import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;
import java.util.concurrent.TimeUnit;

import static org.junit.Assert.assertTrue;

/** Optional TV previews at 1920x1080. Set NIGHTJAR_PREVIEW_OUTPUT to capture PNGs. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 34, application = Application.class, qualifiers = "w960dp-h540dp-land-television-xhdpi",
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
@GraphicsMode(GraphicsMode.Mode.NATIVE)
public class TvScreensPreviewTest {
    private static final int W = 1920, H = 1080;

    @Test public void settingsDay() throws IOException {
        settings("day");
    }

    @Test @Config(qualifiers = "+night")
    public void settingsNight() throws IOException {
        settings("night");
    }

    @Test public void screensDay() throws Exception {
        screens("day");
    }

    @Test @Config(qualifiers = "+night")
    public void screensNight() throws Exception {
        screens("night");
    }

    @SuppressWarnings("unchecked")
    private void screens(String name) throws Exception {
        File directory = new File(outputDirectory(), name);
        assertTrue(directory.isDirectory() || directory.mkdirs());
        Class<?>[] screens = {
                com.limelight.console.HomeShellTest.Home.class,
                com.limelight.preferences.AddComputerManually.class,
                com.limelight.HelpTopicsActivity.class,
                com.limelight.preferences.KeyboardShortcutsActivity.class,
                com.limelight.preferences.StreamProfilesActivity.class,
                com.limelight.preferences.ControllerTestActivity.class,
                com.limelight.preferences.ControllerMappingActivity.class,
                com.limelight.pin.PinLockSettingsActivity.class,
                com.limelight.RemoteSetupActivity.class,
                com.limelight.LatencyBenchmarkActivity.class,
                com.limelight.preferences.GpuDriverSettings.class,
        };
        for (Class<?> type : screens) {
            String shot = type.getSimpleName();
            try (ActivityController<? extends Activity> controller =
                         Robolectric.buildActivity((Class<? extends Activity>) type).setup().visible()) {
                Activity screen = controller.get();
                leaveTouchMode(screen);
                settle(screen);
                System.out.println("PREVIEW " + name + " " + shot + " focus=" + describe(screen.getWindow().getDecorView().findFocus()));
                capture(screen, new File(directory, "tv-" + shot + ".png"));
                for (int i = 1; i <= 6; i++) {
                    key(screen, KeyEvent.KEYCODE_DPAD_DOWN);
                    settle(screen);
                    System.out.println("PREVIEW " + name + " " + shot + " down" + i + " focus=" + describe(screen.getWindow().getDecorView().findFocus()));
                    if (i == 2) capture(screen, new File(directory, "tv-" + shot + "-down2.png"));
                }
            } catch (Throwable t) {
                System.out.println("PREVIEW " + name + " " + shot + " FAILED " + t);
            }
        }
    }

    private void settings(String name) throws IOException {
        File directory = new File(outputDirectory(), name);
        assertTrue(directory.isDirectory() || directory.mkdirs());
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup().visible()) {
            StreamSettings screen = controller.get();
            leaveTouchMode(screen);
            settle(screen);
            describe(screen, name + " start");
            capture(screen, new File(directory, "tv-settings-0.png"));
            for (int i = 1; i <= 4; i++) {
                key(screen, KeyEvent.KEYCODE_DPAD_UP);
                settle(screen);
                describe(screen, "up " + i);
                capture(screen, new File(directory, "tv-settings-up" + i + ".png"));
            }
            for (int i = 1; i <= 3; i++) {
                key(screen, KeyEvent.KEYCODE_DPAD_DOWN);
                settle(screen);
                describe(screen, "down " + i);
                capture(screen, new File(directory, "tv-settings-down" + i + ".png"));
            }
            key(screen, KeyEvent.KEYCODE_DPAD_RIGHT);
            settle(screen);
            describe(screen, "right");
            capture(screen, new File(directory, "tv-settings-right.png"));
        }
    }

    private static void describe(StreamSettings screen, String step) {
        ListView list = (ListView) screen.getFragmentManager().findFragmentById(R.id.settings_sections).getView()
                .findViewById(android.R.id.list);
        View focus = screen.getWindow().getDecorView().findFocus();
        System.out.println("PREVIEW " + step + ": selected=" + list.getSelectedItemPosition() + " focus=" + focus
                + " listFocus=" + list.hasFocus());
    }

    private static String describe(View view) {
        if (view == null) return "none";
        String id = view.getId() == View.NO_ID || view.getId() < 0x01000000 ? "" : view.getResources().getResourceEntryName(view.getId());
        String text = view instanceof android.widget.TextView ? " '" + ((android.widget.TextView) view).getText() + "'" : "";
        android.graphics.Rect r = new android.graphics.Rect();
        view.getGlobalVisibleRect(r);
        return view.getClass().getSimpleName() + " " + id + text + " " + r.toShortString();
    }

    public static void leaveTouchMode(Activity screen) {
        Object root = ReflectionHelpers.callInstanceMethod(screen.getWindow().getDecorView(), "getViewRootImpl");
        ReflectionHelpers.callInstanceMethod(root, "ensureTouchMode",
                ReflectionHelpers.ClassParameter.from(boolean.class, false));
    }

    /**
     * Presses and releases a key. A direction no view used moves focus the way the window does (ViewRootImpl's focus
     * navigation, which Robolectric's dispatch skips).
     */
    public static void key(Activity target, int keyCode) {
        boolean handled = target.dispatchKeyEvent(new KeyEvent(KeyEvent.ACTION_DOWN, keyCode));
        int direction = keyCode == KeyEvent.KEYCODE_DPAD_DOWN ? View.FOCUS_DOWN : keyCode == KeyEvent.KEYCODE_DPAD_UP ? View.FOCUS_UP
                : keyCode == KeyEvent.KEYCODE_DPAD_LEFT ? View.FOCUS_LEFT : keyCode == KeyEvent.KEYCODE_DPAD_RIGHT ? View.FOCUS_RIGHT : 0;
        View focused = target.getWindow().getDecorView().findFocus();
        if (!handled && direction != 0 && focused != null) {
            View next = focused.focusSearch(direction);
            if (next != null && next != focused) next.requestFocus(direction);
        }
        target.dispatchKeyEvent(new KeyEvent(KeyEvent.ACTION_UP, keyCode));
    }

    public static void settle(Activity screen) {
        ShadowLooper.idleMainLooper(400, TimeUnit.MILLISECONDS);
        screen.getFragmentManager().executePendingTransactions();
        View root = screen.getWindow().getDecorView();
        root.measure(View.MeasureSpec.makeMeasureSpec(W, View.MeasureSpec.EXACTLY),
                View.MeasureSpec.makeMeasureSpec(H, View.MeasureSpec.EXACTLY));
        root.layout(0, 0, W, H);
        ShadowLooper.idleMainLooper(400, TimeUnit.MILLISECONDS);
    }

    static File outputDirectory() {
        String output = System.getenv("NIGHTJAR_PREVIEW_OUTPUT");
        Assume.assumeTrue("TV previews are an explicitly requested local capture", output != null && !output.isBlank());
        File directory = new File(output);
        assertTrue(directory.isDirectory() || directory.mkdirs());
        return directory;
    }

    static void capture(Activity screen, File destination) throws IOException {
        Shadows.shadowOf(Looper.getMainLooper()).idle();
        View root = screen.getWindow().getDecorView();
        root.measure(View.MeasureSpec.makeMeasureSpec(W, View.MeasureSpec.EXACTLY),
                View.MeasureSpec.makeMeasureSpec(H, View.MeasureSpec.EXACTLY));
        root.layout(0, 0, W, H);
        Bitmap bitmap = Bitmap.createBitmap(W, H, Bitmap.Config.ARGB_8888);
        try (FileOutputStream output = new FileOutputStream(destination)) {
            root.draw(new Canvas(bitmap));
            assertTrue(bitmap.compress(Bitmap.CompressFormat.PNG, 100, output));
        } finally {
            bitmap.recycle();
        }
    }
}
