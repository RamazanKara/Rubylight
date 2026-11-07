package com.limelight.ui;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotEquals;
import static org.junit.Assert.assertTrue;

import android.app.Application;
import android.app.Dialog;
import android.content.Context;
import android.os.Looper;
import android.view.KeyEvent;
import android.view.LayoutInflater;
import android.view.View;
import android.view.ViewGroup;
import android.widget.EditText;
import android.widget.SeekBar;

import androidx.appcompat.view.ContextThemeWrapper;

import com.google.android.material.slider.LabelFormatter;
import com.google.android.material.slider.Slider;
import com.limelight.R;
import com.limelight.demo.DemoInputWorkspaceActivity;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.Shadows;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowDialog;

import java.io.File;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.util.ArrayList;
import java.util.List;

/** nightjar#61: the thin platform SeekBars are the thick Material slider of the classic controls' opacity. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 34, application = Application.class)
public class StepSliderTest {
    private static Context themed() {
        return new ContextThemeWrapper(RuntimeEnvironment.getApplication(), R.style.AppTheme);
    }

    @Test
    public void keysStepLikeThePlatformSeekBarDid() {
        Context context = themed();
        for (int range : new int[] {1, 4, 5, 10, 20, 21, 29, 30, 40, 99, 100, 105, 125, 340, 375, 1000, 1999500}) {
            SeekBar seekBar = new SeekBar(context);
            seekBar.setMax(range);
            assertEquals("range " + range, seekBar.getKeyProgressIncrement(), StepSlider.seekBarKeyStep(range));
            StepSlider slider = new StepSlider(context);
            slider.setMax(range);
            assertEquals(seekBar.getKeyProgressIncrement(), slider.getKeyStep());
        }
    }

    @Test
    public void leftAndRightStepAndAnEndLetsTheFocusMoveOn() {
        StepSlider slider = new StepSlider(themed());
        slider.setMax(40);
        slider.setKeyStep(3);
        slider.setPosition(37);
        List<Integer> picked = new ArrayList<>();
        slider.setOnPositionChangeListener((changed, position) -> picked.add(position));

        assertTrue(slider.onKeyDown(KeyEvent.KEYCODE_DPAD_RIGHT, key(KeyEvent.KEYCODE_DPAD_RIGHT)));
        assertEquals(40, slider.getPosition());
        assertFalse("at the end the window moves the focus",
                slider.onKeyDown(KeyEvent.KEYCODE_DPAD_RIGHT, key(KeyEvent.KEYCODE_DPAD_RIGHT)));
        assertTrue(slider.onKeyDown(KeyEvent.KEYCODE_DPAD_LEFT, key(KeyEvent.KEYCODE_DPAD_LEFT)));
        assertEquals(37, slider.getPosition());
        assertEquals(List.of(40, 37), picked);

        slider.setPosition(0);
        assertFalse(slider.onKeyDown(KeyEvent.KEYCODE_DPAD_LEFT, key(KeyEvent.KEYCODE_DPAD_LEFT)));
        assertEquals("moving it in code tells nobody", List.of(40, 37), picked);
        assertFalse(slider.onKeyDown(KeyEvent.KEYCODE_DPAD_UP, key(KeyEvent.KEYCODE_DPAD_UP)));
    }

    @Test
    public void itLooksLikeTheClassicControlsOpacitySlider() {
        Context context = new ContextThemeWrapper(RuntimeEnvironment.getApplication(), R.style.StreamTheme);
        Slider reference = LayoutInflater.from(context).inflate(R.layout.osc_opacity_panel, null)
                .findViewById(R.id.osc_opacity_slider);
        StepSlider slider = new StepSlider(context);
        assertTrue(slider instanceof Slider);
        assertEquals(LabelFormatter.LABEL_FLOATING, slider.getLabelBehavior());
        assertEquals(reference.getLabelBehavior(), slider.getLabelBehavior());
        assertEquals(reference.isTickVisible(), slider.isTickVisible());
        assertEquals(reference.getTrackHeight(), slider.getTrackHeight());
        assertEquals(reference.getThumbWidth(), slider.getThumbWidth());
        assertEquals(reference.getThumbHeight(), slider.getThumbHeight());
        assertEquals(reference.getThumbTintList().getDefaultColor(), slider.getThumbTintList().getDefaultColor());
        assertEquals(reference.getTrackActiveTintList().getDefaultColor(),
                slider.getTrackActiveTintList().getDefaultColor());
        assertEquals(reference.getTrackInactiveTintList().getDefaultColor(),
                slider.getTrackInactiveTintList().getDefaultColor());
        assertNotEquals("a focused slider shows where it is", 0, slider.getHaloTintList().getDefaultColor() >>> 24);
    }

    @Test
    public void theBitrateSliderKeepsItsScaleAndSteps() {
        Context context = new ContextThemeWrapper(RuntimeEnvironment.getApplication(), R.style.StreamTheme);
        EditText field = new EditText(context);
        StepSlider bar = BitrateSlider.create(context, field, 20_000, () -> { });
        assertTrue(bar instanceof Slider);
        assertEquals(BitrateSlider.MAX_PROGRESS, bar.getMax());
        assertEquals(BitrateSlider.progress(20_000), bar.getPosition());
        assertEquals(1, bar.getKeyStep());
        bar.onKeyDown(KeyEvent.KEYCODE_DPAD_RIGHT, key(KeyEvent.KEYCODE_DPAD_RIGHT));
        assertEquals("one step above 20 Mbps", "22", field.getText().toString());
        assertEquals("its value floats in the menu's words", "22 Mbps", label(bar, bar.getValue()));
    }

    @Test
    public void aKeyStepShowsTheValueOverTheThumbForAWhile() {
        try (ActivityController<android.app.Activity> controller =
                     Robolectric.buildActivity(android.app.Activity.class).setup()) {
            StepSlider slider = new StepSlider(themed());
            controller.get().setContentView(slider);
            slider.setMax(100);
            slider.setPositionText(position -> position + " %");
            assertFalse(slider.showsValue());

            assertTrue(slider.onKeyDown(KeyEvent.KEYCODE_DPAD_RIGHT, key(KeyEvent.KEYCODE_DPAD_RIGHT)));
            assertTrue("a remote's step shows the value", slider.showsValue());
            assertEquals("5 %", label(slider, slider.getValue()));
            Shadows.shadowOf(Looper.getMainLooper()).idleFor(java.time.Duration.ofMillis(StepSlider.KEY_VALUE_MS - 100));
            assertTrue(slider.onKeyDown(KeyEvent.KEYCODE_DPAD_RIGHT, key(KeyEvent.KEYCODE_DPAD_RIGHT)));
            Shadows.shadowOf(Looper.getMainLooper()).idleFor(java.time.Duration.ofMillis(StepSlider.KEY_VALUE_MS - 100));
            assertTrue("each step keeps it up", slider.showsValue());
            Shadows.shadowOf(Looper.getMainLooper()).idleFor(java.time.Duration.ofMillis(200));
            assertFalse("then it goes", slider.showsValue());
        }
    }

    @Test
    public void everyMaterialSliderShowsItsValueWhileItMoves() throws Exception {
        List<File> sources = new ArrayList<>();
        collectSources(new File("src/main"), sources);
        int sliders = 0;
        for (File file : sources) {
            String source = new String(Files.readAllBytes(file.toPath()), StandardCharsets.UTF_8);
            if (!file.getName().equals("StepSlider.java")) assertFalse(file.getPath(), source.contains("LABEL_GONE"));
            assertFalse(file.getPath(), source.contains("labelBehavior=\"gone\""));
            if (source.contains("<com.google.android.material.slider.Slider")) {
                sliders++;
                assertTrue(file.getPath(), source.contains("labelBehavior=\"floating\""));
            }
            if (source.contains("new Slider(")) {
                sliders++;
                assertTrue("a code-built slider gets its value text: " + file.getPath(), source.contains("StepSlider.showValue("));
            }
        }
        assertTrue("the opacity panels and the code-built sliders were found", sliders >= 4);
    }

    @Test
    public void theDesktopControlsSlidersKeepTheirRanges() {
        try (ActivityController<DemoInputWorkspaceActivity> controller =
                     Robolectric.buildActivity(DemoInputWorkspaceActivity.class).setup()) {
            DesktopControlsPanel controls = controller.get().controls();
            controls.showCustomization();
            Shadows.shadowOf(Looper.getMainLooper()).idle();
            Dialog dialog = ShadowDialog.getLatestDialog();
            List<StepSlider> sliders = new ArrayList<>();
            collect(dialog.getWindow().getDecorView(), sliders);
            // Key height 36-56 dp, panel height 220-560 dp, opacity 50-100 %, pointer speed 25-400 %.
            assertEquals(4, sliders.size());
            int[] ranges = {20, 340, 50, 375};
            for (int i = 0; i < ranges.length; i++) {
                StepSlider slider = sliders.get(i);
                assertEquals(ranges[i], slider.getMax());
                assertEquals(StepSlider.seekBarKeyStep(ranges[i]), slider.getKeyStep());
            }
            StepSlider opacity = sliders.get(2);
            CharSequence before = opacity.getContentDescription();
            opacity.setPosition(10);
            assertTrue(opacity.onKeyDown(KeyEvent.KEYCODE_DPAD_RIGHT, key(KeyEvent.KEYCODE_DPAD_RIGHT)));
            assertEquals(10 + opacity.getKeyStep(), opacity.getPosition());
            assertNotEquals("the label follows the value", before, opacity.getContentDescription());
            assertTrue(opacity.getContentDescription().toString().contains(String.valueOf(60 + opacity.getKeyStep())));
            assertEquals("its value floats with its unit", (60 + opacity.getKeyStep()) + "%", label(opacity, opacity.getValue()));
            assertEquals("40 dp", DesktopControlsPanel.valueOf("Key height: 40 dp", 40));
            assertEquals("a title without the number still gives it", "40", DesktopControlsPanel.valueOf("Key height", 40));
            dialog.dismiss();
        }
    }

    @Test
    public void noPlatformSeekBarIsLeft() throws Exception {
        for (String path : new String[] {
                "src/main/java/com/limelight/preferences/SeekBarPreference.java",
                "src/main/java/com/limelight/ui/BitrateSlider.java",
                "src/main/java/com/limelight/ui/DesktopControlsPanel.java"}) {
            String source = new String(Files.readAllBytes(new File(path).toPath()), StandardCharsets.UTF_8);
            assertFalse(path, source.contains("android.widget.SeekBar"));
            assertFalse(path, source.contains("new SeekBar("));
            assertTrue(path, source.contains("new StepSlider("));
        }
    }

    private static void collectSources(File dir, List<File> into) {
        File[] files = dir.listFiles();
        if (files == null) return;
        for (File file : files) {
            if (file.isDirectory()) collectSources(file, into);
            else if (file.getName().endsWith(".java") || file.getName().endsWith(".xml")) into.add(file);
        }
    }

    /** The text a slider floats over its thumb and TalkBack reads at {@code value}. */
    static String label(Slider slider, float value) {
        try {
            // BaseSlider, which keeps it, is not public.
            java.lang.reflect.Field field = Slider.class.getSuperclass().getDeclaredField("formatter");
            field.setAccessible(true);
            return ((LabelFormatter) field.get(slider)).getFormattedValue(value);
        } catch (ReflectiveOperationException e) {
            throw new AssertionError(e);
        }
    }

    private static void collect(View view, List<StepSlider> into) {
        if (view instanceof StepSlider) into.add((StepSlider) view);
        if (view instanceof ViewGroup) {
            ViewGroup group = (ViewGroup) view;
            for (int i = 0; i < group.getChildCount(); i++) collect(group.getChildAt(i), into);
        }
    }

    private static KeyEvent key(int keyCode) {
        return new KeyEvent(KeyEvent.ACTION_DOWN, keyCode);
    }
}
