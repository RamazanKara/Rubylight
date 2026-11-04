package com.limelight.binding.input.pad;

import android.app.Application;
import android.content.Context;
import android.graphics.Bitmap;
import android.graphics.Canvas;
import android.graphics.Color;
import android.view.MotionEvent;
import android.view.View;
import android.widget.FrameLayout;

import com.limelight.binding.input.HeldInputState;
import com.limelight.binding.input.StickMath;
import com.limelight.nvstream.input.ControllerPacket;
import com.limelight.preferences.PreferenceConfiguration;
import com.limelight.ui.UiNavigationTest;

import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.GraphicsMode;
import org.robolectric.shadows.ShadowLooper;

import java.util.ArrayList;
import java.util.List;

import static org.junit.Assert.*;
import static org.robolectric.Shadows.shadowOf;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 30, application = Application.class, qualifiers = "w800dp-h360dp-land-xhdpi")
public class ModernControllerTest {
    private static final String GAME = "pc-1/app-7";
    private final List<int[]> reports = new ArrayList<>();
    /** Keys from the WASD stick, as "+W" and "-W"; only the WASD stick sends any. */
    private final List<String> keys = new ArrayList<>();
    private final HeldInputState.Sink sink = new HeldInputState.Sink() {
        @Override public void key(int key, boolean down) { keys.add((down ? "+" : "-") + (char) key); }
        @Override public void mouse(int button, boolean down) { fail("Gamepad only"); }
        @Override public void gamepad(int buttons, short lx, short ly, short rx, short ry, byte lt, byte rt) {
            reports.add(new int[] {buttons, lx, ly, rx, ry, lt & 0xFF, rt & 0xFF});
        }
    };
    private FrameLayout frame;
    private PreferenceConfiguration prefs;

    @Before
    public void setUp() {
        Context app = RuntimeEnvironment.getApplication();
        ModernController.clearLayouts(app);
        UiNavigationTest.Screen screen = Robolectric.buildActivity(UiNavigationTest.Screen.class).setup().visible().get();
        frame = new FrameLayout(screen);
        screen.setContentView(frame, new FrameLayout.LayoutParams(1600, 720));
        prefs = new PreferenceConfiguration();
        prefs.oscStyle = PreferenceConfiguration.OSC_STYLE_MODERN;
        prefs.oscOpacity = 90;
        // HOME follows the guide button setting; on here so every control is covered.
        prefs.showGuideButton = true;
        prefs.showTouchpadButton = true;
        layout();
    }

    private void layout() {
        frame.measure(View.MeasureSpec.makeMeasureSpec(1600, View.MeasureSpec.EXACTLY),
                View.MeasureSpec.makeMeasureSpec(720, View.MeasureSpec.EXACTLY));
        frame.layout(0, 0, 1600, 720);
    }

    private ModernController controller(String game) {
        ModernController controller = new ModernController(frame.getContext(), frame, sink, prefs, game);
        controller.refreshLayout();
        controller.show();
        layout();
        return controller;
    }

    private int[] last() {
        return reports.get(reports.size() - 1);
    }

    @Test public void everyControlIsItsOwnViewInTheStreamFrame() {
        ModernController controller = controller(GAME);
        // Every control, plus each stick's knob layer above them (the WASD stick's too, though it starts hidden).
        assertEquals(PadLayout.Control.values().length + 3, frame.getChildCount());
        assertEquals(View.GONE, controller.view(PadLayout.Control.WASD).getVisibility());
        PadControlView face = controller.view(PadLayout.Control.FACE);
        assertEquals(View.VISIBLE, face.getVisibility());
        assertTrue(face.getWidth() > 0);
        controller.destroy();
        assertEquals(0, frame.getChildCount());
    }

    @Test public void aFaceTouchSendsItsButtonAndLettingGoReleasesIt() {
        PadControlView face = controller(GAME).view(PadLayout.Control.FACE);
        float size = Math.min(face.getWidth(), face.getHeight());
        face.down(0, .5f * size, .78f * size, 100);
        assertEquals(ControllerPacket.A_FLAG, last()[0]);
        // Sliding to the gap between A and B holds both, as a thumb resting on two buttons does.
        face.move(0, .64f * size, .64f * size);
        assertEquals(ControllerPacket.A_FLAG | ControllerPacket.B_FLAG, last()[0]);
        face.up(0, 200);
        assertEquals(0, last()[0]);
    }

    @Test public void twoFingersOnTheFaceClusterHoldTwoButtons() {
        PadControlView face = controller(GAME).view(PadLayout.Control.FACE);
        float size = Math.min(face.getWidth(), face.getHeight());
        face.down(0, .5f * size, .78f * size, 100);
        face.down(1, .5f * size, .22f * size, 110);
        assertEquals(ControllerPacket.A_FLAG | ControllerPacket.Y_FLAG, last()[0]);
        face.up(0, 150);
        assertEquals(ControllerPacket.Y_FLAG, last()[0]);
    }

    @Test public void aFloatingStickCentersUnderTheThumbAndReportsItsPush() {
        PadControlView stick = controller(GAME).view(PadLayout.Control.LEFT_STICK);
        float size = Math.min(stick.getWidth(), stick.getHeight());
        float travel = PadPainter.travel(size);
        reports.clear();
        stick.down(0, size * .3f, size * .7f, 100);
        // Landing is not a push.
        assertTrue(reports.isEmpty() || (last()[1] == 0 && last()[2] == 0));
        stick.move(0, size * .3f + travel, size * .7f);
        assertEquals(32766, last()[1]);
        assertEquals(0, last()[2]);
        stick.up(0, 400);
        assertEquals(0, last()[1]);
    }

    /** A controller whose saved layout shows the WASD stick, with fixed sticks so a push starts at the drawn center. */
    private ModernController wasdController() {
        prefs.oscFloatingSticks = false;
        ModernController first = controller(GAME);
        PadLayout draft = first.layout().copy();
        draft.spec(PadLayout.Control.WASD).hidden = false;
        first.save(draft, false);
        first.destroy();
        ModernController controller = controller(GAME);
        assertEquals(View.VISIBLE, controller.view(PadLayout.Control.WASD).getVisibility());
        return controller;
    }

    /** Puts a finger on the WASD stick pushed {@code push} of full travel toward {@code degrees} (0 right, 90 up). */
    private static void push(PadControlView stick, boolean down, double degrees, float push) {
        float size = Math.min(stick.getWidth(), stick.getHeight()), travel = PadPainter.travel(size);
        float x = stick.getWidth() / 2f + (float) Math.cos(Math.toRadians(degrees)) * travel * push;
        float y = stick.getHeight() / 2f - (float) Math.sin(Math.toRadians(degrees)) * travel * push;
        if (down) stick.down(0, x, y, 100); else stick.move(0, x, y);
    }

    private void assertEveryKeyLetGo(String when) {
        int downs = 0, ups = 0;
        for (String key : keys) if (key.startsWith("+")) downs++; else ups++;
        assertTrue(when + ": something was held first " + keys, downs > 0);
        assertEquals(when + ": every key sent down is sent up " + keys, downs, ups);
    }

    @Test public void theWasdStickHoldsKeysForEachOfItsEightDirectionsAndNoGamepad() {
        PadControlView stick = wasdController().view(PadLayout.Control.WASD);
        String[] expected = {"D", "WD", "W", "WA", "A", "SA", "S", "SD"};
        reports.clear();
        for (int sector = 0; sector < 8; sector++) {
            keys.clear();
            push(stick, true, sector * 45, 1);
            List<String> want = new ArrayList<>();
            for (char key : expected[sector].toCharArray()) want.add("+" + key);
            assertEquals(sector * 45 + " degrees", want, keys);
            stick.up(0, 500);
            assertEquals(WasdKeys.NONE, stick.wasd().sector());
        }
        for (int[] report : reports) assertEquals("The WASD stick moves no gamepad stick", 0, report[1] | report[2] | report[0]);
    }

    @Test public void theWasdStickUsesTheModernStickDeadzone() {
        prefs.oscStickDeadzone = 20;
        PadControlView stick = wasdController().view(PadLayout.Control.WASD);
        push(stick, true, 90, .15f);
        push(stick, false, 180, .18f);
        assertTrue("Inside the deadzone nothing is held: " + keys, keys.isEmpty());
        push(stick, false, 90, .6f);
        assertEquals(List.of("+W"), keys);
    }

    @Test public void theWasdStickTicksForADirectionAndDifferentlyForADiagonalOnlyOnAChange() {
        prefs.vibrateOsc = true;
        PadControlView stick = wasdController().view(PadLayout.Control.WASD);
        int marker = android.view.HapticFeedbackConstants.LONG_PRESS;
        stick.performHapticFeedback(marker);
        // Landing in the deadzone is quiet: the stick answers keys, not touches.
        push(stick, true, 90, 0);
        assertEquals(marker, shadowOf(stick).lastHapticFeedbackPerformed());
        push(stick, false, 90, 1);
        int direction = shadowOf(stick).lastHapticFeedbackPerformed();
        assertEquals(android.view.HapticFeedbackConstants.CONTEXT_CLICK, direction);
        // Moving on inside W repeats nothing.
        stick.performHapticFeedback(marker);
        push(stick, false, 100, .8f);
        push(stick, false, 80, 1);
        assertEquals(marker, shadowOf(stick).lastHapticFeedbackPerformed());
        push(stick, false, 135, 1);
        int diagonal = shadowOf(stick).lastHapticFeedbackPerformed();
        assertEquals(android.view.HapticFeedbackConstants.VIRTUAL_KEY, diagonal);
        assertNotEquals("A diagonal feels unlike a direction", direction, diagonal);
        push(stick, false, 180, 1);
        assertEquals(direction, shadowOf(stick).lastHapticFeedbackPerformed());
        // Back to the center and lifting are quiet.
        stick.performHapticFeedback(marker);
        push(stick, false, 0, 0);
        stick.up(0, 500);
        assertEquals(marker, shadowOf(stick).lastHapticFeedbackPerformed());
    }

    @Test public void theWasdStickFollowsVibrateOnPress() {
        prefs.vibrateOsc = false;
        PadControlView stick = wasdController().view(PadLayout.Control.WASD);
        int marker = android.view.HapticFeedbackConstants.LONG_PRESS;
        stick.performHapticFeedback(marker);
        push(stick, true, 90, 1);
        push(stick, false, 45, 1);
        assertEquals(List.of("+W", "+D"), keys);
        assertEquals(marker, shadowOf(stick).lastHapticFeedbackPerformed());
    }

    @Test public void theWasdStickLetsGoOfItsKeysOnEveryWayOut() {
        ModernController controller = wasdController();
        PadControlView stick = controller.view(PadLayout.Control.WASD);

        push(stick, true, 45, 1);
        stick.up(0, 500);
        assertEveryKeyLetGo("Lift");

        keys.clear();
        push(stick, true, 135, 1);
        long now = android.os.SystemClock.uptimeMillis();
        MotionEvent cancel = MotionEvent.obtain(now, now, MotionEvent.ACTION_CANCEL, 0, 0, 0);
        stick.onTouchEvent(cancel);
        cancel.recycle();
        assertEveryKeyLetGo("Cancel");

        keys.clear();
        push(stick, true, 225, 1);
        controller.releaseInput();
        assertEveryKeyLetGo("Focus loss");

        keys.clear();
        push(stick, true, 315, 1);
        controller.hide();
        assertEveryKeyLetGo("Stream end or picture-in-picture");
        controller.show();

        keys.clear();
        push(stick, true, 90, 1);
        controller.startEditing();
        assertEveryKeyLetGo("Editing");
        PadEditor editor = (PadEditor) frame.getChildAt(frame.getChildCount() - 1);
        editor.select(PadLayout.Control.WASD);
        editor.toggleHidden();
        controller.finishEditing(true);
        assertEquals("Hiding it in the editor removes it", View.GONE, stick.getVisibility());

        // Shown again, then removed with the controller.
        PadLayout draft = controller.layout().copy();
        draft.spec(PadLayout.Control.WASD).hidden = false;
        controller.save(draft, false);
        controller.refreshLayout();
        layout();
        stick = controller.view(PadLayout.Control.WASD);
        keys.clear();
        push(stick, true, 270, 1);
        controller.refreshLayout();
        assertEveryKeyLetGo("Rebuilt from the settings");

        stick = controller.view(PadLayout.Control.WASD);
        layout();
        keys.clear();
        push(stick, true, 180, 1);
        controller.destroy();
        assertEveryKeyLetGo("Removed");
    }

    @Test public void theEditorAddsTheWasdStickWithItsOwnButton() {
        ModernController controller = controller(GAME);
        assertEquals(View.GONE, controller.view(PadLayout.Control.WASD).getVisibility());
        controller.startEditing();
        PadEditor editor = (PadEditor) frame.getChildAt(frame.getChildCount() - 1);
        View add = editor.findViewById(com.limelight.R.id.pad_editor_add_wasd);
        assertEquals(View.VISIBLE, add.getVisibility());
        assertTrue(add.performClick());
        assertEquals(PadLayout.Control.WASD, editor.selected());
        assertFalse(editor.draft().spec(PadLayout.Control.WASD).hidden);
        assertEquals("Added, the button goes", View.GONE, add.getVisibility());
        editor.toggleHidden();
        assertEquals("Hidden again, it comes back", View.VISIBLE, add.getVisibility());
        editor.toggleHidden();
        controller.finishEditing(true);
        layout();
        assertEquals(View.VISIBLE, controller.view(PadLayout.Control.WASD).getVisibility());
        assertFalse(controller("pc-1/app-8").layout().spec(PadLayout.Control.WASD).hidden);
    }

    @Test public void theWasdStickHiddenWhileHeldLetsGo() {
        ModernController controller = wasdController();
        PadControlView stick = controller.view(PadLayout.Control.WASD);
        push(stick, true, 0, 1);
        controller.layout().spec(PadLayout.Control.WASD).hidden = true;
        controller.place();
        assertEquals(View.GONE, stick.getVisibility());
        assertEveryKeyLetGo("Hidden");
    }

    @Test public void aFixedStickPushesFromItsDrawnCenter() {
        prefs.oscFloatingSticks = false;
        PadControlView stick = controller(GAME).view(PadLayout.Control.RIGHT_STICK);
        float size = Math.min(stick.getWidth(), stick.getHeight());
        stick.down(0, stick.getWidth() / 2f, stick.getHeight() / 2f - PadPainter.travel(size), 100);
        assertEquals(0, last()[3]);
        assertEquals(32766, last()[4]);
    }

    @Test @GraphicsMode(GraphicsMode.Mode.NATIVE)
    public void aFullyPushedKnobSitsOnTheRimAndDrawsPastTheControlWithoutTakingTouches() {
        prefs.oscFloatingSticks = false;
        ModernController controller = controller(GAME);
        PadControlView stick = controller.view(PadLayout.Control.LEFT_STICK);
        View knob = stick.knobLayer();
        assertNotNull(knob);
        assertNull(controller.view(PadLayout.Control.FACE).knobLayer());

        // The control keeps the layout's box, so saved layouts place and touch exactly as before.
        float density = frame.getResources().getDisplayMetrics().density;
        float[] box = new float[4];
        controller.layout().bounds(PadLayout.Control.LEFT_STICK, 1600, 720, density, box);
        assertEquals(Math.round(box[0]), stick.getLeft());
        assertEquals(Math.round(box[1]), stick.getTop());
        assertEquals(Math.round(box[2]), stick.getWidth());
        assertEquals(Math.round(box[3]), stick.getHeight());

        // The knob's layer is that box grown on every side by enough for a knob centred on the rim.
        int w = stick.getWidth(), h = stick.getHeight();
        float size = Math.min(w, h), stroke = 1.5f * density;
        float base = StickMath.baseRadius(size, stroke), radius = StickMath.knobRadius(size);
        int overhang = stick.knobOverhang(w, h);
        assertTrue("overhang " + overhang, overhang >= base + radius - size / 2);
        assertEquals(stick.getLeft() - overhang, knob.getLeft());
        assertEquals(stick.getTop() - overhang, knob.getTop());
        assertEquals(stick.getRight() + overhang, knob.getRight());
        assertEquals(stick.getBottom() + overhang, knob.getBottom());

        // Pushed fully right (and past the rim): the same full value as before, the knob's centre on the rim.
        stick.down(0, w / 2f, h / 2f, 100);
        stick.move(0, w / 2f + 3 * PadPainter.travel(size), h / 2f);
        assertEquals(32766, last()[1]);
        assertEquals(0, last()[2]);
        PadPainter.State state = stick.state();
        float kx = StickMath.knobX(w / 2f, h / 2f, w / 2f, h / 2f, state.stickX, state.stickY, base);
        assertEquals(w / 2f + base, kx, .01f);

        // Half the knob lies outside the control's box, and the layer paints it there.
        Bitmap pixels = Bitmap.createBitmap(knob.getWidth(), knob.getHeight(), Bitmap.Config.ARGB_8888);
        knob.draw(new Canvas(pixels));
        int outside = Math.round(overhang + w / 2f + base + radius * .6f);
        assertTrue(outside > overhang + w);
        assertTrue("knob not drawn past the control", Color.alpha(pixels.getPixel(outside, Math.round(overhang + h / 2f))) > 0);
        assertEquals(0, Color.alpha(pixels.getPixel(Math.round(overhang + w / 2f - base + radius), Math.round(overhang + h / 2f))));

        // A finger landing on the knob's overhang, outside the control, never reaches the stick.
        stick.up(0, 400);
        MotionEvent down = MotionEvent.obtain(1000, 1000, MotionEvent.ACTION_DOWN, knob.getLeft() + overhang / 2f,
                stick.getTop() + h / 2f, 0);
        assertFalse(knob.dispatchTouchEvent(MotionEvent.obtain(down)));
        frame.dispatchTouchEvent(down);
        assertFalse(stick.state().stickActive);
        down.recycle();
    }

    @Test public void tappingAStickThenTouchingItAgainHoldsItsClick() {
        PadControlView stick = controller(GAME).view(PadLayout.Control.LEFT_STICK);
        float c = stick.getWidth() / 2f;
        stick.down(0, c, c, 1000);
        stick.up(0, 1080);
        stick.down(0, c, c, 1200);
        assertEquals(ControllerPacket.LS_CLK_FLAG, last()[0] & ControllerPacket.LS_CLK_FLAG);
        stick.up(0, 1500);
        assertEquals(0, last()[0]);
        // A third touch is a new stick touch, not another click.
        stick.down(0, c, c, 1600);
        assertEquals(0, last()[0] & ControllerPacket.LS_CLK_FLAG);
    }

    @Test public void triggersPullFullyAndShouldersHoldTheirButton() {
        ModernController controller = controller(GAME);
        controller.view(PadLayout.Control.RT).down(0, 5, 5, 100);
        assertEquals(255, last()[6]);
        controller.view(PadLayout.Control.LB).down(1, 5, 5, 110);
        assertEquals(ControllerPacket.LB_FLAG, last()[0]);
        assertEquals(255, last()[6]);
        controller.releaseInput();
        assertArrayEquals(new int[7], last());
    }

    @Test public void resendsFollowAChangeLikeTheClassicController() {
        ModernController controller = controller(GAME);
        controller.view(PadLayout.Control.START).down(0, 5, 5, 100);
        int sent = reports.size();
        ShadowLooper.idleMainLooper(100, java.util.concurrent.TimeUnit.MILLISECONDS);
        assertEquals(sent + 3, reports.size());
        assertEquals(ControllerPacket.PLAY_FLAG, last()[0]);
    }

    @Test public void onlyStickClicksLeavesJustL3AndR3() {
        prefs.onlyL3R3 = true;
        ModernController controller = controller(GAME);
        assertEquals(2, frame.getChildCount());
        assertNotNull(controller.view(PadLayout.Control.L3));
        assertNull(controller.view(PadLayout.Control.FACE));
    }

    @Test public void homeShowsOnlyWithTheGuideButtonSetting() {
        prefs.showGuideButton = false;
        assertNull(controller(GAME).view(PadLayout.Control.HOME));
    }

    @Test public void theTouchpadClicksThePadsTouchpadAndShowsOnlyWithItsSetting() {
        PadControlView touchpad = controller(GAME).view(PadLayout.Control.TOUCHPAD);
        assertEquals(View.VISIBLE, touchpad.getVisibility());
        touchpad.down(0, touchpad.getWidth() / 2f, touchpad.getHeight() / 2f, 100);
        assertEquals(ControllerPacket.TOUCHPAD_FLAG, last()[0]);
        touchpad.up(0, 200);
        assertEquals(0, last()[0]);
        frame.removeAllViews();
        prefs.showTouchpadButton = false;
        assertNull("Off by default, so the default layout stays as it was", controller(GAME).view(PadLayout.Control.TOUCHPAD));
        assertFalse(new PreferenceConfiguration().showTouchpadButton);
    }

    @Test public void aLayoutSavedForOneGameStaysWithThatGame() {
        ModernController first = controller(GAME);
        PadLayout draft = first.layout().copy();
        draft.setScale(PadLayout.Control.FACE, 1.5f);
        first.save(draft, true);
        first.destroy();
        assertEquals(1.5f, controller(GAME).layout().spec(PadLayout.Control.FACE).scale, 1e-3);
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
        assertEquals(.7f, controller(GAME).layout().spec(PadLayout.Control.FACE).scale, 1e-3);
        assertEquals(.7f, controller("pc-1/app-8").layout().spec(PadLayout.Control.FACE).scale, 1e-3);
        assertFalse(RuntimeEnvironment.getApplication().getSharedPreferences(ModernController.PREFS, Context.MODE_PRIVATE)
                .contains(ModernController.GLOBAL_KEY + ":" + GAME));
    }

    @Test public void aStreamWithoutAGameSavesOnlyTheSharedLayout() {
        ModernController controller = controller(null);
        PadLayout draft = controller.layout().copy();
        draft.spec(PadLayout.Control.LB).hidden = true;
        controller.save(draft, true);
        assertTrue(controller("any").layout().spec(PadLayout.Control.LB).hidden);
    }

    @Test public void editingPausesInputAndCancelKeepsTheOldLayout() {
        ModernController controller = controller(GAME);
        PadControlView face = controller.view(PadLayout.Control.FACE);
        face.down(0, face.getWidth() / 2f, face.getHeight() * .78f, 100);
        controller.startEditing();
        assertTrue(controller.isEditing());
        assertEquals("Editing lets go of what was held", 0, last()[0]);
        assertEquals(View.GONE, face.getVisibility());
        controller.finishEditing(false);
        assertFalse(controller.isEditing());
        assertEquals(View.VISIBLE, face.getVisibility());
        assertTrue(controller.layout().isDefault());
    }

    @Test public void theEditorsDoneKeepsTheDraft() {
        ModernController controller = controller(GAME);
        controller.startEditing();
        PadEditor editor = (PadEditor) frame.getChildAt(frame.getChildCount() - 1);
        editor.select(PadLayout.Control.DPAD);
        editor.scaleBy(.1f);
        editor.toggleHidden();
        controller.finishEditing(true);
        assertEquals(1.1f, controller.layout().spec(PadLayout.Control.DPAD).scale, 1e-3);
        assertEquals(View.GONE, controller.view(PadLayout.Control.DPAD).getVisibility());
        controller.destroy();
        assertTrue("A game without its own layout saves for every game, so another game sees it",
                controller("pc-1/app-8").layout().spec(PadLayout.Control.DPAD).hidden);
    }

    @Test public void theEditorWithNothingPickedChangesEveryControl() {
        ModernController controller = controller(GAME);
        controller.startEditing();
        PadEditor editor = (PadEditor) frame.getChildAt(frame.getChildCount() - 1);
        editor.setOpacity(50);
        editor.scaleBy(-.1f);
        for (PadLayout.Control control : PadLayout.Control.values()) {
            assertEquals(control.name(), 50, editor.draft().spec(control).opacity);
            assertEquals(control.name(), .9f, editor.draft().spec(control).scale, 1e-3);
        }
        editor.toggleHidden();
        assertFalse("Hide needs a picked control", editor.draft().spec(PadLayout.Control.FACE).hidden);
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

    @Test public void closingTheEditorTellsTheMicrophoneButtonToMove() {
        ModernController controller = controller(GAME);
        int[] runs = {0};
        controller.setOnLayoutChanged(() -> runs[0]++);
        controller.startEditing();
        controller.finishEditing(false);
        controller.startEditing();
        controller.finishEditing(true);
        assertEquals("Cancel and Done both report", 2, runs[0]);
        controller.finishEditing(true);
        assertEquals("Nothing to close, nothing reported", 2, runs[0]);
    }

    /** A portrait phone: Hide and Reset turn into 48 dp icons so Cancel and Done stay whole on the row. */
    @Test @Config(qualifiers = "w360dp-h800dp-port-xhdpi")
    public void aNarrowEditorKeepsEveryButtonWholeAndTouchable() {
        frame.getLayoutParams().width = 720;
        frame.getLayoutParams().height = 1600;
        ModernController controller = new ModernController(frame.getContext(), frame, sink, prefs, GAME);
        controller.refreshLayout();
        controller.startEditing();
        for (int pass = 0; pass < 2; pass++) {
            ShadowLooper.idleMainLooper();
            frame.measure(View.MeasureSpec.makeMeasureSpec(720, View.MeasureSpec.EXACTLY),
                    View.MeasureSpec.makeMeasureSpec(1600, View.MeasureSpec.EXACTLY));
            frame.layout(0, 0, 720, 1600);
        }
        PadEditor editor = (PadEditor) frame.getChildAt(frame.getChildCount() - 1);
        editor.select(PadLayout.Control.DPAD);
        ShadowLooper.idleMainLooper();
        float density = frame.getResources().getDisplayMetrics().density;
        View row = (View) editor.findViewById(com.limelight.R.id.pad_editor_done).getParent();
        assertTrue("The editor laid out", row.getWidth() > 0);
        int[] ids = {com.limelight.R.id.pad_editor_hide, com.limelight.R.id.pad_editor_reset,
                com.limelight.R.id.pad_editor_cancel, com.limelight.R.id.pad_editor_done};
        for (int id : ids) {
            com.google.android.material.button.MaterialButton button = editor.findViewById(id);
            String name = frame.getResources().getResourceEntryName(id);
            assertTrue(name + " is " + button.getWidth() / density + " dp wide", button.getWidth() >= 48 * density - 1);
            assertTrue(name + " is " + button.getHeight() / density + " dp tall", button.getHeight() >= 48 * density - 1);
            assertTrue(name + " ends inside the panel", button.getRight() <= row.getWidth());
        }
        com.google.android.material.button.MaterialButton hide = editor.findViewById(com.limelight.R.id.pad_editor_hide);
        assertEquals("Icon alone", 0, hide.getText().length());
        assertEquals("Still named for TalkBack", frame.getContext().getString(com.limelight.R.string.pad_edit_hide),
                hide.getContentDescription());
        controller.destroy();
    }
}
