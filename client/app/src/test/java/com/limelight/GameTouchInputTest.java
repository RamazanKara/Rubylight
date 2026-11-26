package com.limelight;

import android.app.Application;
import android.os.Looper;
import android.view.InputDevice;
import android.view.MotionEvent;
import android.view.View;
import android.widget.FrameLayout;

import com.limelight.binding.input.HeldInputState;
import com.limelight.binding.input.TouchLayout;
import com.limelight.binding.input.touch.TouchMode;
import com.limelight.binding.input.touch.TrackpadPointer;
import com.limelight.nvstream.NvConnection;
import com.limelight.nvstream.StreamConfiguration;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.preferences.PreferenceConfiguration;
import com.limelight.ui.GameTouchControls;
import com.limelight.ui.StreamView;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.Shadows;
import org.robolectric.annotation.Config;
import org.robolectric.util.ReflectionHelpers;

import java.util.ArrayList;
import java.util.Collections;
import java.util.HashSet;
import java.util.List;
import java.util.Set;
import java.util.concurrent.TimeUnit;

import static org.junit.Assert.*;

/**
 * Tests direct/native touch coordinate mapping, touch modes, trackpad momentum, touchpad drag,
 * pointer touchpad scaling, and shared touch test fixtures for {@link Game}.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class,
        shadows = PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class GameTouchInputTest {
    @Test
    public void directPointingAccountsForCutoutAndFoldPaneOrigins() {
        for (int[] origin : new int[][] {{0, 0}, {48, 24}, {1050, 0}, {0, 600}}) {
            RecordingConnection connection = new RecordingConnection();
            Game game = game(connection);
            View background = layout(game, origin[0], origin[1]);
            ReflectionHelpers.callInstanceMethod(game.touchInput, "initializeTouchContexts");
            touch(game, background, MotionEvent.ACTION_DOWN, 500, 250);
            touch(game, background, MotionEvent.ACTION_UP, 500, 250);
            assertEquals(1, connection.positions.size());
            assertArrayEquals(new int[] {400, 200, 800, 400}, connection.positions.get(0));
            Shadows.shadowOf(Looper.getMainLooper()).idleFor(100, TimeUnit.MILLISECONDS);
        }
    }

    @Test
    public void directDragAndLetterboxClampingUseTheSameCoordinates() {
        RecordingConnection connection = new RecordingConnection();
        Game game = game(connection);
        View background = layout(game, 1050, 40);
        ReflectionHelpers.callInstanceMethod(game.touchInput, "initializeTouchContexts");
        touch(game, background, MotionEvent.ACTION_DOWN, 500, 250);
        touch(game, background, MotionEvent.ACTION_MOVE, 950, 490);
        touch(game, background, MotionEvent.ACTION_MOVE, 10, 10);
        touch(game, background, MotionEvent.ACTION_UP, 10, 10);
        assertArrayEquals(new int[] {400, 200, 800, 400}, connection.positions.get(0));
        assertArrayEquals(new int[] {800, 400, 800, 400}, connection.positions.get(1));
        assertArrayEquals(new int[] {0, 0, 800, 400}, connection.positions.get(2));
    }

    @Test
    public void nativeTouchUsesTheSamePaddedPaneMapping() {
        RecordingConnection connection = new RecordingConnection();
        Game game = game(connection);
        View background = layout(game, 1050, 600);
        ReflectionHelpers.setField(game, "nativeTouchEnabled", true);
        touch(game, background, MotionEvent.ACTION_DOWN, 500, 250);
        touch(game, background, MotionEvent.ACTION_UP, 500, 250);
        assertEquals(2, connection.touches.size());
        assertArrayEquals(new float[] {0.5f, 0.5f}, connection.touches.get(0), 0.00001f);
        assertArrayEquals(new float[] {0.5f, 0.5f}, connection.touches.get(1), 0.00001f);
        assertTrue(connection.positions.isEmpty());
    }

    @Test
    public void desktopZoomMapsDirectAndNativeInputBackIntoTheSameHostCoordinates() {
        for (boolean nativeTouch : new boolean[] {false, true}) {
            RecordingConnection connection = new RecordingConnection();
            Game game = game(connection);
            View background = layout(game, 1050, 600);
            StreamView stream = ReflectionHelpers.getField(game, "streamView");
            com.limelight.ui.DesktopGestures desktop = new com.limelight.ui.DesktopGestures(game, stream, () -> { }, () -> { });
            desktop.setEnabled(true); desktop.viewport().resize(800, 400);
            desktop.viewport().zoom(2, 400, 200); desktop.viewport().pan(100, 0);
            ReflectionHelpers.setField(game, "desktopGestures", desktop);
            ReflectionHelpers.setField(game, "nativeTouchEnabled", nativeTouch);
            ReflectionHelpers.callInstanceMethod(game.touchInput, "initializeTouchContexts");
            touch(game, background, MotionEvent.ACTION_DOWN, 500, 250);
            touch(game, background, MotionEvent.ACTION_UP, 500, 250);
            if (nativeTouch) assertArrayEquals(new float[] {.4375f, .5f}, connection.touches.get(0), .00001f);
            else assertArrayEquals(new int[] {350, 200, 800, 400}, connection.positions.get(0));
            Shadows.shadowOf(Looper.getMainLooper()).idleFor(100, TimeUnit.MILLISECONDS);
        }
    }

    @Test
    public void showingOrHidingTheTouchControlsMidGestureLeavesNoHostTouchPressed() {
        // PiP, the stream menu or the running game's own layout can flip the layer under a finger.
        for (boolean hide : new boolean[] {true, false}) {
            for (boolean firstFingerLiftsFirst : new boolean[] {true, false}) {
                RecordingConnection connection = new RecordingConnection();
                Game game = game(connection);
                ReflectionHelpers.setField(game, "nativeTouchEnabled", true);
                GameTouchControls controls = touchControls(game);
                FrameLayout container = streamContainer(game, controls);
                controls.hideForPip(!hide);
                float[] aim = {600, 300}, moved = {610, 300}, other = {500, 200};
                int[] first = {0}, second = {1}, two = {0, 1};
                String flip = (hide ? "hiding" : "showing") +
                        (firstFingerLiftsFirst ? ", first finger lifts first" : ", second finger lifts first");

                dispatch(container, MotionEvent.ACTION_DOWN, 0, first, aim);
                controls.hideForPip(hide);
                dispatch(container, MotionEvent.ACTION_POINTER_DOWN, 1, two, aim, other);
                dispatch(container, MotionEvent.ACTION_MOVE, 0, two, moved, other);
                if (firstFingerLiftsFirst) {
                    dispatch(container, MotionEvent.ACTION_POINTER_UP, 0, two, moved, other);
                    dispatch(container, MotionEvent.ACTION_MOVE, 0, second, new float[] {520, 200});
                    dispatch(container, MotionEvent.ACTION_UP, 0, second, new float[] {520, 200});
                } else {
                    dispatch(container, MotionEvent.ACTION_POINTER_UP, 1, two, moved, other);
                    dispatch(container, MotionEvent.ACTION_MOVE, 0, first, new float[] {620, 300});
                    dispatch(container, MotionEvent.ACTION_UP, 0, first, new float[] {620, 300});
                }
                assertEquals("The host still holds a touch after " + flip, Collections.emptySet(),
                        connection.heldTouches);

                // The next gesture reaches the host again.
                dispatch(container, MotionEvent.ACTION_DOWN, 0, first, other);
                assertEquals(flip, Collections.singleton(0), connection.heldTouches);
                dispatch(container, MotionEvent.ACTION_UP, 0, first, other);
                assertEquals(flip, Collections.emptySet(), connection.heldTouches);
            }
        }
    }

    @Test
    public void localCursorSendsWhereTheDrawnPointerIs() {
        RecordingConnection connection = new RecordingConnection();
        Game game = game(connection);
        PreferenceConfiguration config = touchConfig(game, TouchMode.LOCAL_CURSOR);
        config.localCursorSpeed = 50;
        View background = layout(game, 0, 0);
        ReflectionHelpers.callInstanceMethod(game.touchInput, "initializeTouchContexts");
        touchAt(game, background, MotionEvent.ACTION_DOWN, 500, 250, 1000);
        touchAt(game, background, MotionEvent.ACTION_MOVE, 700, 250, 1100);
        touchAt(game, background, MotionEvent.ACTION_MOVE, 700, -2000, 1200);
        touchAt(game, background, MotionEvent.ACTION_UP, 700, -2000, 1300);
        // The pointer starts in the middle, moves at half speed and stops at the edge.
        assertArrayEquals(new int[] {400, 200, 800, 400}, connection.positions.get(0));
        assertArrayEquals(new int[] {500, 200, 800, 400}, connection.positions.get(1));
        assertArrayEquals(new int[] {500, 0, 800, 400}, connection.positions.get(2));
        assertTrue(connection.moves.isEmpty());
    }

    @Test
    public void touchOffIgnoresFingers() {
        RecordingConnection connection = new RecordingConnection();
        Game game = game(connection);
        touchConfig(game, TouchMode.OFF);
        View background = layout(game, 0, 0);
        ReflectionHelpers.callInstanceMethod(game.touchInput, "initializeTouchContexts");
        touchAt(game, background, MotionEvent.ACTION_DOWN, 500, 250, 1000);
        touchAt(game, background, MotionEvent.ACTION_MOVE, 700, 250, 1100);
        touchAt(game, background, MotionEvent.ACTION_UP, 700, 250, 1150);
        Shadows.shadowOf(Looper.getMainLooper()).idleFor(1, TimeUnit.SECONDS);
        assertTrue(connection.positions.isEmpty());
        assertTrue(connection.moves.isEmpty());
        assertTrue(connection.buttons.isEmpty());
    }

    @Test
    public void trackpadGesturesStartOnlyInsideTheArea() {
        RecordingConnection connection = new RecordingConnection();
        Game game = game(connection);
        touchConfig(game, TouchMode.TRACKPAD).trackpadArea = "right_half";
        FrameLayout container = streamContainer(game, null);
        ReflectionHelpers.callInstanceMethod(game.touchInput, "initializeTouchContexts");
        int[] first = {0};
        dispatch(container, MotionEvent.ACTION_DOWN, 0, first, new float[] {100, 200});
        dispatch(container, MotionEvent.ACTION_UP, 0, first, new float[] {100, 200});
        Shadows.shadowOf(Looper.getMainLooper()).idleFor(200, TimeUnit.MILLISECONDS);
        assertTrue(connection.buttons.isEmpty());
        dispatch(container, MotionEvent.ACTION_DOWN, 0, first, new float[] {600, 200});
        dispatch(container, MotionEvent.ACTION_UP, 0, first, new float[] {600, 200});
        Shadows.shadowOf(Looper.getMainLooper()).idleFor(200, TimeUnit.MILLISECONDS);
        assertEquals(java.util.Arrays.asList("down 1", "up 1"), connection.buttons);
    }

    @Test
    public void momentumGlidesAfterAFlickAndStopsOnTheNextTouch() {
        RecordingConnection connection = new RecordingConnection();
        Game game = game(connection);
        touchConfig(game, TouchMode.TRACKPAD).trackpadMomentum = true;
        View background = layout(game, 0, 0);
        ReflectionHelpers.callInstanceMethod(game.touchInput, "initializeTouchContexts");
        flick(game, background);
        int afterFlick = connection.moves.size();
        TrackpadPointer pointer = ReflectionHelpers.getField(game, "trackpadPointer");
        assertTrue(pointer.isGliding());
        Shadows.shadowOf(Looper.getMainLooper()).idleFor(100, TimeUnit.MILLISECONDS);
        assertTrue(connection.moves.size() > afterFlick);

        long now = android.os.SystemClock.uptimeMillis();
        touchAt(game, background, MotionEvent.ACTION_DOWN, 500, 250, now);
        assertFalse(pointer.isGliding());
        int afterTouch = connection.moves.size();
        Shadows.shadowOf(Looper.getMainLooper()).idleFor(1, TimeUnit.SECONDS);
        assertEquals(afterTouch, connection.moves.size());
        touchAt(game, background, MotionEvent.ACTION_UP, 500, 250, now + 50);

        // Focus loss, mode changes, releasing input and PiP all cancel touch input first.
        flick(game, background);
        assertTrue(pointer.isGliding());
        ReflectionHelpers.callInstanceMethod(game.touchInput, "cancelTouchInput");
        assertFalse(pointer.isGliding());
        int afterCancel = connection.moves.size();
        Shadows.shadowOf(Looper.getMainLooper()).idleFor(1, TimeUnit.SECONDS);
        assertEquals(afterCancel, connection.moves.size());
    }

    @Test
    public void withoutMomentumAFlickStopsWithTheFinger() {
        RecordingConnection connection = new RecordingConnection();
        Game game = game(connection);
        touchConfig(game, TouchMode.TRACKPAD);
        View background = layout(game, 0, 0);
        ReflectionHelpers.callInstanceMethod(game.touchInput, "initializeTouchContexts");
        flick(game, background);
        int afterFlick = connection.moves.size();
        assertTrue(afterFlick > 0);
        Shadows.shadowOf(Looper.getMainLooper()).idleFor(1, TimeUnit.SECONDS);
        assertEquals(afterFlick, connection.moves.size());
    }

    @Test
    public void touchpadModeHoldsTheButtonWhenATouchFollowsATapQuickly() {
        RecordingConnection connection = new RecordingConnection();
        Game game = game(connection);
        touchConfig(game, TouchMode.TOUCHPAD);
        View background = layout(game, 0, 0);
        ReflectionHelpers.callInstanceMethod(game.touchInput, "initializeTouchContexts");
        long start = android.os.SystemClock.uptimeMillis();
        touchAt(game, background, MotionEvent.ACTION_DOWN, 100, 250, start);
        touchAt(game, background, MotionEvent.ACTION_UP, 100, 250, start + 40);
        assertEquals(Collections.singletonList("down 1"), connection.buttons);

        touchAt(game, background, MotionEvent.ACTION_DOWN, 100, 250, start + 150);
        for (int i = 1; i <= 3; i++) {
            touchAt(game, background, MotionEvent.ACTION_MOVE, 100 + i * 12, 250, start + 150 + i * 100);
        }
        assertFalse(connection.moves.isEmpty());
        assertEquals(Collections.singletonList("down 1"), connection.buttons);
        touchAt(game, background, MotionEvent.ACTION_UP, 136, 250, start + 500);
        assertEquals(java.util.Arrays.asList("down 1", "up 1"), connection.buttons);
    }

    /** M2: the pointer touchpad moves the host pointer in the stream's pixels and at Trackpad speed. */
    @Test public void thePointerTouchpadsMovementIsInHostPixels() {
        RecordingConnection connection = new RecordingConnection();
        Game game = game(connection);
        game.prefConfig.onscreenController = true;
        game.prefConfig.width = 1920;
        game.prefConfig.height = 1080;
        game.prefConfig.trackpadSpeed = 100;
        ReflectionHelpers.setField(game, "app", new com.limelight.nvstream.http.NvApp("Game", 7, false));
        StreamView stream = new StreamView(game);
        FrameLayout parent = new FrameLayout(game);
        parent.addView(stream);
        parent.measure(View.MeasureSpec.makeMeasureSpec(1600, View.MeasureSpec.EXACTLY),
                View.MeasureSpec.makeMeasureSpec(720, View.MeasureSpec.EXACTLY));
        parent.layout(0, 0, 1600, 720);
        ReflectionHelpers.setField(game, "streamView", stream);

        game.showOnscreenController();
        assertNotNull(game.modernController);
        com.limelight.binding.input.pad.ModernController.PointerSink sink =
                ReflectionHelpers.getField(game.modernController, "pointerSink");
        sink.move(100, 0);
        assertEquals("1920 host pixels across a 1600 px view", 120, connection.moves.get(0)[0]);
        sink.move(0, 100);
        assertEquals("1080 host pixels down a 720 px view", 150, connection.moves.get(1)[1]);
        game.modernController.destroy();
        assertEquals("Destroying the controls leaves the stream alone", 1, parent.getChildCount());
    }

    @Test
    public void portraitAndInputOnlyUseIsotropicScalingAndDisableAbsoluteMouseMode() {
        float[] portraitScale = com.limelight.binding.input.touch.RelativeTouchContext.motionScale(
                3120, 1440, 1440, 3120, 1f, false);
        assertEquals(1f, portraitScale[0], 0.0001f);
        assertEquals(portraitScale[0], portraitScale[1], 0.0001f);

        float[] inputOnlyScale = com.limelight.binding.input.touch.RelativeTouchContext.motionScale(
                1920, 1080, 2400, 1080, 1f, true);
        assertEquals(0.8f, inputOnlyScale[0], 0.0001f);
        assertEquals(inputOnlyScale[0], inputOnlyScale[1], 0.0001f);

        float[] landscapeVideoScale = com.limelight.binding.input.touch.RelativeTouchContext.motionScale(
                1920, 1080, 1600, 720, 1f, false);
        assertEquals(1.2f, landscapeVideoScale[0], 0.0001f);
        assertEquals(1.5f, landscapeVideoScale[1], 0.0001f);

        // Portrait INPUT_ONLY stream with absoluteMouseMode on must send relative moves with equal X/Y delta.
        RecordingConnection connection = new RecordingConnection();
        Game game = game(connection);
        PreferenceConfiguration config = touchConfig(game, TouchMode.TRACKPAD);
        config.width = 3120;
        config.height = 1440;
        config.absoluteMouseMode = true;
        com.limelight.nvstream.http.NvApp inputOnly = new com.limelight.nvstream.http.NvApp("Input", 9, false);
        inputOnly.setSmallStreamInputOnly(true);
        assertEquals(com.limelight.nvstream.http.NvApp.Role.INPUT_ONLY, inputOnly.getRole());
        ReflectionHelpers.setField(game, "app", inputOnly);
        View background = new View(game);
        background.layout(0, 0, 1440, 3120);
        StreamView stream = new StreamView(game);
        stream.layout(0, 0, 1440, 3120);
        ReflectionHelpers.setField(game, "streamView", stream);
        ReflectionHelpers.callInstanceMethod(game.touchInput, "initializeTouchContexts");

        touchAt(game, background, MotionEvent.ACTION_DOWN, 500, 500, 1000);
        touchAt(game, background, MotionEvent.ACTION_MOVE, 600, 600, 1020);
        touchAt(game, background, MotionEvent.ACTION_UP, 600, 600, 1040);
        assertTrue("INPUT_ONLY never sends absolute mouse positions", connection.positions.isEmpty());
        assertFalse(connection.moves.isEmpty());
        int totalX = 0, totalY = 0;
        for (int[] move : connection.moves) { totalX += move[0]; totalY += move[1]; }
        assertEquals("diagonal swipe produces equal X and Y motion", totalX, totalY);
    }

    @Test
    public void cancelTouchInputCancelsPhysicalTouchpadFallbackDrag() {
        RecordingConnection connection = new RecordingConnection();
        Game game = game(connection);
        StreamView stream = new StreamView(game);
        stream.layout(0, 0, 800, 400);
        ReflectionHelpers.setField(game, "streamView", stream);
        GamePhysicalTouchpad physical = ReflectionHelpers.getField(game.touchInput, "physicalTouchpad");
        physical.initializeContexts();
        com.limelight.binding.input.touch.TrackpadContext[] contexts =
                ReflectionHelpers.getField(physical, "touchpadContextMap");
        contexts[0].setPointerCount(1);
        contexts[0].touchDownEvent(200, 200, 1000, true);
        contexts[0].touchUpEvent(200, 200, 1040);
        contexts[0].touchDownEvent(200, 200, 1080, true);
        contexts[0].touchMoveEvent(260, 200, 1100);
        assertEquals(Collections.singletonList("down 1"), connection.buttons);

        game.touchInput.cancelTouchInput();
        assertEquals(java.util.Arrays.asList("down 1", "up 1"), connection.buttons);
    }

    @Test
    public void draggingAtOneXTransformsDirectAndNativeInputIntoMovedEdgesAndBlackBorders() {
        for (boolean nativeTouch : new boolean[] {false, true}) {
            RecordingConnection connection = new RecordingConnection();
            Game game = game(connection);
            View background = layout(game, 0, 0);
            StreamView stream = ReflectionHelpers.getField(game, "streamView");
            com.limelight.ui.DesktopGestures desktop = new com.limelight.ui.DesktopGestures(game, stream, () -> { }, () -> { });
            desktop.setEnabled(true);
            desktop.viewport().resize(800, 400);
            // At 1.0x, pull the upper-right corner (800, 0) inward to the center (400, 200) of streamView.
            desktop.viewport().pan(-400, 200);
            ReflectionHelpers.setField(game, "desktopGestures", desktop);
            ReflectionHelpers.setField(game, "nativeTouchEnabled", nativeTouch);
            ReflectionHelpers.callInstanceMethod(game.touchInput, "initializeTouchContexts");

            touch(game, background, MotionEvent.ACTION_DOWN, 500, 250);
            touch(game, background, MotionEvent.ACTION_UP, 500, 250);
            if (nativeTouch) assertArrayEquals(new float[] {1.0f, 0.0f}, connection.touches.get(0), .00001f);
            else assertArrayEquals(new int[] {800, 0, 800, 400}, connection.positions.get(0));
            Shadows.shadowOf(Looper.getMainLooper()).idleFor(100, TimeUnit.MILLISECONDS);
        }

        // Fit 600x400 centered at offsetX=100 inside 800x400 container, zoomed 1.25x so video covers left border.
        RecordingConnection connection = new RecordingConnection();
        Game game = game(connection);
        View background = new View(game);
        background.layout(0, 0, 800, 400);
        StreamView stream = new StreamView(game);
        stream.layout(100, 0, 700, 400);
        ReflectionHelpers.setField(game, "streamView", stream);
        com.limelight.ui.DesktopGestures desktop = new com.limelight.ui.DesktopGestures(game, stream, () -> { }, () -> { });
        desktop.setEnabled(true);
        desktop.viewport().setBounds(600, 400, 800, 400, 100, 0);
        desktop.viewport().zoom(1.25f, 300, 200); // x = -75 -> left video edge sits at x=25 in left black border
        ReflectionHelpers.setField(game, "desktopGestures", desktop);
        ReflectionHelpers.callInstanceMethod(game.touchInput, "initializeTouchContexts");
        touch(game, background, MotionEvent.ACTION_DOWN, 25, 200);
        touch(game, background, MotionEvent.ACTION_UP, 25, 200);
        assertArrayEquals(new int[] {0, 200, 600, 400}, connection.positions.get(0));
        Shadows.shadowOf(Looper.getMainLooper()).idleFor(100, TimeUnit.MILLISECONDS);
    }

    static com.limelight.ui.DesktopGestures enableZoom(Game game) {
        StreamView stream = ReflectionHelpers.getField(game, "streamView");
        com.limelight.ui.DesktopGestures desktop = new com.limelight.ui.DesktopGestures(game, stream,
                () -> ReflectionHelpers.callInstanceMethod(game.touchInput, "cancelHostTouchInput"),
                () -> ReflectionHelpers.callInstanceMethod(game.touchInput, "clickRightMouseButton"));
        ReflectionHelpers.setField(game, "desktopGestures", desktop);
        desktop.setEnabled(true);
        desktop.viewport().resize(800, 400);
        desktop.viewport().zoom(2, 400, 200);
        return desktop;
    }

    static FrameLayout gestureContainer(Game game) {
        game.setTheme(R.style.AppTheme);
        FrameLayout container = streamContainer(game, null);
        game.setContentView(container);
        StreamView stream = ReflectionHelpers.getField(game, "streamView");
        stream.setFocusableInTouchMode(true);
        ReflectionHelpers.callInstanceMethod(game.touchInput, "initializeTouchContexts");
        return container;
    }

    static float[] point(int id) { return new float[] {200 + id * 60, 150}; }

    static void beginTap(View view, int count) {
        for (int fingers = 1; fingers <= count; fingers++) {
            int[] ids = new int[fingers];
            float[][] points = new float[fingers][];
            for (int i = 0; i < fingers; i++) { ids[i] = i; points[i] = point(i); }
            dispatch(view, fingers == 1 ? MotionEvent.ACTION_DOWN : MotionEvent.ACTION_POINTER_DOWN,
                    fingers - 1, ids, points);
        }
    }

    static void finishTap(View view, int count) { finishTap(view, count, 1030, 0); }

    static void finishTap(View view, int count, long time, int first) {
        // Lift the original first finger first, exercising Android's pointer-index reassignment.
        for (int lifted = first; lifted < count; lifted++) {
            int remaining = count - lifted;
            int[] ids = new int[remaining];
            float[][] points = new float[remaining][];
            for (int i = 0; i < remaining; i++) { ids[i] = i + lifted; points[i] = point(i + lifted); }
            dispatchAt(view, remaining == 1 ? MotionEvent.ACTION_UP : MotionEvent.ACTION_POINTER_UP,
                    0, ids, time, 0, points);
        }
    }

    private static void flick(Game game, View view) {
        Shadows.shadowOf(Looper.getMainLooper()).idleFor(1, TimeUnit.SECONDS);
        long start = android.os.SystemClock.uptimeMillis();
        touchAt(game, view, MotionEvent.ACTION_DOWN, 100, 250, start);
        for (int i = 1; i <= 8; i++) {
            touchAt(game, view, MotionEvent.ACTION_MOVE, 100 + i * 40, 250, start + i * 8);
        }
        touchAt(game, view, MotionEvent.ACTION_UP, 420, 250, start + 66);
    }

    static PreferenceConfiguration touchConfig(Game game, int mode) {
        PreferenceConfiguration config = ReflectionHelpers.getField(game, "prefConfig");
        config.touchMode = mode;
        config.touchscreenTrackpad = TouchMode.legacyTrackpad(mode);
        config.width = 800;
        config.height = 400;
        config.trackpadSpeed = 100;
        config.localCursorSpeed = 100;
        return config;
    }

    private static void touchAt(Game game, View view, int action, float x, float y, long time) {
        MotionEvent event = MotionEvent.obtain(1000, time, action, x, y, 0);
        event.setSource(InputDevice.SOURCE_TOUCHSCREEN);
        try {
            assertTrue(game.onTouch(view, event));
        } finally {
            event.recycle();
        }
    }

    private static GameTouchControls touchControls(Game game) {
        game.setTheme(R.style.AppTheme);
        new TouchLayout(Collections.singletonList(new TouchLayout.Button("w", "W", new int[] {51}, 0,
                false, .1f, .1f, .2f, .2f))).save(game, "fps");
        TouchLayout.setEnabled(game, "fps", true);
        GameTouchControls controls = new GameTouchControls(game, "fps", new HeldInputState.Sink() {
            @Override public void key(int key, boolean down) { }
            @Override public void mouse(int button, boolean down) { }
        }, () -> true, dialog -> { });
        // Wired the way Game.onCreate wires its controls.
        controls.setStreamTouchListener((view, event) -> ReflectionHelpers.callInstanceMethod(game.touchInput,
                "onTouchControlsPassthrough", ReflectionHelpers.ClassParameter.from(View.class, view),
                ReflectionHelpers.ClassParameter.from(MotionEvent.class, event)));
        controls.setGestureCancelListener(() -> ReflectionHelpers.callInstanceMethod(game.touchInput, "cancelTouchInput"));
        ReflectionHelpers.setField(game, "gameTouchControls", controls);
        return controls;
    }

    // The stream's container: the stream, the touch view below the controls, then the controls.
    static FrameLayout streamContainer(Game game, GameTouchControls controls) {
        StreamView stream = new StreamView(game);
        View background = new View(game);
        background.setOnTouchListener(game);
        FrameLayout container = new FrameLayout(game);
        container.addView(stream);
        container.addView(background);
        if (controls != null) container.addView(controls);
        container.measure(View.MeasureSpec.makeMeasureSpec(800, View.MeasureSpec.EXACTLY),
                View.MeasureSpec.makeMeasureSpec(400, View.MeasureSpec.EXACTLY));
        container.layout(0, 0, 800, 400);
        ReflectionHelpers.setField(game, "streamView", stream);
        return container;
    }

    static void dispatch(View view, int action, int actionIndex, int[] ids, float[]... points) {
        dispatchAt(view, action, actionIndex, ids, 1010, 0, points);
    }

    static void dispatchAt(View view, int action, int actionIndex, int[] ids,
                           long time, int flags, float[]... points) {
        MotionEvent event = motion(action, actionIndex, ids, time, flags, points);
        try { view.dispatchTouchEvent(event); } finally { event.recycle(); }
    }

    static MotionEvent motion(int action, int actionIndex, int[] ids,
                              long time, int flags, float[]... points) {
        MotionEvent.PointerProperties[] properties = new MotionEvent.PointerProperties[points.length];
        MotionEvent.PointerCoords[] coords = new MotionEvent.PointerCoords[points.length];
        for (int i = 0; i < points.length; i++) {
            properties[i] = new MotionEvent.PointerProperties();
            properties[i].id = ids[i];
            properties[i].toolType = MotionEvent.TOOL_TYPE_FINGER;
            coords[i] = new MotionEvent.PointerCoords();
            coords[i].x = points[i][0];
            coords[i].y = points[i][1];
        }
        int masked = action == MotionEvent.ACTION_POINTER_DOWN || action == MotionEvent.ACTION_POINTER_UP ?
                action | (actionIndex << MotionEvent.ACTION_POINTER_INDEX_SHIFT) : action;
        return MotionEvent.obtain(1000, time, masked, points.length, properties, coords, 0, 0, 1, 1,
                0, 0, InputDevice.SOURCE_TOUCHSCREEN, flags);
    }

    static Game game(RecordingConnection connection) {
        Game game = Robolectric.buildActivity(Game.class).get();
        ReflectionHelpers.setField(game, "conn", connection);
        ReflectionHelpers.setField(game, "prefConfig", new PreferenceConfiguration());
        ReflectionHelpers.setField(game, "grabbedInput", true);
        ReflectionHelpers.setField(game, "foreground", true);
        ReflectionHelpers.setField(game, "connected", true);
        return game;
    }

    private static View layout(Game game, int left, int top) {
        View background = new View(game);
        background.layout(left, top, left + 1000, top + 500);
        StreamView stream = new StreamView(game);
        stream.layout(left + 100, top + 50, left + 900, top + 450);
        ReflectionHelpers.setField(game, "streamView", stream);
        return background;
    }

    private static void touch(Game game, View view, int action, float x, float y) {
        MotionEvent event = MotionEvent.obtain(1000, 1010, action, x, y, 0);
        event.setSource(InputDevice.SOURCE_TOUCHSCREEN);
        try {
            assertTrue(game.onTouch(view, event));
        } finally {
            event.recycle();
        }
    }

    static class RecordingConnection extends NvConnection {
        final List<int[]> positions = new ArrayList<>();
        final List<float[]> touches = new ArrayList<>();
        final List<int[]> moves = new ArrayList<>();
        final List<String> buttons = new ArrayList<>();
        final List<Short> scrolls = new ArrayList<>();
        // Pointers the host considers pressed.
        final Set<Integer> heldTouches = new HashSet<>();

        RecordingConnection() {
            super(null, new ComputerDetails.AddressTuple("127.0.0.1", 47989), 47984, "test",
                    new StreamConfiguration.Builder().build(), null, null);
        }

        @Override public boolean canSendInput() { return true; }
        @Override public void sendMousePosition(short x, short y, short width, short height) {
            positions.add(new int[] {x, y, width, height});
        }
        @Override public void sendMouseMove(short x, short y) { moves.add(new int[] {x, y}); }
        @Override public void sendMouseButtonDown(byte button) { buttons.add("down " + button); }
        @Override public void sendMouseButtonUp(byte button) { buttons.add("up " + button); }
        @Override public void sendMouseHighResScroll(short amount) { scrolls.add(amount); }
        @Override public int sendTouchEvent(byte event, int pointer, float x, float y, float pressure,
                                            float major, float minor, short rotation) {
            touches.add(new float[] {x, y});
            if (event == MoonBridge.LI_TOUCH_EVENT_DOWN) heldTouches.add(pointer);
            else if (event == MoonBridge.LI_TOUCH_EVENT_UP || event == MoonBridge.LI_TOUCH_EVENT_CANCEL) {
                heldTouches.remove(pointer);
            } else if (event == MoonBridge.LI_TOUCH_EVENT_CANCEL_ALL) heldTouches.clear();
            return 0;
        }
    }
}
