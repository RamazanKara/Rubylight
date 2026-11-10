package com.limelight.binding.input.touch;

import static org.junit.Assert.*;

import android.app.Application;
import android.os.Looper;

import com.limelight.nvstream.NvConnection;
import com.limelight.nvstream.StreamConfiguration;
import com.limelight.nvstream.http.ComputerDetails;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.Shadows;
import org.robolectric.annotation.Config;

import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collections;
import java.util.List;
import java.util.concurrent.TimeUnit;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class TrackpadContextTest {
    @Test
    public void tappingClicksWithoutPressingTheTouchpad() {
        Recorder connection = new Recorder();
        TrackpadContext context = new TrackpadContext(connection, 0, false, 100, 100, 100, null);
        context.setPointerCount(1);
        context.touchDownEvent(500, 500, 1000, true);
        context.touchUpEvent(500, 500, 1060);
        assertEquals(Collections.singletonList("down 1"), connection.buttons);
        Shadows.shadowOf(Looper.getMainLooper()).idleFor(400, TimeUnit.MILLISECONDS);
        assertEquals(Arrays.asList("down 1", "up 1"), connection.buttons);
        assertTrue(connection.moves.isEmpty());
    }

    @Test
    public void aNegativeSensitivityFlipsThatAxisOnly() {
        int[] normal = drag(false, 100, 100, 100);
        int[] flipped = drag(false, -100, 100, 100);
        assertTrue(normal[0] > 0);
        assertEquals(-normal[0], flipped[0]);
        assertEquals(normal[1], flipped[1]);
    }

    @Test
    public void sensitivityAndMouseSpeedScaleMovement() {
        int[] normal = drag(false, 100, 100, 100);
        int[] half = drag(false, 50, 100, 100);
        int[] fast = drag(false, 100, 100, 200);
        assertTrue(Math.abs(half[0] - normal[0] / 2) <= 2);
        assertTrue(Math.abs(fast[0] - normal[0] * 2) <= 4);
    }

    @Test
    public void swappingTheAxesTurnsHorizontalMovementIntoVertical() {
        int[] normal = drag(false, 100, 100, 100);
        int[] swapped = drag(true, 100, 100, 100);
        assertEquals(normal[0], swapped[1]);
        assertEquals(normal[1], swapped[0]);
    }

    @Test
    public void twoFingerSwipeScrollsInsteadOfMoving() {
        Recorder connection = new Recorder();
        TrackpadContext first = new TrackpadContext(connection, 0, false, 100, 100, 100, null);
        TrackpadContext second = new TrackpadContext(connection, 1, false, 100, 100, 100, null);
        first.setPointerCount(1);
        first.touchDownEvent(500, 500, 1000, true);
        first.setPointerCount(2);
        second.setPointerCount(2);
        second.touchDownEvent(600, 500, 1005, true);
        for (int i = 1; i <= 6; i++) {
            first.touchMoveEvent(500, 500 + i * 30, 1005 + i * 10);
            second.touchMoveEvent(600, 500 + i * 30, 1005 + i * 10);
        }
        assertFalse(connection.scrolls.isEmpty());
        assertTrue(connection.moves.isEmpty());
    }

    @Test
    public void aTouchscreenScaleShrinksMovementButNotScrolling() {
        Recorder connection = new Recorder();
        TrackpadContext context = new TrackpadContext(connection, 0, false, 1920, 1080, new TrackpadContext.MoveScale() {
            @Override public float x() { return 0.5f; }
            @Override public float y() { return 0.5f; }
        });
        context.setPointerCount(1);
        context.touchDownEvent(500, 500, 1000, true);
        for (int i = 1; i <= 4; i++) {
            context.touchMoveEvent(500 + i * 10, 500, 1000 + i * 10);
        }
        int x = 0;
        for (int[] move : connection.moves) x += move[0];
        int[] unscaled = drag(false, 100, 100, 100);
        assertTrue(Math.abs(x - unscaled[0] / 2) <= 2);
    }

    /** Drags one finger 40 units right and returns what the host received, summed. */
    private static int[] drag(boolean swap, int sensitivityX, int sensitivityY, int speed) {
        Recorder connection = new Recorder();
        TrackpadContext context = new TrackpadContext(connection, 0, swap, sensitivityX, sensitivityY, speed, null);
        context.setPointerCount(1);
        context.touchDownEvent(500, 500, 1000, true);
        for (int i = 1; i <= 4; i++) {
            context.touchMoveEvent(500 + i * 10, 500, 1000 + i * 10);
        }
        int x = 0;
        int y = 0;
        for (int[] move : connection.moves) {
            x += move[0];
            y += move[1];
        }
        return new int[] {x, y};
    }

    private static class Recorder extends NvConnection {
        final List<int[]> moves = new ArrayList<>();
        final List<String> buttons = new ArrayList<>();
        final List<Short> scrolls = new ArrayList<>();

        Recorder() {
            super(null, new ComputerDetails.AddressTuple("127.0.0.1", 47989), 47984, "test",
                    new StreamConfiguration.Builder().build(), null, null);
        }

        @Override public boolean canSendInput() { return true; }
        @Override public void sendMouseMove(short x, short y) { moves.add(new int[] {x, y}); }
        @Override public void sendMouseButtonDown(byte button) { buttons.add("down " + button); }
        @Override public void sendMouseButtonUp(byte button) { buttons.add("up " + button); }
        @Override public void sendMouseHighResScroll(short amount) { scrolls.add(amount); }
    }
}
