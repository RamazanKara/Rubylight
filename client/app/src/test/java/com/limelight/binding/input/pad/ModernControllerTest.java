package com.limelight.binding.input.pad;

import android.app.Application;
import android.graphics.Bitmap;
import android.graphics.Canvas;
import android.graphics.Color;
import android.view.MotionEvent;
import android.view.View;

import com.limelight.binding.input.StickMath;
import com.limelight.nvstream.input.ControllerPacket;
import com.limelight.preferences.PreferenceConfiguration;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.GraphicsMode;
import org.robolectric.shadows.ShadowLooper;

import java.util.ArrayList;
import java.util.List;

import static org.junit.Assert.*;
import static org.robolectric.Shadows.shadowOf;

/**
 * Core {@link ModernController} gameplay tests: views, face/stick/trigger/shoulder input, WASD
 * stick directions and release paths, split clusters, knob overhang, and visibility flags.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 30, application = Application.class, qualifiers = "w800dp-h360dp-land-xhdpi")
public class ModernControllerTest extends ModernControllerHarness {
    @Test public void everyControlIsItsOwnViewInTheStreamFrame() {
        ModernController controller = controller(GAME);
        // Every control, plus each stick's knob layer above them (the WASD stick's too, though it starts hidden) and
        // the press halos' layer under them; the Steam Controller controls only with their setting.
        assertEquals(PadLayout.Control.values().length - 8 + 3 + 1, frame.getChildCount());
        assertTrue(frame.getChildAt(0) instanceof PadControlView.HaloLayer);
        assertNull(controller.view(PadLayout.Control.LEFT_PAD));
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

    /** A controller whose saved layout has the face cluster or the D-pad split into its members. */
    private ModernController splitController(PadLayout.Control cluster) {
        ModernController first = controller(GAME);
        PadLayout draft = first.layout().copy();
        draft.split(cluster, frame.getWidth(), frame.getHeight(), frame.getResources().getDisplayMetrics().density);
        first.save(draft, false);
        first.destroy();
        return controller(GAME);
    }

    /** A point of the frame in {@code view}'s own coordinates, as a finger on it reports. */
    private static float[] in(View view, float frameX, float frameY) {
        return new float[] {frameX - view.getLeft(), frameY - view.getTop()};
    }

    /**
     * Split, A, B, X and Y are controls of their own, and a finger slides across them as it does on the cluster: from
     * A to B it lets go of A and presses B, though the finger still belongs to A's view. Their boxes never overlap, so
     * one finger never holds two, and between two it holds neither. Each shows itself pressed while held.
     */
    @Test public void aSplitFaceButtonSlidesToTheNextAndNeverHoldsTwo() {
        ModernController controller = splitController(PadLayout.Control.FACE);
        assertEquals(View.GONE, controller.view(PadLayout.Control.FACE).getVisibility());
        for (PadLayout.Control member : PadLayout.members(PadLayout.Control.FACE)) {
            assertEquals(member.name(), View.VISIBLE, controller.view(member).getVisibility());
        }
        PadControlView a = controller.view(PadLayout.Control.A), b = controller.view(PadLayout.Control.B);
        a.down(0, a.getWidth() / 2f, a.getHeight() / 2f, 100);
        assertEquals(ControllerPacket.A_FLAG, last()[0]);
        assertTrue(a.state().pressed);

        float[] onB = in(a, b.getLeft() + b.getWidth() / 2f, b.getTop() + b.getHeight() / 2f);
        a.move(0, onB[0], onB[1]);
        assertEquals("Slid onto B", ControllerPacket.B_FLAG, last()[0]);
        assertFalse(a.state().pressed);
        assertTrue(b.state().pressed);

        assertTrue("A, below the middle, is clear of B beside it", a.getTop() >= b.getBottom() && a.getRight() <= b.getLeft());
        float[] between = in(a, b.getLeft() + 1, a.getTop() + 1);
        a.move(0, between[0], between[1]);
        assertEquals("Between them, neither", 0, last()[0]);
        a.move(0, onB[0], onB[1]);
        assertEquals(ControllerPacket.B_FLAG, last()[0]);

        // A second finger on Y holds Y as well; the first lifting lets go of only its own.
        PadControlView y = controller.view(PadLayout.Control.Y);
        y.down(1, y.getWidth() / 2f, y.getHeight() / 2f, 150);
        assertEquals(ControllerPacket.B_FLAG | ControllerPacket.Y_FLAG, last()[0]);
        a.up(0, 300);
        assertEquals(ControllerPacket.Y_FLAG, last()[0]);
        assertFalse(a.state().pressed || b.state().pressed);
        // Slid off every member, nothing.
        float[] away = in(y, y.getLeft() - 300, y.getTop() - 300);
        y.move(1, away[0], away[1]);
        assertEquals(0, last()[0]);
        y.up(1, 400);

        // A member a finger slid onto lets go when it hides, and a hidden member takes no input.
        a.down(2, a.getWidth() / 2f, a.getHeight() / 2f, 500);
        a.move(2, onB[0], onB[1]);
        assertEquals(ControllerPacket.B_FLAG, last()[0]);
        controller.layout().spec(PadLayout.Control.B).hidden = true;
        controller.place();
        assertEquals(View.GONE, b.getVisibility());
        assertEquals(0, last()[0]);
        a.move(2, onB[0], onB[1]);
        assertEquals(0, last()[0]);
        a.up(2, 600);
        controller.destroy();
    }

    /** Split, the D-pad's arms are controls of their own; where two neighbours overlap a finger holds the diagonal. */
    @Test public void aSplitDpadHoldsADiagonalWhereTwoArmsOverlap() {
        ModernController controller = splitController(PadLayout.Control.DPAD);
        PadControlView up = controller.view(PadLayout.Control.DPAD_UP), right = controller.view(PadLayout.Control.DPAD_RIGHT);
        up.down(0, up.getWidth() / 2f, up.getHeight() / 2f, 100);
        assertEquals(ControllerPacket.UP_FLAG, last()[0]);
        int overlapLeft = Math.max(up.getLeft(), right.getLeft()), overlapTop = Math.max(up.getTop(), right.getTop());
        assertTrue("Up's and right's boxes overlap", overlapLeft < Math.min(up.getRight(), right.getRight()) &&
                overlapTop < Math.min(up.getBottom(), right.getBottom()));
        float[] corner = in(up, overlapLeft + 1, overlapTop + 1);
        up.move(0, corner[0], corner[1]);
        assertEquals("The diagonal", ControllerPacket.UP_FLAG | ControllerPacket.RIGHT_FLAG, last()[0]);
        float[] onRight = in(up, right.getLeft() + right.getWidth() - 4, right.getTop() + right.getHeight() / 2f);
        up.move(0, onRight[0], onRight[1]);
        assertEquals(ControllerPacket.RIGHT_FLAG, last()[0]);
        // The middle, between the four arms, holds nothing.
        PadControlView left = controller.view(PadLayout.Control.DPAD_LEFT);
        float[] middle = in(up, (up.getLeft() + up.getRight()) / 2f, (left.getTop() + left.getBottom()) / 2f);
        up.move(0, middle[0], middle[1]);
        assertEquals(0, last()[0]);
        up.up(0, 300);
        controller.destroy();
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
        assertEquals("L3, R3 and the halo layer under them", 3, frame.getChildCount());
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
}
