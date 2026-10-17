package com.limelight;

import android.app.Application;
import android.os.Looper;
import android.view.InputDevice;
import android.view.MotionEvent;
import android.view.View;
import android.widget.FrameLayout;

import com.limelight.binding.input.HeldInputState;
import com.limelight.binding.input.TouchLayout;
import com.limelight.binding.input.touch.FingerTapGesture;
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
            ReflectionHelpers.callInstanceMethod(game, "initializeTouchContexts");
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
        ReflectionHelpers.callInstanceMethod(game, "initializeTouchContexts");
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
            com.limelight.ui.DesktopGestures desktop = new com.limelight.ui.DesktopGestures(game, stream, () -> { });
            desktop.setEnabled(true); desktop.viewport().resize(800, 400);
            desktop.viewport().zoom(2, 400, 200); desktop.viewport().pan(100, 0);
            ReflectionHelpers.setField(game, "desktopGestures", desktop);
            ReflectionHelpers.setField(game, "nativeTouchEnabled", nativeTouch);
            ReflectionHelpers.callInstanceMethod(game, "initializeTouchContexts");
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
        ReflectionHelpers.callInstanceMethod(game, "initializeTouchContexts");
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
        ReflectionHelpers.callInstanceMethod(game, "initializeTouchContexts");
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
        ReflectionHelpers.callInstanceMethod(game, "initializeTouchContexts");
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
    public void threeFingerTapCanMiddleClick() {
        RecordingConnection connection = new RecordingConnection();
        Game game = game(connection);
        touchConfig(game, TouchMode.TRACKPAD).threeFingerTap = "middle_click";
        FrameLayout container = streamContainer(game, null);
        ReflectionHelpers.callInstanceMethod(game, "initializeTouchContexts");
        float[] a = {300, 200}, b = {400, 200}, c = {500, 200};
        dispatch(container, MotionEvent.ACTION_DOWN, 0, new int[] {0}, a);
        dispatch(container, MotionEvent.ACTION_POINTER_DOWN, 1, new int[] {0, 1}, a, b);
        dispatch(container, MotionEvent.ACTION_POINTER_DOWN, 2, new int[] {0, 1, 2}, a, b, c);
        dispatch(container, MotionEvent.ACTION_POINTER_UP, 0, new int[] {0, 1, 2}, a, b, c);
        dispatch(container, MotionEvent.ACTION_POINTER_UP, 0, new int[] {1, 2}, b, c);
        dispatch(container, MotionEvent.ACTION_UP, 0, new int[] {2}, c);
        Shadows.shadowOf(Looper.getMainLooper()).idleFor(200, TimeUnit.MILLISECONDS);
        assertEquals(java.util.Arrays.asList("down 2", "up 2"), connection.buttons);
    }

    @Test
    public void threeFingerTapSurvivesTheFirstFingerLiftingBeforeTheOthers() {
        RecordingConnection connection = new RecordingConnection();
        Game game = game(connection);
        touchConfig(game, TouchMode.TRACKPAD).threeFingerTap = "middle_click";
        FrameLayout container = streamContainer(game, null);
        ReflectionHelpers.callInstanceMethod(game, "initializeTouchContexts");
        float[] a = {300, 200}, b = {400, 200}, c = {500, 200};
        dispatch(container, MotionEvent.ACTION_DOWN, 0, new int[] {0}, a);
        dispatch(container, MotionEvent.ACTION_POINTER_DOWN, 1, new int[] {0, 1}, a, b);
        dispatch(container, MotionEvent.ACTION_POINTER_DOWN, 2, new int[] {0, 1, 2}, a, b, c);
        dispatch(container, MotionEvent.ACTION_POINTER_UP, 0, new int[] {0, 1, 2}, a, b, c);
        // Pointer index 0 is now the second finger, 100 px from where the first one was.
        dispatch(container, MotionEvent.ACTION_MOVE, 0, new int[] {1, 2}, b, c);
        dispatch(container, MotionEvent.ACTION_POINTER_UP, 0, new int[] {1, 2}, b, c);
        dispatch(container, MotionEvent.ACTION_UP, 0, new int[] {2}, c);
        Shadows.shadowOf(Looper.getMainLooper()).idleFor(200, TimeUnit.MILLISECONDS);
        assertEquals(java.util.Arrays.asList("down 2", "up 2"), connection.buttons);
    }

    @Test public void eitherGestureCanOpenTheDesktopPanelInEveryMouseTouchMode() {
        for (int mode : new int[] {TouchMode.TRACKPAD, TouchMode.DIRECT, TouchMode.LOCAL_CURSOR}) {
            for (int fingers : new int[] {3, 4}) {
                RecordingConnection connection = new RecordingConnection();
                Game game = game(connection);
                PreferenceConfiguration config = touchConfig(game, mode);
                config.threeFingerTap = fingers == 3 ? "desktop_controls" : "middle_click";
                config.fourFingerTap = fingers == 4 ? "desktop_controls" : "none";
                FrameLayout container = gestureContainer(game);
                beginTap(container, fingers);
                assertNull(ReflectionHelpers.getField(game, "keyboardOverlay"));
                finishTap(container, fingers);
                View panel = ReflectionHelpers.getField(game, "keyboardOverlay");
                assertNotNull(panel);
                assertEquals(View.VISIBLE, panel.getVisibility());
                assertFalse(connection.buttons.contains("down 2"));
                // A second gesture in the uncovered stream hides the same panel.
                beginTap(container, fingers);
                finishTap(container, fingers);
                assertEquals(View.GONE, panel.getVisibility());
                Shadows.shadowOf(Looper.getMainLooper()).idleFor(200, TimeUnit.MILLISECONDS);
            }
        }
    }

    @Test public void fourFingerMiddleClickDoesNotAlsoOpenTheThreeFingerPhoneKeyboard() {
        RecordingConnection connection = new RecordingConnection();
        Game game = game(connection);
        PreferenceConfiguration config = touchConfig(game, TouchMode.TRACKPAD);
        config.threeFingerTap = "keyboard";
        config.fourFingerTap = "middle_click";
        FrameLayout container = gestureContainer(game);
        beginTap(container, 4);
        finishTap(container, 4);
        android.view.inputmethod.InputMethodManager input = (android.view.inputmethod.InputMethodManager)
                game.getSystemService(android.content.Context.INPUT_METHOD_SERVICE);
        assertFalse(Shadows.shadowOf(input).isSoftInputVisible());
        Shadows.shadowOf(Looper.getMainLooper()).idleFor(200, TimeUnit.MILLISECONDS);
        assertEquals(java.util.Arrays.asList("down 2", "up 2"), connection.buttons);
    }

    @Test public void fourFingersAreOffByDefaultAndNeverTriggerTheThreeFingerAction() {
        RecordingConnection connection = new RecordingConnection();
        Game game = game(connection);
        PreferenceConfiguration config = touchConfig(game, TouchMode.TRACKPAD);
        config.threeFingerTap = "middle_click";
        assertEquals("none", config.fourFingerTap);
        FrameLayout container = gestureContainer(game);
        beginTap(container, 4);
        finishTap(container, 4);
        Shadows.shadowOf(Looper.getMainLooper()).idleFor(200, TimeUnit.MILLISECONDS);
        assertTrue(connection.buttons.isEmpty());
        assertNull(ReflectionHelpers.getField(game, "keyboardOverlay"));
        config.threeFingerTap = "none";
        beginTap(container, 3);
        FingerTapGesture tap = ReflectionHelpers.getField(game, "fingerTap");
        assertFalse(tap.isStarted());
        dispatch(container, MotionEvent.ACTION_CANCEL, 0, new int[] {0}, point(0));
    }

    @Test public void eitherGestureCanOpenThePhoneKeyboardAndHideTheDesktopPanel() {
        for (int fingers : new int[] {3, 4}) {
            Game game = game(new RecordingConnection());
            PreferenceConfiguration config = touchConfig(game, TouchMode.TRACKPAD);
            config.threeFingerTap = fingers == 3 ? "keyboard" : "none";
            config.fourFingerTap = fingers == 4 ? "keyboard" : "none";
            FrameLayout container = gestureContainer(game);
            ReflectionHelpers.callInstanceMethod(game, "toggleKeyboardOverlay");
            View panel = ReflectionHelpers.getField(game, "keyboardOverlay");
            beginTap(container, fingers);
            finishTap(container, fingers);
            android.view.inputmethod.InputMethodManager input = (android.view.inputmethod.InputMethodManager)
                    game.getSystemService(android.content.Context.INPUT_METHOD_SERVICE);
            assertTrue(Shadows.shadowOf(input).isSoftInputVisible());
            assertEquals(View.GONE, panel.getVisibility());
        }
    }

    @Test public void invalidMultiFingerGesturesStayConsumedUntilTheLastFingerLifts() {
        for (String invalid : new String[] {"five", "slow", "history", "final_position", "cancel", "lifecycle"}) {
            RecordingConnection connection = new RecordingConnection();
            Game game = game(connection);
            PreferenceConfiguration config = touchConfig(game, TouchMode.TRACKPAD);
            config.threeFingerTap = config.fourFingerTap = "desktop_controls";
            FrameLayout container = gestureContainer(game);
            int fingers = "five".equals(invalid) ? 5 : 4;
            beginTap(container, fingers);
            if ("slow".equals(invalid)) {
                dispatchAt(container, MotionEvent.ACTION_MOVE, 0, new int[] {0, 1, 2, 3}, 1400, 0,
                        point(0), point(1), point(2), point(3));
            } else if ("history".equals(invalid)) {
                MotionEvent move = motion(MotionEvent.ACTION_MOVE, 0, new int[] {0, 1, 2, 3}, 1010, 0,
                        new float[] {500, 150}, point(1), point(2), point(3));
                MotionEvent.PointerCoords[] returned = new MotionEvent.PointerCoords[4];
                for (int i = 0; i < 4; i++) {
                    returned[i] = new MotionEvent.PointerCoords();
                    returned[i].x = point(i)[0]; returned[i].y = point(i)[1];
                }
                move.addBatch(1020, returned, 0);
                try { container.dispatchTouchEvent(move); } finally { move.recycle(); }
            } else if ("final_position".equals(invalid)) {
                dispatch(container, MotionEvent.ACTION_POINTER_UP, 0, new int[] {0, 1, 2, 3},
                        new float[] {500, 150}, point(1), point(2), point(3));
            } else if ("cancel".equals(invalid)) {
                dispatch(container, MotionEvent.ACTION_CANCEL, 0, new int[] {0, 1, 2, 3},
                        point(0), point(1), point(2), point(3));
            } else if ("lifecycle".equals(invalid)) {
                ReflectionHelpers.callInstanceMethod(game, "cancelTouchInput");
            }
            if (!"cancel".equals(invalid) && !"lifecycle".equals(invalid)) {
                finishTap(container, fingers, "slow".equals(invalid) ? 1400 : 1030,
                        "final_position".equals(invalid) ? 1 : 0);
            }
            assertNull(invalid, ReflectionHelpers.getField(game, "keyboardOverlay"));
            assertFalse(invalid, connection.buttons.contains("down 2"));
            // A clean gesture immediately afterwards must still work.
            beginTap(container, 4);
            finishTap(container, 4);
            View panel = ReflectionHelpers.getField(game, "keyboardOverlay");
            assertNotNull(invalid, panel);
            assertEquals(invalid, View.VISIBLE, panel.getVisibility());
            Shadows.shadowOf(Looper.getMainLooper()).idleFor(200, TimeUnit.MILLISECONDS);
        }
    }

    @Test @Config(sdk = 33)
    public void canceledPointerCannotPromoteTheRemainderIntoAnotherShortcut() {
        Game game = game(new RecordingConnection());
        PreferenceConfiguration config = touchConfig(game, TouchMode.TRACKPAD);
        config.threeFingerTap = config.fourFingerTap = "desktop_controls";
        FrameLayout container = gestureContainer(game);
        beginTap(container, 4);
        dispatchAt(container, MotionEvent.ACTION_POINTER_UP, 0, new int[] {0, 1, 2, 3},
                1020, MotionEvent.FLAG_CANCELED, point(0), point(1), point(2), point(3));
        finishTap(container, 4, 1030, 1);
        assertNull(ReflectionHelpers.getField(game, "keyboardOverlay"));
    }

    @Test public void shortcutsRespectNativeTouchOffAndTheConfiguredTrackpadArea() {
        for (int mode : new int[] {TouchMode.MULTI_TOUCH, TouchMode.OFF, TouchMode.TRACKPAD}) {
            RecordingConnection connection = new RecordingConnection();
            Game game = game(connection);
            PreferenceConfiguration config = touchConfig(game, mode);
            config.threeFingerTap = config.fourFingerTap = "desktop_controls";
            if (mode == TouchMode.TRACKPAD) config.trackpadArea = "right_half";
            FrameLayout container = gestureContainer(game);
            if (mode == TouchMode.MULTI_TOUCH) ReflectionHelpers.setField(game, "nativeTouchEnabled", true);
            beginTap(container, 4);
            finishTap(container, 4);
            assertNull(ReflectionHelpers.getField(game, "keyboardOverlay"));
            assertTrue(connection.heldTouches.isEmpty());
            if (mode == TouchMode.MULTI_TOUCH) assertFalse(connection.touches.isEmpty());
        }
    }

    private static FrameLayout gestureContainer(Game game) {
        game.setTheme(R.style.AppTheme);
        FrameLayout container = streamContainer(game, null);
        game.setContentView(container);
        StreamView stream = ReflectionHelpers.getField(game, "streamView");
        stream.setFocusableInTouchMode(true);
        ReflectionHelpers.callInstanceMethod(game, "initializeTouchContexts");
        return container;
    }

    private static float[] point(int id) { return new float[] {200 + id * 60, 150}; }

    private static void beginTap(View view, int count) {
        for (int fingers = 1; fingers <= count; fingers++) {
            int[] ids = new int[fingers];
            float[][] points = new float[fingers][];
            for (int i = 0; i < fingers; i++) { ids[i] = i; points[i] = point(i); }
            dispatch(view, fingers == 1 ? MotionEvent.ACTION_DOWN : MotionEvent.ACTION_POINTER_DOWN,
                    fingers - 1, ids, points);
        }
    }

    private static void finishTap(View view, int count) { finishTap(view, count, 1030, 0); }

    private static void finishTap(View view, int count, long time, int first) {
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

    @Test
    public void momentumGlidesAfterAFlickAndStopsOnTheNextTouch() {
        RecordingConnection connection = new RecordingConnection();
        Game game = game(connection);
        touchConfig(game, TouchMode.TRACKPAD).trackpadMomentum = true;
        View background = layout(game, 0, 0);
        ReflectionHelpers.callInstanceMethod(game, "initializeTouchContexts");
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
        ReflectionHelpers.callInstanceMethod(game, "cancelTouchInput");
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
        ReflectionHelpers.callInstanceMethod(game, "initializeTouchContexts");
        flick(game, background);
        int afterFlick = connection.moves.size();
        assertTrue(afterFlick > 0);
        Shadows.shadowOf(Looper.getMainLooper()).idleFor(1, TimeUnit.SECONDS);
        assertEquals(afterFlick, connection.moves.size());
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

    private static PreferenceConfiguration touchConfig(Game game, int mode) {
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
        controls.setStreamTouchListener((view, event) -> ReflectionHelpers.callInstanceMethod(game,
                "onTouchControlsPassthrough", ReflectionHelpers.ClassParameter.from(View.class, view),
                ReflectionHelpers.ClassParameter.from(MotionEvent.class, event)));
        controls.setGestureCancelListener(() -> ReflectionHelpers.callInstanceMethod(game, "cancelTouchInput"));
        ReflectionHelpers.setField(game, "gameTouchControls", controls);
        return controls;
    }

    // The stream's container: the stream, the touch view below the controls, then the controls.
    private static FrameLayout streamContainer(Game game, GameTouchControls controls) {
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

    private static void dispatch(View view, int action, int actionIndex, int[] ids, float[]... points) {
        dispatchAt(view, action, actionIndex, ids, 1010, 0, points);
    }

    private static void dispatchAt(View view, int action, int actionIndex, int[] ids,
                                   long time, int flags, float[]... points) {
        MotionEvent event = motion(action, actionIndex, ids, time, flags, points);
        try { view.dispatchTouchEvent(event); } finally { event.recycle(); }
    }

    private static MotionEvent motion(int action, int actionIndex, int[] ids,
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

    private static Game game(RecordingConnection connection) {
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

    private static class RecordingConnection extends NvConnection {
        final List<int[]> positions = new ArrayList<>();
        final List<float[]> touches = new ArrayList<>();
        final List<int[]> moves = new ArrayList<>();
        final List<String> buttons = new ArrayList<>();
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
