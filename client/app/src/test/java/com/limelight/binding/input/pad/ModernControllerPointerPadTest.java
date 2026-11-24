package com.limelight.binding.input.pad;

import android.app.Application;
import android.view.MotionEvent;
import android.view.View;

import com.limelight.nvstream.input.MouseButtonPacket;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;
import org.robolectric.util.ReflectionHelpers;

import static org.junit.Assert.*;

/**
 * {@link ModernController} pointer touchpad tests: mouse and camera modes, multi-finger tap and
 * scroll, trackpad speed and acceleration, historical event timing, and finger trail fading.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 30, application = Application.class, qualifiers = "w800dp-h360dp-land-xhdpi")
public class ModernControllerPointerPadTest extends ModernControllerHarness {
    @Test public void aPointerTouchpadDragMovesTheMouseAndATapClicks() {
        prefs.trackpadAcceleration = true;
        pointerMoves.clear();
        pointerButtons.clear();
        PadControlView pad = pointerController(false).view(PadLayout.Control.POINTER_PAD);
        assertEquals(View.VISIBLE, pad.getVisibility());
        assertEquals("The pad says what it does", "MOUSE", pad.state().label);
        float x = pad.getWidth() / 2f, y = pad.getHeight() / 2f;

        pad.down(0, x, y, 1000);
        // A gesture starts at the slow end, so acceleration shows from the second move on.
        pad.move(0, x + 20, y, 1_010_000_000L);
        pad.move(0, x + 40, y, 1_020_000_000L);
        assertEquals("Both moves reach the pointer", 2, pointerMoves.size());
        int first = pointerMoves.get(0)[0], second = pointerMoves.get(1)[0];
        assertTrue("A faster move is accelerated: " + first + " then " + second, second > first);
        assertTrue("and passes the finger's own 20 px", second > 20);
        pad.up(0, 1200);
        assertTrue("A drag is not a click", pointerButtons.isEmpty());

        pad.down(0, x, y, 2000);
        pad.up(0, 2080);
        assertEquals(2, pointerButtons.size());
        assertEquals(Integer.valueOf(MouseButtonPacket.BUTTON_LEFT), pointerButtons.get(0));
        assertEquals(Integer.valueOf(-MouseButtonPacket.BUTTON_LEFT), pointerButtons.get(1));
    }

    @Test public void twoFingersOnThePointerTouchpadTapRightAndDragToScroll() {
        pointerButtons.clear();
        pointerScrolls.clear();
        PadControlView pad = pointerController(false).view(PadLayout.Control.POINTER_PAD);
        float x = pad.getWidth() / 2f, y = pad.getHeight() / 2f;

        pad.down(0, x, y, 1000);
        pad.down(1, x + 30, y, 1050);
        pad.up(1, 1100);
        pad.up(0, 1150);
        assertEquals("Two fingers tap the right button", 2, pointerButtons.size());
        assertEquals(Integer.valueOf(MouseButtonPacket.BUTTON_RIGHT), pointerButtons.get(0));

        pointerButtons.clear();
        pad.down(0, x, y, 2000);
        pad.down(1, x + 30, y, 2050);
        pad.move(1, x + 30, y + 80, 2_100_000_000L);
        pad.up(1, 2200);
        pad.up(0, 2250);
        assertFalse("Two fingers dragging scroll", pointerScrolls.isEmpty());
        assertTrue(pointerButtons.isEmpty());
    }

    @Test public void thePointerTouchpadAsACameraPushesTheRightStickAndLetsGo() {
        ModernController controller = pointerController(true);
        PadControlView pad = controller.view(PadLayout.Control.POINTER_PAD);
        assertEquals("The pad says it is the camera", "CAM", pad.state().label);
        float x = pad.getWidth() / 2f, y = pad.getHeight() / 2f;
        pad.down(0, x, y, 1000);
        // The camera starts at rest like the pointer: its speed needs an interval behind it.
        pad.move(0, x + 50, y, 1_005_000_000L);
        pad.move(0, x + 100, y, 1_010_000_000L);
        assertTrue("A fast drag turns right", last()[3] > 0);
        assertEquals(0, last()[4]);
        pad.release();
        assertEquals("Letting go centers the camera", 0, last()[3] | last()[4]);
    }

    /** Trackpad speed reaches the touchpad's Camera mode: the same drag turns further at 200 % than at 100 %. */
    @Test public void thePointerTouchpadAsACameraFollowsTrackpadSpeed() {
        int[] turned = new int[2];
        int[] speeds = {100, 200};
        for (int i = 0; i < speeds.length; i++) {
            prefs.trackpadSpeed = speeds[i];
            ModernController controller = pointerController(true);
            PadControlView pad = controller.view(PadLayout.Control.POINTER_PAD);
            float x = pad.getWidth() / 2f, y = pad.getHeight() / 2f;
            pad.down(0, x, y, 1000);
            pad.move(0, x + 4, y, 1_010_000_000L);
            pad.move(0, x + 8, y, 1_020_000_000L);
            turned[i] = last()[3];
            pad.release();
            controller.destroy();
        }
        assertTrue("A drag turns the camera", turned[0] > 0);
        assertTrue("and turns it further at 200 %: " + turned[0] + " vs " + turned[1], turned[1] > turned[0]);
    }

    /** M6: a scroll is never a right click, even when the first finger lifts before the scroll starts. */
    @Test public void aScrollNeverRightClicksAfterTheFirstFingerLifts() {
        pointerButtons.clear();
        pointerScrolls.clear();
        PadControlView pad = pointerController(false).view(PadLayout.Control.POINTER_PAD);
        float x = pad.getWidth() / 2f, y = pad.getHeight() / 2f;

        pad.down(0, x, y, 1000);
        pad.down(1, x + 30, y, 1020);
        pad.up(0, 1080);
        pad.move(1, x + 30, y + 80, 1_100_000_000L);
        pad.up(1, 1200);
        assertFalse("The second finger scrolls", pointerScrolls.isEmpty());
        assertTrue("and the scroll does not right-click: " + pointerButtons, pointerButtons.isEmpty());
    }

    /** M6: a finger that stays after its partner lifts is not a two-finger tap. */
    @Test public void aLingeringFingerIsNotATwoFingerTap() {
        pointerButtons.clear();
        PadControlView pad = pointerController(false).view(PadLayout.Control.POINTER_PAD);
        float x = pad.getWidth() / 2f, y = pad.getHeight() / 2f;

        pad.down(0, x, y, 1000);
        pad.down(1, x + 30, y, 1010);
        pad.up(0, 1090);
        pad.up(1, 1400);
        assertTrue("A late lift is not a tap: " + pointerButtons, pointerButtons.isEmpty());
    }

    /** M3: other controls report once per event; only the pointer touchpad needs every historical sample. */
    @Test public void historicalSamplesChangeOnlyThePointerTouchpad() {
        ModernController controller = pointerController(false);
        PadControlView pad = controller.view(PadLayout.Control.POINTER_PAD);
        float x = pad.getWidth() / 2f, y = pad.getHeight() / 2f;
        pad.down(0, x, y, 1000);
        pointerMoves.clear();
        MotionEvent drag = MotionEvent.obtain(1000, 1010, MotionEvent.ACTION_MOVE, x + 60, y, 0);
        drag.addBatch(1005, x + 30, y, 1f, 1f, 0);
        assertEquals("The event carries a historical sample", 1, drag.getHistorySize());
        pad.onTouchEvent(drag);
        drag.recycle();
        assertTrue("The pointer pad uses each sample's own time: " + pointerMoves.size(), pointerMoves.size() >= 2);

        PadControlView stick = controller.view(PadLayout.Control.LEFT_STICK);
        float size = Math.min(stick.getWidth(), stick.getHeight()), travel = PadPainter.travel(size);
        float cx = stick.getWidth() / 2f, cy = stick.getHeight() / 2f;
        long now = android.os.SystemClock.uptimeMillis();
        MotionEvent down = MotionEvent.obtain(now, now, MotionEvent.ACTION_DOWN, cx, cy, 0);
        stick.onTouchEvent(down);
        down.recycle();
        int before = reports.size();
        MotionEvent move = MotionEvent.obtain(now, now + 10, MotionEvent.ACTION_MOVE, cx + travel, cy, 0);
        move.addBatch(now + 5, cx + travel / 2, cy, 1f, 1f, 0);
        stick.onTouchEvent(move);
        move.recycle();
        assertEquals("A stick reports once for the event, as it always has", 1, reports.size() - before);
    }

    /** L1: Trackpad acceleration off is linear for the pointer touchpad, as it is for the screen's own touchpad. */
    @Test public void withTrackpadAccelerationOffThePointerTouchpadPassesTheFingerThrough() {
        prefs.trackpadAcceleration = false;
        pointerMoves.clear();
        PadControlView pad = pointerController(false).view(PadLayout.Control.POINTER_PAD);
        float x = pad.getWidth() / 2f, y = pad.getHeight() / 2f;
        pad.down(0, x, y, 1000);
        pad.move(0, x + 20, y, 1_010_000_000L);
        assertEquals("The finger's own 20 px, not the medium curve's 12", 20, pointerMoves.get(0)[0]);
        pad.release();
    }

    /**
     * M2: every sample is stamped with its own event time. A late frame (a dispatch delay) must not read as a slow
     * finger, so the speed comes from the 5 ms between the two samples. Robolectric's MotionEvent reports whole
     * milliseconds, so this runs below Android 14; the nanosecond times above it are the same times, wider.
     */
    @Test
    public void thePointerTouchpadsSpeedFollowsEventTimesThroughDispatchJitter() {
        PadControlView pad = pointerController(false).view(PadLayout.Control.POINTER_PAD);
        float x = pad.getWidth() / 2f, y = pad.getHeight() / 2f;
        pad.down(0, x, y, 1000);
        PadPointerMotion motion = ReflectionHelpers.getField(pad, "pointerMotion");
        MotionEvent drag = MotionEvent.obtain(1000, 1005, MotionEvent.ACTION_MOVE, x + 30, y, 0);
        drag.addBatch(1010, x + 60, y, 1f, 1f, 0);
        assertEquals("The event carries one historical sample", 1, drag.getHistorySize());
        pad.onTouchEvent(drag);
        drag.recycle();
        // The historical sample starts the speed; the current one adds 30 px over 5 ms: 15 dp over 5 ms at density 2.
        assertEquals(3000f, motion.speed(), 1f);
        pad.release();
    }

    /** The pointer touchpad's finger leaves a trail behind it that fades once it stops, also after it lifts. */
    @Test public void aTouchpadFingerLeavesATrailThatFades() {
        ModernController controller = pointerController(false);
        PadControlView pad = controller.view(PadLayout.Control.POINTER_PAD);
        pad.down(0, 20, 20, 100);
        for (int i = 1; i <= 5; i++) pad.move(0, 20 + 10 * i, 20 + 4 * i, 100_000_000L + i * 16_000_000L);
        PadPainter.State state = pad.state();
        assertEquals(6, state.trailCount);
        long now = android.os.SystemClock.uptimeMillis();
        assertTrue(state.trailShows(now));
        pad.up(0, 300);
        assertTrue("Lifted, it still fades", state.trailShows(now));
        assertFalse("then it is gone", state.trailShows(now + PadPainter.TRAIL_MS));
        controller.destroy();
    }
}
