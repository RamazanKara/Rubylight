package com.limelight.binding.input.pad;

import android.app.Application;
import android.graphics.Bitmap;
import android.graphics.Canvas;
import android.graphics.Rect;
import android.graphics.drawable.GradientDrawable;
import android.os.Looper;
import android.view.View;
import android.view.ViewGroup;
import android.widget.FrameLayout;

import com.limelight.binding.input.HeldInputState;
import com.limelight.preferences.PreferenceConfiguration;
import com.limelight.ui.MicButton;
import com.limelight.ui.UiNavigationTest;

import org.junit.Assume;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.Shadows;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.GraphicsMode;

import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;
import java.util.ArrayList;
import java.util.List;

import static org.junit.Assert.*;

/**
 * Optional previews of the modern controller, its editor and the microphone button over a stand-in
 * stream. Set NIGHTJAR_PREVIEW_OUTPUT to a directory to capture PNGs; every capture also checks that
 * the controls keep apart, stay on the surface and are at least 48 dp to touch.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 34, application = Application.class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
public class ModernControllerPreviewTest {
    private static final HeldInputState.Sink SINK = new HeldInputState.Sink() {
        @Override public void key(int key, boolean down) { }
        @Override public void mouse(int button, boolean down) { }
        @Override public void gamepad(int buttons, short lx, short ly, short rx, short ry, byte lt, byte rt) { }
    };

    @Test @Config(qualifiers = "w800dp-h360dp-land-notnight-xhdpi")
    public void phoneLandscapeLight() throws IOException { capture("phone-land-light"); }

    @Test @Config(qualifiers = "w800dp-h360dp-land-night-xhdpi")
    public void phoneLandscapeDark() throws IOException { capture("phone-land-dark"); }

    @Test @Config(qualifiers = "w360dp-h800dp-port-notnight-xhdpi")
    public void phonePortraitLight() throws IOException { capture("phone-port-light"); }

    @Test @Config(qualifiers = "w360dp-h800dp-port-night-xhdpi")
    public void phonePortraitDark() throws IOException { capture("phone-port-dark"); }

    @Test @Config(qualifiers = "w1280dp-h800dp-land-notnight-mdpi")
    public void tabletLight() throws IOException { capture("tablet-light"); }

    @Test @Config(qualifiers = "w1280dp-h800dp-land-night-mdpi")
    public void tabletDark() throws IOException { capture("tablet-dark"); }

    private static final class FakeMic implements MicButton.Host {
        boolean muted;
        float rowBottom = Float.NaN;
        @Override public boolean offered() { return true; }
        @Override public boolean muted() { return muted; }
        @Override public void setMuted(boolean muted) { this.muted = muted; }
        @Override public float topRowBottom() { return rowBottom; }
    }

    private void capture(String name) throws IOException {
        File directory = outputDirectory();
        RuntimeEnvironment.getApplication().getSharedPreferences("mic_button", 0).edit().clear().commit();
        ModernController.clearLayouts(RuntimeEnvironment.getApplication());
        UiNavigationTest.Screen screen = Robolectric.buildActivity(UiNavigationTest.Screen.class).setup().visible().get();
        android.util.DisplayMetrics metrics = screen.getResources().getDisplayMetrics();
        int width = Math.round(screen.getResources().getConfiguration().screenWidthDp * metrics.density);
        int height = Math.round(screen.getResources().getConfiguration().screenHeightDp * metrics.density);
        FrameLayout frame = new FrameLayout(screen);
        // A stand-in stream: a dark scene that brightens to a pale sky, so the glass is seen on both.
        frame.setBackground(new GradientDrawable(GradientDrawable.Orientation.BR_TL,
                new int[] {0xFF10161F, 0xFF3B4F63, 0xFFB9C9D6}));
        screen.setContentView(frame, new FrameLayout.LayoutParams(width, height));
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        prefs.oscStyle = PreferenceConfiguration.OSC_STYLE_MODERN;
        prefs.oscOpacity = 90;
        prefs.showGuideButton = true;
        ModernController controller = new ModernController(screen, frame, SINK, prefs, "pc/app");
        controller.refreshLayout();
        FakeMic mic = new FakeMic();
        MicButton button = new MicButton(screen, mic);
        frame.addView(button, button.layoutParams());
        button.setSuppressed(false);
        layout(frame, width, height);
        mic.rowBottom = controller.topRowBottom();
        button.place();
        layout(frame, width, height);
        List<String> problems = new ArrayList<>();
        sane(controller, button, frame, metrics.density, problems);
        write(frame, width, height, new File(directory, name + "-1-idle.png"));

        // Held: A on the face cluster, the left stick pushed up and right, RT pulled, the microphone muted.
        PadControlView face = controller.view(PadLayout.Control.FACE);
        float faceSize = Math.min(face.getWidth(), face.getHeight());
        face.down(0, .5f * faceSize, .78f * faceSize, 100);
        PadControlView stick = controller.view(PadLayout.Control.LEFT_STICK);
        float stickSize = Math.min(stick.getWidth(), stick.getHeight());
        stick.down(1, stickSize * .45f, stickSize * .55f, 100);
        stick.move(1, stickSize * .45f + PadPainter.travel(stickSize) * .7f, stickSize * .55f - PadPainter.travel(stickSize) * .7f);
        controller.view(PadLayout.Control.RT).down(2, 5, 5, 100);
        button.performClick();
        assertTrue(mic.muted);
        write(frame, width, height, new File(directory, name + "-2-pressed-muted.png"));
        controller.releaseInput();

        controller.startEditing();
        layout(frame, width, height);
        Shadows.shadowOf(Looper.getMainLooper()).idle();
        layout(frame, width, height);
        PadEditor editor = (PadEditor) frame.getChildAt(frame.getChildCount() - 1);
        touchTargets(editor, metrics.density, problems);
        write(frame, width, height, new File(directory, name + "-3-editor-all.png"));
        editor.select(PadLayout.Control.FACE);
        editor.toggleHidden();
        editor.select(PadLayout.Control.DPAD);
        write(frame, width, height, new File(directory, name + "-4-editor-picked.png"));
        controller.finishEditing(false);
        controller.destroy();
        assertTrue(name + ": " + problems, problems.isEmpty());
    }

    /** Every shown control on the surface, at least 48 dp, and apart from the others and the microphone button. */
    private static void sane(ModernController controller, MicButton button, FrameLayout frame, float density, List<String> problems) {
        List<Rect> boxes = new ArrayList<>();
        List<String> names = new ArrayList<>();
        for (PadLayout.Control control : PadLayout.Control.values()) {
            PadControlView view = controller.view(control);
            if (view == null || view.getVisibility() != View.VISIBLE) continue;
            Rect box = new Rect(view.getLeft(), view.getTop(), view.getRight(), view.getBottom());
            if (box.left < 0 || box.top < 0 || box.right > frame.getWidth() || box.bottom > frame.getHeight()) {
                problems.add(control + " off the surface");
            }
            if (Math.min(box.width(), box.height()) < Math.floor(48 * density)) {
                problems.add(control + " is " + Math.min(box.width(), box.height()) / density + " dp");
            }
            boxes.add(box);
            names.add(control.name());
        }
        Rect mic = new Rect(Math.round(button.getTranslationX()), Math.round(button.getTranslationY()),
                Math.round(button.getTranslationX()) + button.getWidth(), Math.round(button.getTranslationY()) + button.getHeight());
        boxes.add(mic);
        names.add("MIC");
        for (int i = 0; i < boxes.size(); i++) {
            for (int j = i + 1; j < boxes.size(); j++) {
                if (Rect.intersects(boxes.get(i), boxes.get(j))) problems.add(names.get(i) + " overlaps " + names.get(j));
            }
        }
    }

    /** The editor panel's buttons and switch take a 48 dp touch. */
    private static void touchTargets(View view, float density, List<String> problems) {
        if (view.getVisibility() != View.VISIBLE) return;
        if (view.isClickable() && view instanceof android.widget.Button) {
            if (view.getHeight() < Math.floor(48 * density) || view.getWidth() < Math.floor(48 * density)) {
                problems.add(((android.widget.Button) view).getText() + " is " + view.getWidth() / density + "x"
                        + view.getHeight() / density + " dp");
            }
        }
        if (view instanceof ViewGroup) {
            ViewGroup group = (ViewGroup) view;
            for (int i = 0; i < group.getChildCount(); i++) touchTargets(group.getChildAt(i), density, problems);
        }
    }

    private static File outputDirectory() {
        String output = System.getenv("NIGHTJAR_PREVIEW_OUTPUT");
        Assume.assumeTrue("Native previews are an explicitly requested local capture", output != null && !output.isBlank());
        File directory = new File(output);
        assertTrue(directory.isDirectory() || directory.mkdirs());
        return directory;
    }

    private static void layout(View view, int width, int height) {
        Shadows.shadowOf(Looper.getMainLooper()).idle();
        view.measure(View.MeasureSpec.makeMeasureSpec(width, View.MeasureSpec.EXACTLY),
                View.MeasureSpec.makeMeasureSpec(height, View.MeasureSpec.EXACTLY));
        view.layout(0, 0, width, height);
    }

    private static void write(View view, int width, int height, File destination) throws IOException {
        layout(view, width, height);
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
