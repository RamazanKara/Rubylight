package com.limelight.binding.input.pad;

import android.app.Application;
import android.content.Context;
import android.graphics.Bitmap;
import android.graphics.Canvas;
import android.graphics.Color;
import android.view.View;

import com.limelight.nvstream.input.ControllerPacket;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.GraphicsMode;
import org.robolectric.shadows.ShadowLooper;

import static org.junit.Assert.*;

/**
 * {@link ModernController} layout persistence, orientation/classic migration, top-row placement,
 * and press-halo/opacity rendering tests.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 30, application = Application.class, qualifiers = "w800dp-h360dp-land-xhdpi")
public class ModernControllerLayoutAndVisualsTest extends ModernControllerHarness {
    @Test public void aLayoutSavedForOneGameStaysWithThatGame() {
        ModernController first = controller(GAME);
        PadLayout draft = first.layout().copy();
        draft.setScale(PadLayout.Control.FACE, 1.5f);
        first.save(draft, true);
        first.destroy();
        assertEquals(1.5f, controller(GAME).layout().spec(PadLayout.Control.FACE).scale(), 1e-3);
        assertTrue("Another game keeps the shared layout", controller("pc-1/app-8").layout().isDefault());
    }

    @Test public void savingForAllGamesDropsThisGamesOwnLayout() {
        ModernController controller = controller(GAME);
        PadLayout own = controller.layout().copy();
        own.setScale(PadLayout.Control.FACE, 1.5f);
        controller.save(own, true);
        PadLayout shared = controller.layout().copy();
        shared.setScale(PadLayout.Control.FACE, .7f);
        controller.save(shared, false);
        controller.destroy();
        assertEquals(.7f, controller(GAME).layout().spec(PadLayout.Control.FACE).scale(), 1e-3);
        assertEquals(.7f, controller("pc-1/app-8").layout().spec(PadLayout.Control.FACE).scale(), 1e-3);
        assertFalse(RuntimeEnvironment.getApplication().getSharedPreferences(ModernController.PREFS, Context.MODE_PRIVATE)
                .contains(PadLayoutKeys.key(true, GAME)));
    }

    @Test public void aStreamWithoutAGameSavesOnlyTheSharedLayout() {
        ModernController controller = controller(null);
        PadLayout draft = controller.layout().copy();
        draft.spec(PadLayout.Control.LB).hidden = true;
        controller.save(draft, true);
        assertTrue(controller("any").layout().spec(PadLayout.Control.LB).hidden);
    }

    @Test public void theTopRowLeavesRoomForTheMicrophoneButton() {
        ModernController controller = controller(GAME);
        float bottom = controller.topRowBottom();
        float density = frame.getResources().getDisplayMetrics().density;
        // The Touchpad sits below View, Home and Menu, so the microphone goes below it.
        float[] box = controller.layout().bounds(PadLayout.Control.TOUCHPAD, 1600, 720, density, new float[4]);
        assertEquals(box[1] + box[3], bottom, 1);
        float[] menu = controller.layout().bounds(PadLayout.Control.START, 1600, 720, density, new float[4]);
        assertTrue(menu[1] + menu[3] < box[1]);
        // The microphone's default place clears the Touchpad pill, in landscape and in portrait.
        float size = com.limelight.ui.MicButton.SIZE_DP * density;
        float[] mic = com.limelight.ui.MicButton.Placement.center(Float.NaN, Float.NaN, 1600, 720, size,
                12 * density, bottom);
        assertTrue(mic[1] - size / 2 >= box[1] + box[3]);
        float[] portrait = controller.layout().bounds(PadLayout.Control.TOUCHPAD, 720, 1600, density, new float[4]);
        assertTrue("Counted as the top row", portrait[1] <= 1600 / 3f);
        mic = com.limelight.ui.MicButton.Placement.center(Float.NaN, Float.NaN, 720, 1600, size,
                12 * density, portrait[1] + portrait[3]);
        assertTrue(mic[1] - size / 2 >= portrait[1] + portrait[3]);
        controller.hide();
        assertTrue(Float.isNaN(controller.topRowBottom()));
    }

    /** A layout edited in portrait is stored for portrait; landscape keeps its own. */
    @Test @Config(qualifiers = "w360dp-h800dp-port-xhdpi")
    public void portraitAndLandscapeKeepTheirOwnLayouts() {
        frame.getLayoutParams().width = 720;
        frame.getLayoutParams().height = 1600;
        ModernController controller = new ModernController(frame.getContext(), frame, sink, prefs, GAME);
        controller.refreshLayout();
        controller.show();
        frame.measure(View.MeasureSpec.makeMeasureSpec(720, View.MeasureSpec.EXACTLY),
                View.MeasureSpec.makeMeasureSpec(1600, View.MeasureSpec.EXACTLY));
        frame.layout(0, 0, 720, 1600);
        PadLayout draft = controller.layout().copy();
        draft.setScale(PadLayout.Control.FACE, 1.4f);
        controller.save(draft, false);
        android.content.SharedPreferences store = RuntimeEnvironment.getApplication()
                .getSharedPreferences(ModernController.PREFS, Context.MODE_PRIVATE);
        assertTrue("Saved for portrait", store.contains(PadLayoutKeys.key(false, null)));
        assertFalse("Not for landscape", store.contains(PadLayoutKeys.key(true, null)));
        controller.destroy();
    }

    /** A layout saved by the version before separate orientations loads in both. */
    @Test public void anOlderSharedLayoutStillLoads() {
        RuntimeEnvironment.getApplication().getSharedPreferences(ModernController.PREFS, Context.MODE_PRIVATE)
                .edit().putString(PadLayoutKeys.legacyKey(null), "v1|FACE,0.7083,0.5556,1.300,100,0").commit();
        ModernController controller = controller(GAME);
        assertEquals(.7083f, controller.layout().spec(PadLayout.Control.FACE).cx, 1e-4);
        assertEquals(1.3f, controller.layout().spec(PadLayout.Control.FACE).scale(), 1e-3);
    }

    /** The first modern stream after a classic one keeps the classic controls' places. */
    @Test public void aClassicLayoutIsCarriedOverOnce() {
        RuntimeEnvironment.getApplication().getSharedPreferences(
                        ClassicLayoutMigration.CLASSIC_PREFS,
                        Context.MODE_PRIVATE)
                .edit().putString("13", "{\"LEFT\":1200,\"TOP\":400,\"WIDTH\":200,\"HEIGHT\":200}").commit();
        // A stream that never saw the modern controller: the one-time carry-over is still due.
        RuntimeEnvironment.getApplication().getSharedPreferences(ModernController.PREFS, Context.MODE_PRIVATE)
                .edit().remove("classic-migrated").commit();
        ModernController controller = controller(GAME);
        PadLayout.Spec stick = controller.layout().spec(PadLayout.Control.RIGHT_STICK);
        assertTrue("The classic right stick's place is carried over", stick.placed());
        assertEquals(1300f / 1600f, stick.cx, .01f);
        assertTrue(RuntimeEnvironment.getApplication().getSharedPreferences(ModernController.PREFS, Context.MODE_PRIVATE)
                .contains(PadLayoutKeys.legacyKey(null)));
        controller.destroy();
        // The carried layout is used again, not re-migrated from whatever the classic controller holds later.
        assertTrue(controller(GAME).layout().spec(PadLayout.Control.RIGHT_STICK).placed());
    }

    /**
     * Opacity 0 is a player's choice of controls nobody sees: a press still plays but shows nothing, neither the
     * control, a stick's knob, the touchpad's finger nor the pressed amber. Above 0 a faint control still answers.
     */
    @Test public void atOpacityZeroAPressStaysInvisibleAndAboveItAFaintControlStillAnswers() {
        prefs.oscOpacity = 0;
        ModernController controller = pointerController(false);
        PadControlView button = controller.view(PadLayout.Control.LB);
        PadControlView face = controller.view(PadLayout.Control.FACE);
        PadControlView stick = controller.view(PadLayout.Control.LEFT_STICK);
        PadControlView pad = controller.view(PadLayout.Control.POINTER_PAD);
        for (PadControlView view : new PadControlView[] {button, face, stick, pad}) {
            assertEquals(view.control + " at rest", 0, view.getAlpha(), 0);
        }
        button.down(0, 5, 5, 100);
        face.down(1, face.getWidth() / 2f, face.getHeight() * .78f, 100);
        float size = Math.min(stick.getWidth(), stick.getHeight());
        stick.down(2, stick.getWidth() / 2f, stick.getHeight() / 2f, 100);
        stick.move(2, stick.getWidth() / 2f + PadPainter.travel(size), stick.getHeight() / 2f);
        pad.down(3, pad.getWidth() / 2f, pad.getHeight() / 2f, 100);
        pad.move(3, pad.getWidth() / 2f + 20, pad.getHeight() / 2f, 200_000_000L);
        assertEquals("Pressed, the buttons still play", ControllerPacket.LB_FLAG | ControllerPacket.A_FLAG,
                last()[0] & (ControllerPacket.LB_FLAG | ControllerPacket.A_FLAG));
        assertTrue("and so does the stick", last()[1] > 0);
        assertFalse("The touchpad follows its finger", Float.isNaN(pad.state().touchX));
        for (PadControlView view : new PadControlView[] {button, face, stick, pad}) {
            assertEquals(view.control + " pressed", 0, view.getAlpha(), 0);
        }
        assertEquals("The knob layer too", 0, stick.knobLayer().getAlpha(), 0);
        controller.releaseInput();

        prefs.oscOpacity = 20;
        controller.place();
        assertEquals(.2f * PadLayout.MAX_OPACITY / 100f, button.getAlpha(), 1e-4);
        button.down(0, 5, 5, 100);
        stick.down(2, stick.getWidth() / 2f, stick.getHeight() / 2f, 100);
        assertEquals("A press raises a faint control", PadControlView.PRESSED_ALPHA, button.getAlpha(), 1e-4);
        assertEquals(PadControlView.PRESSED_ALPHA, stick.knobLayer().getAlpha(), 1e-4);
        button.up(0, 200);
        assertEquals("Let go, it eases back", PadControlView.PRESSED_ALPHA, button.getAlpha(), 1e-4);
        ShadowLooper.idleMainLooper(PadPainter.RELEASE_MS + 40, java.util.concurrent.TimeUnit.MILLISECONDS);
        assertEquals("and is faint again", .2f, button.getAlpha(), 1e-4);
        controller.destroy();
    }

    /** H3: this game's own layout, in the old single key, keeps winning after the player saves one for all games. */
    @Test public void aGamesOwnLayoutWinsOverASharedOneSavedLater() {
        android.content.SharedPreferences store = RuntimeEnvironment.getApplication()
                .getSharedPreferences(ModernController.PREFS, Context.MODE_PRIVATE);
        store.edit()
                .putString(PadLayoutKeys.legacyKey(GAME), "v1|FACE,0.7083,0.5556,1.300,100,0")
                .putString(PadLayoutKeys.key(true, null), "v1|FACE,0.3000,0.4000,0.700,100,0")
                .commit();
        ModernController controller = controller(GAME);
        assertEquals("The game's own layout wins", 1.3f,
                controller.layout().spec(PadLayout.Control.FACE).scale(), 1e-3);
        assertEquals(.7083f, controller.layout().spec(PadLayout.Control.FACE).cx, 1e-4);
        assertEquals("A game without its own layout gets the shared one", .7f,
                controller("pc-1/app-8").layout().spec(PadLayout.Control.FACE).scale(), 1e-3);
    }

    /** M5: saving "for all games" in portrait must not delete the game's landscape layout. */
    @Test @Config(qualifiers = "w360dp-h800dp-port-xhdpi")
    public void savingForAllGamesKeepsTheOtherOrientationsPerGameLayout() {
        android.content.SharedPreferences store = RuntimeEnvironment.getApplication()
                .getSharedPreferences(ModernController.PREFS, Context.MODE_PRIVATE);
        store.edit().putString(PadLayoutKeys.key(true, GAME), "v1|FACE,0.7083,0.5556,1.300,100,0").commit();
        frame.getLayoutParams().width = 720;
        frame.getLayoutParams().height = 1600;
        ModernController controller = new ModernController(frame.getContext(), frame, sink, prefs, GAME);
        controller.refreshLayout();
        frame.measure(View.MeasureSpec.makeMeasureSpec(720, View.MeasureSpec.EXACTLY),
                View.MeasureSpec.makeMeasureSpec(1600, View.MeasureSpec.EXACTLY));
        frame.layout(0, 0, 720, 1600);
        PadLayout draft = controller.layout().copy();
        draft.setScale(PadLayout.Control.FACE, .7f);
        controller.save(draft, false);
        assertTrue("Saved for all games in portrait", store.contains(PadLayoutKeys.key(false, null)));
        assertTrue("The game's landscape layout stays", store.contains(PadLayoutKeys.key(true, GAME)));
        assertFalse("This orientation's own per-game layout goes", store.contains(PadLayoutKeys.key(false, GAME)));
        controller.destroy();
    }

    /** L4: showing the controls again tells the microphone button to move back below the top row. */
    @Test public void showingTheControlsAgainTellsTheMicrophoneButtonToMove() {
        ModernController controller = controller(GAME);
        int[] runs = {0};
        controller.setOnLayoutChanged(() -> runs[0]++);
        controller.hide();
        controller.show();
        assertEquals(1, runs[0]);
    }

    /** M3: the old single per-game layout is both orientations' fallback, so saving for all games keeps it for both. */
    @Test public void savingForAllGamesHandsTheGamesOldLayoutToTheOtherOrientation() {
        android.content.SharedPreferences store = RuntimeEnvironment.getApplication()
                .getSharedPreferences(ModernController.PREFS, Context.MODE_PRIVATE);
        String legacy = "v1|FACE,0.7083,0.5556,1.300,100,0";
        store.edit().putString(PadLayoutKeys.legacyKey(GAME), legacy).commit();
        ModernController controller = controller(GAME);
        assertEquals("The game's old layout is its own", 1.3f, controller.layout().spec(PadLayout.Control.FACE).scale(), 1e-3);
        PadLayout shared = controller.layout().copy();
        shared.setScale(PadLayout.Control.FACE, .7f);
        controller.save(shared, false);
        controller.destroy();
        assertFalse("The old shared key goes", store.contains(PadLayoutKeys.legacyKey(GAME)));
        assertEquals("The other orientation keeps the game's own layout", legacy,
                store.getString(PadLayoutKeys.key(false, GAME), null));
        // Portrait finds that layout instead of the shared one saved in landscape.
        RuntimeEnvironment.setQualifiers("w360dp-h800dp-port-xhdpi");
        frame.getLayoutParams().width = 720;
        frame.getLayoutParams().height = 1600;
        ModernController portrait = new ModernController(frame.getContext(), frame, sink, prefs, GAME);
        assertEquals(1.3f, portrait.layout().spec(PadLayout.Control.FACE).scale(), 1e-3);
        portrait.destroy();
    }

    /**
     * L2: the first modern start after a classic layout is carried over maps it with landscape dimensions, whatever
     * orientation it starts in, because the shared layout it becomes is used in both.
     */
    @Test @Config(qualifiers = "w360dp-h800dp-port-xhdpi")
    public void aClassicLayoutCarriedOverInPortraitUsesLandscapeDimensions() {
        RuntimeEnvironment.getApplication().getSharedPreferences(
                        ClassicLayoutMigration.CLASSIC_PREFS,
                        Context.MODE_PRIVATE)
                .edit().putString("13", "{\"LEFT\":1200,\"TOP\":400,\"WIDTH\":200,\"HEIGHT\":200}").commit();
        RuntimeEnvironment.getApplication().getSharedPreferences(ModernController.PREFS, Context.MODE_PRIVATE)
                .edit().remove("classic-migrated").commit();
        ModernController controller = controller(GAME);
        PadLayout.Spec stick = controller.layout().spec(PadLayout.Control.RIGHT_STICK);
        // This display is 720x1600; the classic stick sat on its 1600x720 landscape side at (1300, 500).
        assertEquals(1300f / 1600f, stick.cx, .01f);
        assertEquals(500f / 720f, stick.cy, .01f);
        controller.destroy();
    }

    /** The halo layer under the controls drawn into a bitmap of the frame's size. */
    private Bitmap haloPixels() {
        View halo = frame.getChildAt(0);
        assertTrue(halo instanceof PadControlView.HaloLayer);
        Bitmap pixels = Bitmap.createBitmap(frame.getWidth(), frame.getHeight(), Bitmap.Config.ARGB_8888);
        halo.draw(new Canvas(pixels));
        return pixels;
    }

    /**
     * A press shows in full at once and rings; let go, it eases back over the release time, and its halo past the
     * button's edge, where a thumb on the button leaves it in view, fades with it.
     */
    @Test @GraphicsMode(GraphicsMode.Mode.NATIVE)
    public void aPressShowsAtOnceAndItsHaloFadesAsItEasesBack() {
        ModernController controller = controller(GAME);
        PadControlView start = controller.view(PadLayout.Control.START);
        float density = frame.getResources().getDisplayMetrics().density;
        int outsideX = start.getRight() + Math.round(5 * density), middleY = (start.getTop() + start.getBottom()) / 2;
        assertEquals("No halo at rest", 0, Color.alpha(haloPixels().getPixel(outsideX, middleY)));
        start.down(0, 5, 5, 100);
        assertEquals(1, start.state().press[0], 0);
        assertTrue("It rings", start.state().pressedAt[0] != 0);
        assertTrue("Held, a halo past its box", Color.alpha(haloPixels().getPixel(outsideX, middleY)) > 0);
        start.up(0, 200);
        assertEquals("Let go, it starts easing back", 1, start.state().press[0], 0);
        // Robolectric runs an animation's frames back to back; a frame callback after the easing's records each frame.
        java.util.List<Float> frames = new java.util.ArrayList<>();
        Runnable[] record = new Runnable[1];
        record[0] = () -> {
            frames.add(start.state().press[0]);
            if (start.state().press[0] > 0 && frames.size() < 100) start.postOnAnimation(record[0]);
        };
        start.postOnAnimation(record[0]);
        ShadowLooper.idleMainLooper(PadPainter.RELEASE_MS + 40, java.util.concurrent.TimeUnit.MILLISECONDS);
        assertTrue("Over several frames: " + frames, frames.size() >= 4);
        for (int i = 1; i < frames.size(); i++) assertTrue("Only ever further out: " + frames, frames.get(i) <= frames.get(i - 1));
        assertTrue("Partway back on the way: " + frames, frames.get(frames.size() / 2) > 0 && frames.get(frames.size() / 2) < 1);
        assertEquals("Back out", 0, start.state().press[0], 0);
        ShadowLooper.idleMainLooper(PadPainter.RIPPLE_MS, java.util.concurrent.TimeUnit.MILLISECONDS);
        assertEquals("and its halo gone", 0, Color.alpha(haloPixels().getPixel(outsideX, middleY)));
        controller.destroy();
    }

    /**
     * Every control a player presses answers past its edge, where a thumb on it leaves the halo in view: each control of
     * the layout with the Steam Controller's added, the WASD stick's keys, a stick's click and a finger on the pointer
     * touchpad. Once let go and the ring has run out, nothing stays lit.
     */
    @Test @GraphicsMode(GraphicsMode.Mode.NATIVE)
    public void everyPressableControlShowsAHaloPastItsEdge() {
        prefs.oscSteamControls = true;
        ModernController controller = controller(GAME);
        PadLayout shown = controller.layout().copy();
        shown.spec(PadLayout.Control.WASD).hidden = false;
        shown.spec(PadLayout.Control.POINTER_PAD).hidden = false;
        for (PadLayout.Control control : PadLayout.Control.values()) {
            if (control.steam) shown.spec(control).hidden = false;
        }
        controller.save(shown, false);
        controller.refreshLayout();
        layout();
        java.util.Set<PadLayout.Shape> shapes = java.util.EnumSet.noneOf(PadLayout.Shape.class);
        for (PadLayout.Control control : PadLayout.Control.values()) {
            PadControlView view = controller.view(control);
            if (view == null || view.getVisibility() != View.VISIBLE) continue;
            press(view);
            assertTrue(control + " lights past its box", litOutside(haloPixels(), view) > 0);
            view.up(0, 400);
            if (PadInput.latches(control)) {
                // A grip holds from one tap to the next.
                assertTrue(control + " still held", litOutside(haloPixels(), view) > 0);
                view.down(0, view.getWidth() / 2f, view.getHeight() / 2f, 500);
                view.up(0, 600);
            }
            ShadowLooper.idleMainLooper(PadPainter.RELEASE_MS + PadPainter.RIPPLE_MS + 40,
                    java.util.concurrent.TimeUnit.MILLISECONDS);
            assertEquals(control + " leaves nothing once let go", 0, litOutside(haloPixels(), null));
            shapes.add(control.shape);
        }
        // The split members' shapes are covered by the preview's split captures and drawHalo's own cases.
        for (PadLayout.Shape shape : new PadLayout.Shape[] {PadLayout.Shape.FACE, PadLayout.Shape.DPAD,
                PadLayout.Shape.STICK, PadLayout.Shape.PILL, PadLayout.Shape.ROUND, PadLayout.Shape.PAD,
                PadLayout.Shape.TOUCHPAD}) {
            assertTrue(shape + " was pressed", shapes.contains(shape));
        }
        controller.destroy();
    }

    /** Presses {@code view} the way a player does: a face button, a D-pad arm, a WASD key, a stick's click or the middle. */
    private static void press(PadControlView view) {
        float w = view.getWidth(), h = view.getHeight();
        switch (view.control.shape) {
            case FACE:
                view.down(0, PadInput.FACE_CENTERS[0][0] * w, PadInput.FACE_CENTERS[0][1] * h, 100);
                break;
            case DPAD:
                view.down(0, w / 2, h * .15f, 100);
                break;
            case STICK:
                if (view.control == PadLayout.Control.WASD) {
                    view.down(0, w / 2, h / 2, 100);
                    view.move(0, w / 2, h * .1f);
                } else {
                    // A double tap clicks a stick.
                    view.down(0, w / 2, h / 2, 100);
                    view.up(0, 150);
                    view.down(0, w / 2, h / 2, 200);
                }
                break;
            default:
                view.down(0, w / 2, h / 2, 100);
                break;
        }
    }

    /** How many pixels are lit outside {@code view}'s box, or anywhere when it is null. */
    private static int litOutside(Bitmap pixels, View view) {
        int width = pixels.getWidth(), height = pixels.getHeight();
        int[] argb = new int[width * height];
        pixels.getPixels(argb, 0, width, 0, 0, width, height);
        int lit = 0;
        for (int y = 0; y < height; y++) {
            for (int x = 0; x < width; x++) {
                if (view != null && x >= view.getLeft() && x < view.getRight() && y >= view.getTop()
                        && y < view.getBottom()) continue;
                if (Color.alpha(argb[y * width + x]) > 0) lit++;
            }
        }
        return lit;
    }

    /** At opacity 0 a press neither rings nor leaves a halo, and letting go drops it at once: nothing is drawn. */
    @Test @GraphicsMode(GraphicsMode.Mode.NATIVE)
    public void atOpacityZeroAPressHasNoHaloAndNoEasing() {
        prefs.oscOpacity = 0;
        ModernController controller = controller(GAME);
        PadControlView start = controller.view(PadLayout.Control.START);
        float density = frame.getResources().getDisplayMetrics().density;
        start.down(0, 5, 5, 100);
        assertEquals(0, start.state().pressedAt[0]);
        Bitmap pixels = haloPixels();
        for (int x = 0; x < pixels.getWidth(); x += Math.round(4 * density)) {
            for (int y = 0; y < pixels.getHeight(); y += Math.round(4 * density)) {
                assertEquals("Nothing at " + x + "," + y, 0, Color.alpha(pixels.getPixel(x, y)));
            }
        }
        start.up(0, 200);
        assertEquals(0, start.state().press[0], 0);
        assertEquals(0, start.getAlpha(), 0);
        controller.destroy();
    }

    /** Input taken away (the editor, focus loss) drops a press at once, without easing back. */
    @Test public void inputTakenAwayDropsAPressAtOnce() {
        ModernController controller = controller(GAME);
        PadControlView start = controller.view(PadLayout.Control.START);
        PadControlView face = controller.view(PadLayout.Control.FACE);
        start.down(0, 5, 5, 100);
        face.down(1, face.getWidth() / 2f, face.getHeight() * .78f, 100);
        assertEquals(1, face.state().press[0], 0);
        controller.releaseInput();
        for (PadControlView view : new PadControlView[] {start, face}) {
            assertEquals(view.control.name(), 0, view.state().press[0], 0);
            assertEquals(view.control.name(), 0, view.state().pressedAt[0]);
        }
        assertEquals(prefs.oscOpacity / 100f, start.getAlpha(), 1e-4);
        controller.destroy();
    }
}
