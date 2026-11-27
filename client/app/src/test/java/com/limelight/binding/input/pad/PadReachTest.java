package com.limelight.binding.input.pad;

import android.app.Application;
import android.os.SystemClock;
import android.view.MotionEvent;
import android.view.View;

import com.limelight.binding.input.SteamControls;
import com.limelight.nvstream.input.ControllerPacket;
import com.limelight.nvstream.jni.MoonBridge;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;

import java.util.ArrayList;
import java.util.List;

import static org.junit.Assert.*;

/**
 * The on-screen controls' touch area (Nightjar #172): at the default a control takes touches on its box alone, as
 * before; a larger area lets a finger around the box work it, and the layout keeps the area for older builds too.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 30, application = Application.class, qualifiers = "w800dp-h360dp-land-xhdpi")
public class PadReachTest extends ModernControllerHarness {
    private ModernController withReach(PadLayout.Control control, float reach) {
        ModernController first = controller(GAME);
        PadLayout draft = first.layout().copy();
        draft.setReach(control, reach);
        first.save(draft, false);
        first.destroy();
        return controller(GAME);
    }

    private static MotionEvent touch(long down, long at, int action, float x, float y) {
        return MotionEvent.obtain(down, at, action, x, y, 0);
    }

    @Test public void theDefaultAreaIsTheBoxAndSavesNothingNew() {
        ModernController controller = controller(GAME);
        for (PadLayout.Control control : PadLayout.Control.values()) {
            PadControlView view = controller.view(control);
            if (view != null) assertEquals(control.name(), View.GONE, view.reachLayer().getVisibility());
        }
        PadLayout draft = controller.layout().copy();
        draft.setScale(PadLayout.Control.FACE, 1.2f);
        assertEquals(PadLayout.MIN_REACH, draft.spec(PadLayout.Control.FACE).reach, 0);
        assertFalse("No ninth field without a touch area", draft.serialize().matches(".*FACE(,[^|,]*){8}.*"));
    }

    @Test public void theAreaRoundTripsAndAnOlderReaderKeepsTheRest() {
        PadLayout layout = new PadLayout();
        layout.setReach(PadLayout.Control.LB, 1.75f);
        layout.setReach(PadLayout.Control.RB, 9f);
        String text = layout.serialize();
        PadLayout back = PadLayout.parse(text);
        assertEquals(1.75f, back.spec(PadLayout.Control.LB).reach, 1e-3);
        assertEquals("kept within the slider's range", PadLayout.MAX_REACH, back.spec(PadLayout.Control.RB).reach, 1e-3);
        assertEquals(1f, back.spec(PadLayout.Control.LB).widthScale, 1e-3);
        // A build before the field reads the first eight and drops the ninth: the box stays as it was.
        PadLayout old = PadLayout.parse(text.replaceAll("(LB(,[^|,]*){7}),[^|,]*", "$1"));
        assertEquals(PadLayout.MIN_REACH, old.spec(PadLayout.Control.LB).reach, 0);
        assertEquals(1f, old.spec(PadLayout.Control.LB).scale(), 1e-3);
    }

    @Test public void aFingerBesideAButtonPressesItWithinItsArea() {
        ModernController controller = withReach(PadLayout.Control.LB, 2f);
        PadControlView lb = controller.view(PadLayout.Control.LB);
        View area = lb.reachLayer();
        assertEquals(View.VISIBLE, area.getVisibility());
        assertTrue("the area is larger than the box", area.getWidth() > lb.getWidth() && area.getHeight() > lb.getHeight());
        long t = SystemClock.uptimeMillis();
        float x = (lb.getLeft() - area.getLeft()) / 2f, y = area.getHeight() / 2f;
        area.dispatchTouchEvent(touch(t, t, MotionEvent.ACTION_DOWN, x, y));
        assertNotEquals("LB holds from beside its box", 0, last()[0]);
        area.dispatchTouchEvent(touch(t, t + 50, MotionEvent.ACTION_UP, x, y));
        assertEquals("and lets go with the finger", 0, last()[0]);
    }

    /** #150 holds in the area too: the touch goes down at zero pressure, and a tap clicks by a click-pressure MOVE. */
    @Test public void aTapInATrackpadsAreaClicksItAsOnThePadItself() {
        prefs.oscSteamControls = true;
        ModernController controller = withReach(PadLayout.Control.LEFT_PAD, 1.5f);
        List<float[]> touches = new ArrayList<>();
        controller.setTouchSink((type, pointer, x, y, pressure) -> touches.add(new float[] {type, x, y, pressure}));
        View area = controller.view(PadLayout.Control.LEFT_PAD).reachLayer();
        assertEquals(View.VISIBLE, area.getVisibility());
        long t = SystemClock.uptimeMillis();
        float x = 2, y = area.getHeight() / 2f;
        area.dispatchTouchEvent(touch(t, t, MotionEvent.ACTION_DOWN, x, y));
        assertFalse(touches.isEmpty());
        assertEquals(0f, touches.get(touches.size() - 1)[3], 0);
        area.dispatchTouchEvent(touch(t, t + 100, MotionEvent.ACTION_UP, x, y));
        float[] click = touches.get(touches.size() - 1);
        assertEquals(MoonBridge.LI_TOUCH_EVENT_MOVE, (int) click[0]);
        assertEquals(SteamControls.PAD_CLICK_PRESSURE, click[3], 1e-3);
        assertEquals(SteamControls.LEFT_PAD_CLICK_FLAG, last()[0] & SteamControls.LEFT_PAD_CLICK_FLAG);
    }

    private ModernController withLayout(java.util.function.Consumer<PadLayout> change) {
        ModernController first = controller(GAME);
        PadLayout draft = first.layout().copy();
        change.accept(draft);
        first.save(draft, false);
        first.destroy();
        return controller(GAME);
    }

    private void touchFrame(long down, long at, int action, float x, float y) {
        frame.dispatchTouchEvent(touch(down, at, action, x, y));
    }

    /** Where two areas overlap, the finger works the control whose box is nearer, whichever area lies on top. */
    @Test public void overlappingAreasGoToTheNearerControl() {
        float density = frame.getResources().getDisplayMetrics().density;
        ModernController controller = withLayout(draft -> {
            draft.setBounds(PadLayout.Control.LB, 740, 150, 120, 100, frame.getWidth(), frame.getHeight(), density);
            draft.setBounds(PadLayout.Control.LT, 740, 330, 120, 100, frame.getWidth(), frame.getHeight(), density);
            draft.setReach(PadLayout.Control.LB, 3f);
            draft.setReach(PadLayout.Control.LT, 3f);
        });
        PadControlView lb = controller.view(PadLayout.Control.LB), lt = controller.view(PadLayout.Control.LT);
        int gap = lt.getTop() - lb.getBottom();
        assertTrue("a gap between the boxes: " + gap, gap >= 10);
        assertTrue("within both areas", lb.getHeight() > gap && lt.getHeight() > gap);
        // The middle of the screen, where no other control's box lies.
        float x = (lb.getLeft() + lb.getRight()) / 2f;
        long t = SystemClock.uptimeMillis();
        touchFrame(t, t, MotionEvent.ACTION_DOWN, x, lt.getTop() - 3);
        assertEquals("LT, the nearer, pulls", 255, last()[5]);
        assertEquals("LB stays up", 0, last()[0] & PadInput.buttonFlag(PadLayout.Control.LB));
        touchFrame(t, t + 40, MotionEvent.ACTION_UP, x, lt.getTop() - 3);
        touchFrame(t + 80, t + 80, MotionEvent.ACTION_DOWN, x, lb.getBottom() + 3);
        assertEquals("LB, the nearer, presses", PadInput.buttonFlag(PadLayout.Control.LB),
                last()[0] & PadInput.buttonFlag(PadLayout.Control.LB));
        assertEquals("LT stays released", 0, last()[5]);
        touchFrame(t + 80, t + 120, MotionEvent.ACTION_UP, x, lb.getBottom() + 3);
        assertEquals(0, last()[0]);
    }

    /** A split D-pad's finger that lands in an arm's area presses that arm and slides on to the next as on a box. */
    @Test public void aSplitDpadsAreaPressesAnArmAndSlidesToTheNext() {
        float density = frame.getResources().getDisplayMetrics().density;
        ModernController controller = withLayout(draft -> {
            draft.split(PadLayout.Control.DPAD, frame.getWidth(), frame.getHeight(), density);
            for (PadLayout.Control arm : PadLayout.members(PadLayout.Control.DPAD)) draft.setReach(arm, 2f);
        });
        PadControlView up = controller.view(PadLayout.Control.DPAD_UP), right = controller.view(PadLayout.Control.DPAD_RIGHT);
        float x = (up.getLeft() + up.getRight()) / 2f, y = up.getTop() - 3;
        long t = SystemClock.uptimeMillis();
        touchFrame(t, t, MotionEvent.ACTION_DOWN, x, y);
        assertEquals("up, from above its box", ControllerPacket.UP_FLAG, last()[0]);
        touchFrame(t, t + 30, MotionEvent.ACTION_MOVE, right.getRight() - 4, (right.getTop() + right.getBottom()) / 2f);
        assertEquals("slid onto right", ControllerPacket.RIGHT_FLAG, last()[0]);
        touchFrame(t, t + 60, MotionEvent.ACTION_UP, right.getRight() - 4, (right.getTop() + right.getBottom()) / 2f);
        assertEquals(0, last()[0]);
    }

    /** An unreadable touch area keeps the box and the rest of the entry, as a broken scale does. */
    @Test public void anUnreadableAreaKeepsTheEntry() {
        PadLayout layout = new PadLayout();
        layout.setReach(PadLayout.Control.LB, 2f);
        layout.setOpacity(PadLayout.Control.LB, 40);
        PadLayout back = PadLayout.parse(layout.serialize().replaceAll("(LB(,[^|,]*){7}),[^|,]*", "$1,x"));
        assertEquals(PadLayout.MIN_REACH, back.spec(PadLayout.Control.LB).reach, 0);
        assertEquals(40, back.spec(PadLayout.Control.LB).opacity);
    }

    @Test public void theEditorAndHidingTakeTheAreaAway() {
        ModernController controller = withReach(PadLayout.Control.RB, 1.5f);
        View area = controller.view(PadLayout.Control.RB).reachLayer();
        assertEquals(View.VISIBLE, area.getVisibility());
        controller.startEditing();
        assertEquals(View.GONE, area.getVisibility());
        controller.finishEditing(false);
        assertEquals(View.VISIBLE, area.getVisibility());
        controller.hide();
        assertEquals(View.GONE, area.getVisibility());
    }

    @Test public void aSwipeInATrackpadsAreaMovesWithoutClickingAndCancelledStickSkipsDoubleTap() {
        prefs.oscSteamControls = true;
        ModernController controller = withLayout(draft -> {
            draft.setReach(PadLayout.Control.LEFT_PAD, 2f);
            draft.setReach(PadLayout.Control.LEFT_STICK, 2f);
        });
        List<float[]> touches = new ArrayList<>();
        controller.setTouchSink((type, pointer, x, y, pressure) -> touches.add(new float[] {type, x, y, pressure}));
        PadControlView pad = controller.view(PadLayout.Control.LEFT_PAD);
        View padArea = pad.reachLayer();
        long t = SystemClock.uptimeMillis();
        // Swipe outward inside the left reach margin: real finger travel suppresses tap-to-click.
        padArea.dispatchTouchEvent(touch(t, t, MotionEvent.ACTION_DOWN, 40f, padArea.getHeight() / 2f));
        padArea.dispatchTouchEvent(touch(t, t + 30, MotionEvent.ACTION_MOVE, 2f, padArea.getHeight() / 2f));
        padArea.dispatchTouchEvent(touch(t, t + 60, MotionEvent.ACTION_UP, 2f, padArea.getHeight() / 2f));
        assertEquals("Outward swipe in reach area does not click the Steam trackpad",
                0, last()[0] & SteamControls.LEFT_PAD_CLICK_FLAG);

        // Cancelled stick touch in reach area must not arm double-tap L3 for the next touch.
        PadControlView stick = controller.view(PadLayout.Control.LEFT_STICK);
        View stickArea = stick.reachLayer();
        long t2 = t + 500;
        stickArea.dispatchTouchEvent(touch(t2, t2, MotionEvent.ACTION_DOWN, 2f, stickArea.getHeight() / 2f));
        stickArea.dispatchTouchEvent(touch(t2, t2 + 20, MotionEvent.ACTION_CANCEL, 2f, stickArea.getHeight() / 2f));
        stickArea.dispatchTouchEvent(touch(t2 + 60, t2 + 60, MotionEvent.ACTION_DOWN, 2f, stickArea.getHeight() / 2f));
        assertEquals("Cancelled reach touch does not arm double-tap L3",
                0, last()[0] & ControllerPacket.LS_CLK_FLAG);
        stickArea.dispatchTouchEvent(touch(t2 + 60, t2 + 90, MotionEvent.ACTION_UP, 2f, stickArea.getHeight() / 2f));
    }

    @Test public void releaseHidingAndDetachingClearTrackedReachPointers() {
        ModernController controller = withLayout(draft -> {
            draft.setReach(PadLayout.Control.LEFT_STICK, 2f);
            draft.setReach(PadLayout.Control.LB, 2f);
        });
        PadControlView stick = controller.view(PadLayout.Control.LEFT_STICK);
        PadReachLayer stickArea = stick.reachLayer();
        long t = SystemClock.uptimeMillis();
        // Start a stick gesture in the reach area, then release the control without an ACTION_CANCEL on the reach layer.
        stickArea.dispatchTouchEvent(touch(t, t, MotionEvent.ACTION_DOWN, 2f, stickArea.getHeight() / 2f));
        assertEquals(1, stickArea.trackedPointerCount());
        stick.release();
        assertEquals("release() clears tracked reach pointers", 0, stickArea.trackedPointerCount());
        // A stale MOVE after release() must not deflect the stick.
        stickArea.dispatchTouchEvent(touch(t, t + 20, MotionEvent.ACTION_MOVE, stickArea.getWidth() - 2f, stickArea.getHeight() / 2f));
        assertEquals(0, last()[1]);
        assertEquals(0, last()[2]);

        // Hiding the reach layer clears pointers and releases the button it held.
        PadControlView lb = controller.view(PadLayout.Control.LB);
        PadReachLayer lbArea = lb.reachLayer();
        long t2 = t + 100;
        lbArea.dispatchTouchEvent(touch(t2, t2, MotionEvent.ACTION_DOWN, 2f, lbArea.getHeight() / 2f));
        assertNotEquals(0, last()[0] & PadInput.buttonFlag(PadLayout.Control.LB));
        assertEquals(1, lbArea.trackedPointerCount());
        lbArea.setVisibility(View.GONE);
        assertEquals("hiding clears tracked pointers", 0, lbArea.trackedPointerCount());
        assertEquals("hiding releases the held button", 0, last()[0] & PadInput.buttonFlag(PadLayout.Control.LB));

        // Detaching the reach layer from the window also clears pointers and releases the button it held.
        lbArea.setVisibility(View.VISIBLE);
        long t3 = t + 200;
        lbArea.dispatchTouchEvent(touch(t3, t3, MotionEvent.ACTION_DOWN, 2f, lbArea.getHeight() / 2f));
        assertNotEquals(0, last()[0] & PadInput.buttonFlag(PadLayout.Control.LB));
        assertEquals(1, lbArea.trackedPointerCount());
        lbArea.onDetachedFromWindow();
        assertEquals("detaching clears tracked pointers", 0, lbArea.trackedPointerCount());
        assertEquals("detaching releases the held button", 0, last()[0] & PadInput.buttonFlag(PadLayout.Control.LB));
    }
}
