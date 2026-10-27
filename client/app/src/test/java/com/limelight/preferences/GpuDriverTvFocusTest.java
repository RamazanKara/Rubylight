package com.limelight.preferences;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertSame;
import static org.junit.Assert.assertTrue;

import android.app.Application;
import android.content.Context;
import android.view.KeyEvent;
import android.view.View;
import android.widget.LinearLayout;
import android.widget.RadioButton;
import android.widget.RadioGroup;

import com.google.android.material.button.MaterialButton;
import com.limelight.R;
import com.limelight.binding.video.GpuDriverStore;
import com.limelight.ui.TvScreensPreviewTest;

import org.junit.After;
import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.GraphicsMode;
import org.robolectric.shadows.ShadowLooper;

import java.io.ByteArrayInputStream;
import java.io.ByteArrayOutputStream;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.List;
import java.util.zip.ZipEntry;
import java.util.zip.ZipOutputStream;

/**
 * The GPU driver page with a TV remote (nightjar#7): every choice is a full-width row whose label is never cut, Up
 * and Down walk the choices and the buttons one by one, and picking a driver keeps focus on it while the page
 * rebuilds instead of dropping it on the toolbar or the first row.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 34, application = Application.class, qualifiers = "w960dp-h540dp-land-television-xhdpi",
        shadows = {com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class, GpuDriverSettingsTest.SupportedDriverStore.class},
        instrumentedPackages = {"com.limelight.nvstream.jni", "com.limelight.binding.video"})
@GraphicsMode(GraphicsMode.Mode.NATIVE)
public class GpuDriverTvFocusTest {
    private static final String LONG_NAME = "Mesa Turnip 25.1.0 for Adreno 6xx, 7xx and 8xx with the A7xx fixes";
    private ActivityController<GpuDriverSettings> controller;

    @Before
    public void installDrivers() throws IOException {
        Context app = RuntimeEnvironment.getApplication();
        install(app, LONG_NAME);
        install(app, "Turnip short");
    }

    @After
    public void tearDown() throws IOException {
        if (controller != null) controller.pause().stop().destroy();
        Context app = RuntimeEnvironment.getApplication();
        for (GpuDriverStore.Driver driver : GpuDriverStore.list(app)) GpuDriverStore.remove(app, driver);
        GpuDriverStore.select(app, null);
    }

    private GpuDriverSettings open() {
        controller = Robolectric.buildActivity(GpuDriverSettings.class).setup().visible();
        GpuDriverSettings screen = controller.get();
        TvScreensPreviewTest.leaveTouchMode(screen);
        TvScreensPreviewTest.settle(screen);
        return screen;
    }

    @Test
    public void choicesSpanTheRowAndTheirLabelsWrapInsteadOfBeingCut() {
        GpuDriverSettings screen = open();
        RadioGroup group = choices(screen);
        assertEquals(3, group.getChildCount());
        for (int i = 0; i < group.getChildCount(); i++) {
            RadioButton radio = (RadioButton) group.getChildAt(i);
            assertEquals(radio.getText() + " spans the row", group.getWidth(), radio.getWidth());
            assertNotNull(radio.getLayout());
            float available = radio.getWidth() - radio.getCompoundPaddingLeft() - radio.getCompoundPaddingRight();
            for (int line = 0; line < radio.getLayout().getLineCount(); line++) {
                assertTrue(radio.getText() + " line " + line + " fits",
                        radio.getLayout().getLineWidth(line) <= available + 0.5f);
            }
            int lines = radio.getLayout().getLineCount();
            int needed = radio.getLayout().getLineTop(lines) + radio.getCompoundPaddingTop() + radio.getCompoundPaddingBottom();
            assertTrue(radio.getText() + " is tall enough for its " + lines + " lines", radio.getHeight() >= needed);
            assertTrue("a remote-sized row", radio.getHeight() >= screen.getResources().getDimensionPixelSize(R.dimen.touch_target));
        }
    }

    @Test
    public void upAndDownWalkEveryChoiceAndButtonOneAtATime() {
        GpuDriverSettings screen = open();
        List<View> stops = stops(screen);
        assertEquals("System, two drivers, Import, Remove", 5, stops.size());
        assertSame("the first focus is the first choice", stops.get(0), focus(screen));
        for (int i = 1; i < stops.size(); i++) {
            TvScreensPreviewTest.key(screen, KeyEvent.KEYCODE_DPAD_DOWN);
            TvScreensPreviewTest.settle(screen);
            assertSame("Down " + i, stops.get(i), focus(screen));
        }
        for (int i = stops.size() - 2; i >= 0; i--) {
            TvScreensPreviewTest.key(screen, KeyEvent.KEYCODE_DPAD_UP);
            TvScreensPreviewTest.settle(screen);
            assertSame("Up to " + i, stops.get(i), focus(screen));
        }
    }

    @Test
    public void pickingADriverKeepsFocusOnItThroughTheRebuild() throws InterruptedException {
        GpuDriverSettings screen = open();
        TvScreensPreviewTest.key(screen, KeyEvent.KEYCODE_DPAD_DOWN);
        TvScreensPreviewTest.key(screen, KeyEvent.KEYCODE_DPAD_DOWN);
        TvScreensPreviewTest.settle(screen);
        View picked = focus(screen);
        CharSequence label = ((RadioButton) picked).getText();
        TvScreensPreviewTest.key(screen, KeyEvent.KEYCODE_DPAD_CENTER);
        waitForWork(screen);
        View now = focus(screen);
        assertTrue("focus stays on a choice: " + now, now instanceof RadioButton);
        assertEquals("on the same choice, rebuilt", label.toString(), ((RadioButton) now).getText().toString());
        assertTrue(((RadioButton) now).isChecked());
        assertEquals(label.toString(), GpuDriverStore.selected(screen).name);
        // And the walk still works from there.
        TvScreensPreviewTest.key(screen, KeyEvent.KEYCODE_DPAD_DOWN);
        TvScreensPreviewTest.settle(screen);
        assertTrue(focus(screen) instanceof MaterialButton);
    }

    /** Waits for the background change: the page disables every choice while it runs and enables them after. */
    private static void waitForWork(GpuDriverSettings screen) throws InterruptedException {
        for (int i = 0; i < 100; i++) {
            TvScreensPreviewTest.settle(screen);
            if (stops(screen).get(0).isEnabled() && !shows(screen, screen.getString(R.string.gpu_driver_working))) return;
            Thread.sleep(20);
        }
        throw new AssertionError("the driver change never finished");
    }

    private static boolean shows(GpuDriverSettings screen, String text) {
        LinearLayout content = screen.findViewById(R.id.gpu_driver_content);
        for (int i = 0; i < content.getChildCount(); i++) {
            View child = content.getChildAt(i);
            if (child instanceof android.widget.TextView && !(child instanceof MaterialButton)
                    && text.contentEquals(((android.widget.TextView) child).getText())) return true;
        }
        return false;
    }

    private static View focus(GpuDriverSettings screen) {
        return screen.getWindow().getDecorView().findFocus();
    }

    private static RadioGroup choices(GpuDriverSettings screen) {
        LinearLayout content = screen.findViewById(R.id.gpu_driver_content);
        for (int i = 0; i < content.getChildCount(); i++) {
            if (content.getChildAt(i) instanceof RadioGroup) return (RadioGroup) content.getChildAt(i);
        }
        throw new AssertionError("no choices");
    }

    /** The page's focus stops from top to bottom: each choice, then each button. */
    private static List<View> stops(GpuDriverSettings screen) {
        List<View> stops = new ArrayList<>();
        RadioGroup group = choices(screen);
        for (int i = 0; i < group.getChildCount(); i++) stops.add(group.getChildAt(i));
        LinearLayout content = screen.findViewById(R.id.gpu_driver_content);
        for (int i = 0; i < content.getChildCount(); i++) {
            if (content.getChildAt(i) instanceof MaterialButton) stops.add(content.getChildAt(i));
        }
        return stops;
    }

    private static void install(Context context, String name) throws IOException {
        String metadata = "{\"schemaVersion\":1,\"name\":\"" + name + "\",\"author\":\"Mesa\",\"packageVersion\":\"1\","
                + "\"vendor\":\"Mesa\",\"driverVersion\":\"25.1.0\",\"minApi\":29,\"libraryName\":\"libvulkan_freedreno.so\"}";
        ByteArrayOutputStream bytes = new ByteArrayOutputStream();
        try (ZipOutputStream zip = new ZipOutputStream(bytes)) {
            zip.putNextEntry(new ZipEntry("meta.json"));
            zip.write(metadata.getBytes(StandardCharsets.UTF_8));
            zip.closeEntry();
            zip.putNextEntry(new ZipEntry("libvulkan_freedreno.so"));
            byte[] elf = new byte[64];
            elf[0] = 0x7f;
            elf[1] = 'E'; elf[2] = 'L'; elf[3] = 'F';
            elf[4] = 2; elf[5] = 1; elf[6] = 1;
            elf[16] = 3; elf[18] = (byte) 183;
            zip.write(elf);
            zip.closeEntry();
        }
        GpuDriverStore.importDriver(context, new ByteArrayInputStream(bytes.toByteArray()));
    }
}
