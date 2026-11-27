package com.limelight.binding.input.pad;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import android.app.Application;
import android.graphics.Bitmap;
import android.graphics.Canvas;
import android.graphics.Color;

import com.limelight.nvstream.input.ControllerPacket;

import org.junit.Assume;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.GraphicsMode;

import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;

/**
 * The 7.0 skin of the on-screen controller: it reads over white and over black video, lights amber when pressed,
 * draws nothing at opacity 0 and uses no blur, shadow or offscreen layer. Set NIGHTJAR_PREVIEW_OUTPUT to a directory
 * for sheets of every control at rest and pressed, per glyph family, opacity and background.
 */
@RunWith(RobolectricTestRunner.class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = 30, application = Application.class, qualifiers = "xhdpi")
public class PadSkin70Test {
    private static final int WHITE = 0xFFFFFFFF, BLACK = 0xFF000000;
    /** The controls of a sheet's row with their sizes in dp, as the default layout draws them on a phone. */
    private static final PadLayout.Control[] ROW = {PadLayout.Control.LEFT_STICK, PadLayout.Control.DPAD,
            PadLayout.Control.FACE, PadLayout.Control.LB, PadLayout.Control.RT, PadLayout.Control.SELECT,
            PadLayout.Control.HOME, PadLayout.Control.START, PadLayout.Control.L3, PadLayout.Control.TOUCHPAD,
            PadLayout.Control.LEFT_PAD, PadLayout.Control.POINTER_PAD, PadLayout.Control.WASD, PadLayout.Control.DPAD_UP,
            PadLayout.Control.Y, PadLayout.Control.LEFT_GRIP};
    private static final float[][] SIZE_DP = {{150, 150}, {108, 108}, {144, 144}, {90, 44}, {90, 44}, {48, 48}, {48, 48},
            {48, 48}, {48, 48}, {90, 44}, {80, 80}, {108, 68}, {120, 120}, {48, 56}, {54, 54}, {90, 44}};

    private final PadPainter painter = new PadPainter(RuntimeEnvironment.getApplication());
    private final float density = RuntimeEnvironment.getApplication().getResources().getDisplayMetrics().density;

    /** The glyph is the label colour over a dark hairline, so it reads over the brightest and the darkest video. */
    @Test public void aGlyphReadsOverWhiteAndOverBlackVideo() {
        for (PadLayout.Control control : new PadLayout.Control[] {PadLayout.Control.START, PadLayout.Control.LB,
                PadLayout.Control.A, PadLayout.Control.DPAD_UP}) {
            for (int background : new int[] {WHITE, BLACK}) {
                Bitmap pixels = drawn(control, 64, 64, background, new PadPainter.State(), 1);
                float[] range = luminanceRange(pixels, .3f);
                float contrast = (range[1] + .05f) / (range[0] + .05f);
                assertTrue(control + " over " + Integer.toHexString(background) + ": glyph contrast " + contrast, contrast >= 4.5f);
            }
        }
    }

    /**
     * Over white video the glyph also stands against its own body, not only its hairline: the body's fill over white
     * and the label over that body keep 3:1, the floor for large text and icons (it was 1.9:1 at a 35 % fill).
     */
    @Test public void aGlyphStandsAgainstItsBodyOverWhite() {
        Application app = RuntimeEnvironment.getApplication();
        int fill = androidx.core.content.ContextCompat.getColor(app, com.limelight.R.color.nj_pad_skin_fill);
        int label = androidx.core.content.ContextCompat.getColor(app, com.limelight.R.color.nj_pad_skin_label);
        int body = androidx.core.graphics.ColorUtils.compositeColors(fill, WHITE);
        int glyph = androidx.core.graphics.ColorUtils.compositeColors(label, body);
        double contrast = androidx.core.graphics.ColorUtils.calculateContrast(glyph, body);
        assertTrue("glyph against its body over white: " + contrast, contrast >= 3);
    }

    /** A body's edge shows against white video through its dark hairline and against black video through its rim. */
    @Test public void aBodysEdgeShowsOverWhiteAndOverBlackVideo() {
        int size = Math.round(64 * density), middle = size / 2;
        Bitmap white = drawn(PadLayout.Control.START, 64, 64, WHITE, new PadPainter.State(), 1);
        Bitmap black = drawn(PadLayout.Control.START, 64, 64, BLACK, new PadPainter.State(), 1);
        float darkest = 1, lightest = 0;
        // Along the middle row from the box's edge to well inside the rim.
        for (int x = 0; x < Math.round(10 * density); x++) {
            darkest = Math.min(darkest, luminance(white.getPixel(x, middle)));
            lightest = Math.max(lightest, luminance(black.getPixel(x, middle)));
        }
        float overWhite = 1.05f / (darkest + .05f), overBlack = (lightest + .05f) / .05f;
        assertTrue("The hairline against white: " + overWhite, overWhite >= 3f);
        assertTrue("The rim against black: " + overBlack, overBlack >= 4.5f);
    }

    /** Pressed, a part's body and rim are amber at once; at rest nothing of it is. */
    @Test public void aPressLightsBodyAndRimAmber() {
        for (PadLayout.Control control : ROW) {
            if (control.shape == PadLayout.Shape.PAD || control.shape == PadLayout.Shape.TOUCHPAD) continue;
            float[] size = SIZE_DP[java.util.Arrays.asList(ROW).indexOf(control)];
            assertEquals(control + " at rest", 0, amber(drawn(control, size[0], size[1], BLACK, new PadPainter.State(), 1)));
            assertTrue(control + " pressed", amber(drawn(control, size[0], size[1], BLACK, pressed(control), 1)) > 20);
        }
    }

    /** Opacity fades the whole control evenly, and at 0 not one pixel is drawn, pressed or not. */
    @Test public void opacityFadesToNothing() {
        for (PadLayout.Control control : ROW) {
            float[] size = SIZE_DP[java.util.Arrays.asList(ROW).indexOf(control)];
            PadPainter.State held = pressed(control);
            int full = coverage(drawn(control, size[0], size[1], 0, held, 1));
            int half = coverage(drawn(control, size[0], size[1], 0, held, .5f));
            assertTrue(control + " shows", full > 0);
            // Where a rim lies on a body the two add up, so half the opacity leaves a little more than half.
            assertTrue(control + " at 50 %: " + half + " of " + full, half > full * .4f && half < full * .75f);
            assertEquals(control + " at 0 %", 0, coverage(drawn(control, size[0], size[1], 0, held, 0)));
            Bitmap halo = Bitmap.createBitmap(Math.round((size[0] + 60) * density), Math.round((size[1] + 60) * density),
                    Bitmap.Config.ARGB_8888);
            painter.setOpacity(0);
            painter.drawHalo(new Canvas(halo), control, 30 * density, 30 * density, size[0] * density, size[1] * density, held);
            painter.setOpacity(1);
            assertEquals(control + "'s glow at 0 %", 0, coverage(halo));
        }
    }

    /** A stick shows the deadzone it works with as a small ring where the thumb landed, in view once the cap leaves. */
    @Test public void aStickShowsItsDeadzoneWhereTheThumbLanded() {
        PadPainter.State state = new PadPainter.State();
        state.stickActive = true;
        state.stickX = 1;
        int size = Math.round(150 * density);
        Bitmap without = Bitmap.createBitmap(size, size, Bitmap.Config.ARGB_8888);
        painter.drawStickKnob(new Canvas(without), PadLayout.Control.LEFT_STICK, 0, 0, size, size, state);
        state.deadzone = .3f;
        Bitmap with = Bitmap.createBitmap(size, size, Bitmap.Config.ARGB_8888);
        painter.drawStickKnob(new Canvas(with), PadLayout.Control.LEFT_STICK, 0, 0, size, size, state);
        // The ring's right edge, on the middle row: 30 % of the travel from the centre.
        int ring = size / 2 + Math.round(.3f * PadPainter.travel(size));
        assertEquals("No guide without a deadzone", 0, Color.alpha(without.getPixel(ring, size / 2)));
        assertTrue("The guide at the deadzone's edge", Color.alpha(with.getPixel(ring, size / 2)) > 100);
        assertEquals("and nothing inside it", 0, Color.alpha(with.getPixel(size / 2, size / 2)));
    }

    /** Over a stream nothing may blur, cast a shadow layer or draw through an offscreen buffer. */
    @Test public void theDrawPathHasNoBlurShadowOrLayer() throws IOException {
        File sources = new File("src/main/java/com/limelight/binding/input/pad");
        Assume.assumeTrue("Run from the app module", sources.isDirectory());
        for (String name : new String[] {"PadSkin70.java", "PadPainter.java", "PadPainterControls.java",
                "PadPainterHalo.java", "PadControlView.java", "PadPressAnimator.java"}) {
            String source = new String(Files.readAllBytes(new File(sources, name).toPath()), StandardCharsets.UTF_8);
            for (String banned : new String[] {"setShadowLayer", "RenderEffect", "BlurMaskFilter", "saveLayer",
                    "setLayerType", "Shader", "Gradient"}) {
                assertFalse(name + " uses " + banned, source.contains(banned));
            }
        }
    }

    /** Sheets of the skin for review, when asked for: every control, each family, 100, 50 and 0 %, on white and black. */
    @Test public void sheets() throws IOException {
        String output = System.getenv("NIGHTJAR_PREVIEW_OUTPUT");
        Assume.assumeTrue("Sheets are an explicitly requested local capture", output != null && !output.isBlank());
        File directory = new File(output);
        assertTrue(directory.isDirectory() || directory.mkdirs());
        float[] opacities = {1, 1, .5f, .5f, 0};
        boolean[] held = {false, true, false, true, true};
        float rowHeight = 166, gap = 10, width = gap;
        for (float[] size : SIZE_DP) width += size[0] + gap;
        for (PadLayout.Glyph glyph : PadLayout.Glyph.values()) {
            painter.setGlyph(glyph);
            painter.setFacesSwapped(glyph == PadLayout.Glyph.NINTENDO);
            for (int background : new int[] {WHITE, BLACK}) {
                Bitmap sheet = Bitmap.createBitmap(Math.round(width * density), Math.round(rowHeight * opacities.length * density),
                        Bitmap.Config.ARGB_8888);
                Canvas canvas = new Canvas(sheet);
                canvas.drawColor(background);
                for (int row = 0; row < opacities.length; row++) {
                    float x = gap;
                    for (int i = 0; i < ROW.length; i++) {
                        float w = SIZE_DP[i][0] * density, h = SIZE_DP[i][1] * density;
                        float left = x * density, top = (row * rowHeight + (rowHeight - SIZE_DP[i][1]) / 2) * density;
                        PadPainter.State state = held[row] ? pressed(ROW[i]) : rest(ROW[i]);
                        painter.setOpacity(opacities[row]);
                        painter.drawHalo(canvas, ROW[i], left, top, w, h, state);
                        painter.draw(canvas, ROW[i], left, top, w, h, state);
                        painter.setOpacity(1);
                        x += SIZE_DP[i][0] + gap;
                    }
                }
                File file = new File(directory, "skin-" + glyph.name().toLowerCase(java.util.Locale.ROOT)
                        + (background == WHITE ? "-white" : "-black") + ".png");
                try (FileOutputStream stream = new FileOutputStream(file)) {
                    assertTrue(sheet.compress(Bitmap.CompressFormat.PNG, 100, stream));
                }
            }
        }
        painter.setGlyph(PadLayout.Glyph.XBOX);
        painter.setFacesSwapped(false);
    }

    private PadPainter.State rest(PadLayout.Control control) {
        PadPainter.State state = new PadPainter.State();
        state.deadzone = .08f;
        if (control == PadLayout.Control.POINTER_PAD) state.label = painter.pointerLabel(false);
        return state;
    }

    /** {@code control} as a player holds it: a button down, a stick pushed and clicked, a finger on a pad. */
    private PadPainter.State pressed(PadLayout.Control control) {
        PadPainter.State state = rest(control);
        state.pressed = true;
        state.press[0] = 1;
        switch (control.shape) {
            case STICK:
                state.stickActive = true;
                state.stickX = .7f;
                state.stickY = .5f;
                if (control == PadLayout.Control.WASD) {
                    state.press[3] = 1;
                } else {
                    state.flags = PadInput.buttonFlag(control);
                }
                break;
            case DPAD:
                state.flags = ControllerPacket.UP_FLAG | ControllerPacket.RIGHT_FLAG;
                state.press[3] = 1;
                break;
            case FACE:
                state.flags = ControllerPacket.A_FLAG;
                break;
            case PAD:
            case TOUCHPAD:
                state.touchX = 40 * density;
                state.touchY = 30 * density;
                break;
            default:
                break;
        }
        return state;
    }

    private Bitmap drawn(PadLayout.Control control, float widthDp, float heightDp, int background, PadPainter.State state,
                         float opacity) {
        int width = Math.round(widthDp * density), height = Math.round(heightDp * density);
        Bitmap bitmap = Bitmap.createBitmap(width, height, Bitmap.Config.ARGB_8888);
        Canvas canvas = new Canvas(bitmap);
        if (background != 0) canvas.drawColor(background);
        painter.setOpacity(opacity);
        painter.draw(canvas, control, 0, 0, width, height, state);
        painter.setOpacity(1);
        return bitmap;
    }

    /** The darkest and the lightest luminance in the middle {@code share} of the bitmap, where the glyph is. */
    private static float[] luminanceRange(Bitmap pixels, float share) {
        int w = pixels.getWidth(), h = pixels.getHeight();
        float darkest = 1, lightest = 0;
        for (int y = Math.round(h * (1 - share) / 2); y < h * (1 + share) / 2; y++) {
            for (int x = Math.round(w * (1 - share) / 2); x < w * (1 + share) / 2; x++) {
                float luminance = luminance(pixels.getPixel(x, y));
                darkest = Math.min(darkest, luminance);
                lightest = Math.max(lightest, luminance);
            }
        }
        return new float[] {darkest, lightest};
    }

    /** WCAG relative luminance of an opaque colour. */
    private static float luminance(int color) {
        return .2126f * linear(Color.red(color)) + .7152f * linear(Color.green(color)) + .0722f * linear(Color.blue(color));
    }

    private static float linear(int channel) {
        float c = channel / 255f;
        return c <= .03928f ? c / 12.92f : (float) Math.pow((c + .055f) / 1.055f, 2.4);
    }

    /** How many pixels are clearly amber: far more red than blue, as nothing of the skin is at rest. */
    private static int amber(Bitmap pixels) {
        int count = 0;
        for (int y = 0; y < pixels.getHeight(); y++) {
            for (int x = 0; x < pixels.getWidth(); x++) {
                int color = pixels.getPixel(x, y);
                if (Color.red(color) - Color.blue(color) > 60 && Color.red(color) > Color.green(color)) count++;
            }
        }
        return count;
    }

    /** The sum of every pixel's alpha, in units of a fully opaque pixel. */
    private static int coverage(Bitmap pixels) {
        long sum = 0;
        for (int y = 0; y < pixels.getHeight(); y++) {
            for (int x = 0; x < pixels.getWidth(); x++) sum += Color.alpha(pixels.getPixel(x, y));
        }
        return (int) (sum / 255);
    }
}
