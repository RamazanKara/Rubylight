package com.limelight.binding.input.touch;

import android.app.Application;
import android.os.Looper;
import android.view.View;

import com.limelight.PcViewLifecycleTest;
import com.limelight.nvstream.NvConnection;
import com.limelight.nvstream.StreamConfiguration;
import com.limelight.nvstream.http.ComputerDetails;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.Shadows;
import org.robolectric.annotation.Config;

import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collections;
import java.util.List;
import java.util.concurrent.TimeUnit;

import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class,
        shadows = PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class AbsoluteTouchContextTest {
    @Test public void stationaryTapWaitsForReleaseThenSendsAPollableLeftClick() {
        for (int duration : new int[] {50, 200, 640}) {
            RecordingConnection connection = new RecordingConnection();
            AbsoluteTouchContext touch = context(connection, 1);
            touch.touchDownEvent(200, 150, 1000, true);
            idle(duration);
            assertTrue(connection.buttons.isEmpty());
            touch.touchUpEvent(200, 150, 1000 + duration);
            assertEquals(Collections.singletonList("down 1"), connection.buttons);
            idle(99);
            assertEquals(Collections.singletonList("down 1"), connection.buttons);
            idle(1);
            assertEquals(Arrays.asList("down 1", "up 1"), connection.buttons);
            assertArrayEquals(new int[] {200, 150}, connection.positions.get(0));
            touch.cancelTouch();
            assertEquals(2, connection.buttons.size());
        }
    }

    @Test public void holdPositionsAndPressesOnlyRightThenSupportsRightDragging() {
        RecordingConnection connection = new RecordingConnection();
        AbsoluteTouchContext touch = context(connection, 1);
        touch.touchDownEvent(200, 150, 1000, true);
        idle(649);
        assertTrue(connection.buttons.isEmpty());
        idle(1);
        assertEquals(Arrays.asList("position 200 150", "down 3"), connection.events);
        touch.touchMoveEvent(400, 250, 1700);
        touch.touchUpEvent(400, 250, 1750);
        idle(1000);
        touch.cancelTouch();
        assertEquals(Arrays.asList("down 3", "up 3"), connection.buttons);
        assertArrayEquals(new int[] {400, 250}, connection.positions.get(1));
    }

    @Test @Config(qualifiers = "560dpi")
    public void highDensityHoldToleratesWobbleInScreenDpEvenWhenZoomed() {
        assertEquals(3.5f, RuntimeEnvironment.getApplication().getResources().getDisplayMetrics().density, 0);
        for (double scale : new double[] {1, 4}) {
            RecordingConnection connection = new RecordingConnection();
            AbsoluteTouchContext touch = context(connection, scale);
            touch.touchDownEvent(200, 150, 1000, true);
            idle(100);
            // 80 screen pixels is less than the 30 dp hold radius (105 px).
            touch.touchMoveEvent(200 + (int) (80 / scale), 150, 1100);
            assertTrue(connection.buttons.isEmpty());
            idle(550);
            touch.touchUpEvent(200, 150, 1700);
            assertEquals(Arrays.asList("down 3", "up 3"), connection.buttons);
        }
    }

    @Test public void crossingHoldSlopStartsLeftDragAtOriginalPositionAndCannotLaterRightClick() {
        for (double scale : new double[] {1, 4}) {
            RecordingConnection connection = new RecordingConnection();
            AbsoluteTouchContext touch = context(connection, scale);
            touch.touchDownEvent(200, 150, 1000, true);
            int moved = 200 + (int) (32 / scale);
            touch.touchMoveEvent(moved, 150, 1050);
            assertEquals(Arrays.asList("position 200 150", "down 1", "position " + moved + " 150"),
                    connection.events);
            idle(1000);
            assertEquals(Collections.singletonList("down 1"), connection.buttons);
            touch.touchUpEvent(moved, 150, 2000);
            touch.cancelTouch();
            assertEquals(Arrays.asList("down 1", "up 1"), connection.buttons);
        }
    }

    @Test public void motionInsideHoldRadiusDoesNotPrematurelyPressLeft() {
        RecordingConnection connection = new RecordingConnection();
        AbsoluteTouchContext touch = context(connection, 1);
        touch.touchDownEvent(200, 150, 1000, true);
        touch.touchMoveEvent(225, 150, 1100);
        idle(650);
        touch.touchUpEvent(225, 150, 1700);
        assertEquals(Arrays.asList("down 3", "up 3"), connection.buttons);
    }

    @Test public void finalPositionWithoutAMoveEventIsHandledAsALeftDrag() {
        RecordingConnection connection = new RecordingConnection();
        AbsoluteTouchContext touch = context(connection, 1);
        touch.touchDownEvent(200, 150, 1000, true);
        touch.touchUpEvent(300, 150, 1050);
        idle(1000);
        assertEquals(Arrays.asList("position 200 150", "down 1", "position 300 150", "up 1"), connection.events);
    }

    @Test public void doubleTapsKeepTheClickPositionAndSeparateBothPresses() {
        RecordingConnection connection = new RecordingConnection();
        AbsoluteTouchContext touch = context(connection, 1);
        touch.touchDownEvent(200, 150, 1000, true);
        touch.touchUpEvent(200, 150, 1050);
        touch.touchDownEvent(210, 150, 1100, true);
        touch.touchUpEvent(210, 150, 1150);
        idle(100);
        assertEquals(Arrays.asList("down 1", "up 1", "down 1", "up 1"), connection.buttons);
        assertEquals(1, connection.positions.size());
    }

    @Test public void holdingAfterATapUsesTheNewLocationWithoutAnotherLeftClick() {
        RecordingConnection connection = new RecordingConnection();
        AbsoluteTouchContext touch = context(connection, 1);
        touch.touchDownEvent(200, 150, 1000, true);
        touch.touchUpEvent(200, 150, 1050);
        touch.touchDownEvent(220, 150, 1100, true);
        idle(650);
        touch.touchUpEvent(220, 150, 1800);
        assertEquals(Arrays.asList("down 1", "up 1", "down 3", "up 3"), connection.buttons);
        assertArrayEquals(new int[] {220, 150}, connection.positions.get(1));
    }

    @Test public void cancellationAndSecondFingerReleaseOnlyButtonsActuallyPressed() {
        for (boolean secondFinger : new boolean[] {false, true}) {
            for (String state : new String[] {"pending", "tap", "drag", "hold"}) {
                RecordingConnection connection = new RecordingConnection();
                AbsoluteTouchContext touch = context(connection, 1);
                touch.touchDownEvent(200, 150, 1000, true);
                if (state.equals("tap")) touch.touchUpEvent(200, 150, 1050);
                else if (state.equals("drag")) touch.touchMoveEvent(300, 150, 1050);
                else if (state.equals("hold")) idle(650);
                if (secondFinger) touch.setPointerCount(2);
                else touch.cancelTouch();
                touch.touchUpEvent(200, 150, 1800);
                touch.cancelTouch();
                idle(1000);
                assertEquals(state, state.equals("pending") ? Collections.emptyList() :
                        state.equals("hold") ? Arrays.asList("down 3", "up 3") :
                                Arrays.asList("down 1", "up 1"), connection.buttons);
            }
        }
    }

    private static AbsoluteTouchContext context(RecordingConnection connection, double scale) {
        View view = new View(RuntimeEnvironment.getApplication());
        view.layout(0, 0, 800, 400);
        return new AbsoluteTouchContext(connection, 0, view, () -> scale);
    }

    private static void idle(long ms) { Shadows.shadowOf(Looper.getMainLooper()).idleFor(ms, TimeUnit.MILLISECONDS); }

    private static class RecordingConnection extends NvConnection {
        final List<String> buttons = new ArrayList<>(), events = new ArrayList<>();
        final List<int[]> positions = new ArrayList<>();

        RecordingConnection() {
            super(null, new ComputerDetails.AddressTuple("127.0.0.1", 47989), 47984, "test",
                    new StreamConfiguration.Builder().build(), null, null);
        }

        @Override public void sendMousePosition(short x, short y, short width, short height) {
            positions.add(new int[] {x, y});
            events.add("position " + x + " " + y);
        }
        @Override public void sendMouseButtonDown(byte button) {
            buttons.add("down " + button); events.add("down " + button);
        }
        @Override public void sendMouseButtonUp(byte button) {
            buttons.add("up " + button); events.add("up " + button);
        }
    }
}
