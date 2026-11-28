package com.limelight;

import android.app.Application;
import android.os.Looper;
import android.view.MotionEvent;
import android.view.View;
import android.widget.FrameLayout;

import com.limelight.GameTouchInputTest.RecordingConnection;
import com.limelight.binding.input.touch.FingerTapGesture;
import com.limelight.binding.input.touch.TouchMode;
import com.limelight.preferences.PreferenceConfiguration;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.Shadows;
import org.robolectric.annotation.Config;
import org.robolectric.util.ReflectionHelpers;

import java.util.Collections;
import java.util.concurrent.TimeUnit;

import static com.limelight.GameTouchInputTest.*;
import static org.junit.Assert.*;

/**
 * Tests two-finger tap/scroll/zoom interactions and three-/four-finger tap shortcuts in
 * {@link Game}.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class,
        shadows = PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class GameTouchGesturesTest {
    @Test
    public void twoFingerTapClicksOnceAfterBothFingersLiftInEitherOrderWithOrWithoutZoom() {
        for (int mode : new int[] {TouchMode.TRACKPAD, TouchMode.LOCAL_CURSOR}) {
            for (boolean zoom : new boolean[] {false, true}) {
                for (int firstLift : new int[] {0, 1}) {
                    RecordingConnection connection = new RecordingConnection();
                    Game game = game(connection);
                    touchConfig(game, mode);
                    FrameLayout container = gestureContainer(game);
                    com.limelight.ui.DesktopGestures desktop = zoom ? enableZoom(game) : null;
                    beginTap(container, 2);
                    dispatchAt(container, MotionEvent.ACTION_POINTER_UP, firstLift, new int[] {0, 1},
                            1050, 0, point(0), point(1));
                    assertTrue("Wait for the entire gesture", connection.buttons.isEmpty());
                    int remaining = 1 - firstLift;
                    dispatchAt(container, MotionEvent.ACTION_UP, 0, new int[] {remaining},
                            1080, 0, point(remaining));
                    assertEquals(Collections.singletonList("down 3"), connection.buttons);
                    Shadows.shadowOf(Looper.getMainLooper()).idleFor(100, TimeUnit.MILLISECONDS);
                    assertEquals(java.util.Arrays.asList("down 3", "up 3"), connection.buttons);
                    assertTrue(connection.scrolls.isEmpty());
                    if (desktop != null) assertEquals(2f, desktop.viewport().scale(), 0);
                }
            }
        }
    }

    @Test
    public void twoFingerTapsRejectMovementFromEitherFingerHistoryExtraFingersAndCancellation() {
        for (boolean zoom : new boolean[] {false, true}) {
            for (String reason : new String[] {"first_moves", "second_moves", "before_second", "history",
                    "lift_moves", "last_lift_moves", "slow", "third", "replacement", "cancel", "lifecycle"}) {
                RecordingConnection connection = new RecordingConnection();
                Game game = game(connection);
                PreferenceConfiguration config = touchConfig(game, TouchMode.TRACKPAD);
                config.threeFingerTap = config.fourFingerTap = "none";
                FrameLayout container = gestureContainer(game);
                if (zoom) enableZoom(game);
                if (reason.equals("before_second")) {
                    beginTap(container, 1);
                    dispatch(container, MotionEvent.ACTION_MOVE, 0, new int[] {0}, new float[] {400, 150});
                    dispatch(container, MotionEvent.ACTION_POINTER_DOWN, 1, new int[] {0, 1}, point(0), point(1));
                } else beginTap(container, 2);

                if (reason.endsWith("_moves") && !reason.contains("lift")) {
                    dispatch(container, MotionEvent.ACTION_MOVE, 0, new int[] {0, 1},
                            reason.startsWith("first") ? new float[] {400, 150} : point(0),
                            reason.startsWith("second") ? new float[] {460, 150} : point(1));
                } else if (reason.equals("history")) {
                    MotionEvent event = motion(MotionEvent.ACTION_MOVE, 0, new int[] {0, 1}, 1020, 0,
                            point(0), new float[] {460, 150});
                    MotionEvent.PointerCoords[] returned = new MotionEvent.PointerCoords[2];
                    for (int i = 0; i < 2; i++) {
                        returned[i] = new MotionEvent.PointerCoords();
                        returned[i].x = point(i)[0]; returned[i].y = point(i)[1];
                    }
                    event.addBatch(1030, returned, 0);
                    try { container.dispatchTouchEvent(event); } finally { event.recycle(); }
                } else if (reason.equals("third")) {
                    dispatch(container, MotionEvent.ACTION_POINTER_DOWN, 2, new int[] {0, 1, 2},
                            point(0), point(1), point(2));
                    dispatch(container, MotionEvent.ACTION_POINTER_UP, 2, new int[] {0, 1, 2},
                            point(0), point(1), point(2));
                } else if (reason.equals("cancel")) {
                    dispatch(container, MotionEvent.ACTION_CANCEL, 0, new int[] {0, 1}, point(0), point(1));
                } else if (reason.equals("lifecycle")) {
                    ReflectionHelpers.callInstanceMethod(game.touchInput, "cancelTouchInput");
                }
                dispatchAt(container, MotionEvent.ACTION_POINTER_UP, 0, new int[] {0, 1}, 1050, 0,
                        reason.equals("lift_moves") ? new float[] {400, 150} : point(0), point(1));
                if (reason.equals("replacement")) {
                    dispatch(container, MotionEvent.ACTION_POINTER_DOWN, 1, new int[] {1, 7}, point(1), point(2));
                    dispatch(container, MotionEvent.ACTION_POINTER_UP, 1, new int[] {1, 7}, point(1), point(2));
                }
                dispatchAt(container, MotionEvent.ACTION_UP, 0, new int[] {1}, reason.equals("slow") ? 1400 : 1080,
                        0, reason.equals("last_lift_moves") ? new float[] {460, 150} : point(1));
                Shadows.shadowOf(Looper.getMainLooper()).idleFor(1, TimeUnit.SECONDS);
                assertTrue(reason + ", zoom=" + zoom + ": " + connection.buttons, connection.buttons.isEmpty());
                // Rejection cannot poison the next clean gesture.
                beginTap(container, 2);
                finishTap(container, 2);
                Shadows.shadowOf(Looper.getMainLooper()).idleFor(100, TimeUnit.MILLISECONDS);
                assertEquals(reason, java.util.Arrays.asList("down 3", "up 3"), connection.buttons);
            }
        }
    }

    @Test @Config(sdk = 33)
    public void canceledPointerLiftCannotBecomeARightClick() {
        for (boolean zoom : new boolean[] {false, true}) {
            RecordingConnection connection = new RecordingConnection();
            Game game = game(connection);
            touchConfig(game, TouchMode.TRACKPAD);
            FrameLayout container = gestureContainer(game);
            if (zoom) enableZoom(game);
            beginTap(container, 2);
            dispatchAt(container, MotionEvent.ACTION_POINTER_UP, 1, new int[] {0, 1}, 1030,
                    MotionEvent.FLAG_CANCELED, point(0), point(1));
            dispatchAt(container, MotionEvent.ACTION_UP, 0, new int[] {0}, 1050, 0, point(0));
            Shadows.shadowOf(Looper.getMainLooper()).idleFor(1, TimeUnit.SECONDS);
            assertTrue(connection.buttons.isEmpty());
        }
    }

    @Test @Config(qualifiers = "560dpi")
    public void tapSlopScalesWithDensityForOneAndTwoFingers() {
        for (int fingers : new int[] {1, 2}) {
            RecordingConnection connection = new RecordingConnection();
            Game game = game(connection);
            touchConfig(game, TouchMode.TRACKPAD);
            FrameLayout container = gestureContainer(game);
            assertEquals(3.5f, game.getResources().getDisplayMetrics().density, 0);
            beginTap(container, fingers);
            int[] ids = fingers == 1 ? new int[] {0} : new int[] {0, 1};
            float[][] moved = fingers == 1 ? new float[][] {{240, 150}} : new float[][] {{240, 150}, {300, 150}};
            dispatchAt(container, MotionEvent.ACTION_MOVE, 0, ids, 1020, 0, moved);
            // Returning adds 40 px more travel: 80 px < 25 dp at this density.
            finishTap(container, fingers);
            Shadows.shadowOf(Looper.getMainLooper()).idleFor(100, TimeUnit.MILLISECONDS);
            String button = fingers == 1 ? "1" : "3";
            assertEquals(java.util.Arrays.asList("down " + button, "up " + button), connection.buttons);
            assertTrue(connection.scrolls.isEmpty());
        }
    }

    @Test
    public void twoFingerScrollAndDesktopPanOrPinchNeverAlsoClick() {
        for (String gesture : new String[] {"scroll", "pan", "pinch"}) {
            RecordingConnection connection = new RecordingConnection();
            Game game = game(connection);
            touchConfig(game, TouchMode.TRACKPAD);
            FrameLayout container = gestureContainer(game);
            com.limelight.ui.DesktopGestures desktop = gesture.equals("scroll") ? null : enableZoom(game);
            float[] first = {200, 150}, second = {600, 150};
            dispatch(container, MotionEvent.ACTION_DOWN, 0, new int[] {0}, first);
            dispatch(container, MotionEvent.ACTION_POINTER_DOWN, 1, new int[] {0, 1}, first, second);
            if (gesture.equals("pinch")) {
                dispatch(container, MotionEvent.ACTION_MOVE, 0, new int[] {0, 1},
                        new float[] {100, 150}, new float[] {700, 150});
                dispatch(container, MotionEvent.ACTION_MOVE, 0, new int[] {0, 1},
                        new float[] {50, 150}, new float[] {750, 150});
                assertTrue(desktop.viewport().scale() > 2);
            } else {
                dispatch(container, MotionEvent.ACTION_MOVE, 0, new int[] {0, 1},
                        new float[] {200, 250}, new float[] {600, 250});
                if (desktop != null) assertEquals(-100f, desktop.viewport().y(), .01f);
                else assertFalse(connection.scrolls.isEmpty());
            }
            finishTap(container, 2);
            Shadows.shadowOf(Looper.getMainLooper()).idleFor(1, TimeUnit.SECONDS);
            assertTrue(connection.buttons.isEmpty());
            if (desktop != null) assertTrue(connection.scrolls.isEmpty());
        }
    }

    @Test
    public void passivePinchZoomWorksAlongsideTwoFingerScrollAndTransformsDirectTouch() {
        RecordingConnection connection = new RecordingConnection();
        Game game = game(connection);
        PreferenceConfiguration config = touchConfig(game, TouchMode.TRACKPAD);
        config.pinchToZoom = true;
        FrameLayout container = gestureContainer(game);
        com.limelight.ui.StreamView stream = ReflectionHelpers.getField(game, "streamView");
        com.limelight.ui.DesktopGestures desktop = new com.limelight.ui.DesktopGestures(
                game, stream, game.touchInput::cancelHostTouchInput, game.touchInput::clickRightMouseButton);
        desktop.viewport().resize(800, 400);
        ReflectionHelpers.setField(game, "desktopGestures", desktop);
        assertFalse(desktop.enabled());

        // Parallel 2-finger drag at 1.0x passes through to trackpad scroll.
        dispatch(container, MotionEvent.ACTION_DOWN, 0, new int[] {0}, new float[] {200, 150});
        dispatch(container, MotionEvent.ACTION_POINTER_DOWN, 1, new int[] {0, 1},
                new float[] {200, 150}, new float[] {400, 150});
        dispatch(container, MotionEvent.ACTION_MOVE, 0, new int[] {0, 1},
                new float[] {200, 260}, new float[] {400, 260});
        dispatch(container, MotionEvent.ACTION_POINTER_UP, 1, new int[] {0, 1},
                new float[] {200, 260}, new float[] {400, 260});
        dispatch(container, MotionEvent.ACTION_UP, 0, new int[] {0}, new float[] {200, 260});
        assertFalse("expected scroll events at 1.0x", connection.scrolls.isEmpty());
        assertEquals(1.0f, desktop.viewport().scale(), 0f);
        int scrollCountBeforePinch = connection.scrolls.size();

        // Pinching out starts passive zoom without extra scrolls.
        dispatch(container, MotionEvent.ACTION_DOWN, 0, new int[] {0}, new float[] {300, 200});
        dispatch(container, MotionEvent.ACTION_POINTER_DOWN, 1, new int[] {0, 1},
                new float[] {300, 200}, new float[] {500, 200});
        dispatch(container, MotionEvent.ACTION_MOVE, 0, new int[] {0, 1},
                new float[] {200, 200}, new float[] {600, 200});
        dispatch(container, MotionEvent.ACTION_MOVE, 0, new int[] {0, 1},
                new float[] {100, 200}, new float[] {700, 200});
        dispatch(container, MotionEvent.ACTION_POINTER_UP, 1, new int[] {0, 1},
                new float[] {100, 200}, new float[] {700, 200});
        dispatch(container, MotionEvent.ACTION_UP, 0, new int[] {0}, new float[] {100, 200});
        assertTrue(desktop.viewport().scale() > 1.0f);
        assertEquals(scrollCountBeforePinch, connection.scrolls.size());

        // Single-finger Direct touch transforms through zoomInverse even though Zoom toggle is off.
        config.touchscreenTrackpad = false;
        config.touchMode = TouchMode.DIRECT;
        ReflectionHelpers.callInstanceMethod(game.touchInput, "initializeTouchContexts");
        dispatch(container, MotionEvent.ACTION_DOWN, 0, new int[] {0}, new float[] {400, 200});
        dispatch(container, MotionEvent.ACTION_UP, 0, new int[] {0}, new float[] {400, 200});
        assertFalse(connection.positions.isEmpty());
        Shadows.shadowOf(Looper.getMainLooper()).idleFor(100, TimeUnit.MILLISECONDS);
    }

    @Test
    public void desktopTapWobbleDoesNotPanAndClickRespectsModeAreaAndInputCapture() {
        for (String policy : new String[] {"allowed", "off", "native", "direct", "area", "ungrabbed"}) {
            RecordingConnection connection = new RecordingConnection();
            Game game = game(connection);
            int mode = policy.equals("off") ? TouchMode.OFF : policy.equals("native") ? TouchMode.MULTI_TOUCH :
                    policy.equals("direct") ? TouchMode.DIRECT : TouchMode.TRACKPAD;
            PreferenceConfiguration config = touchConfig(game, mode);
            if (policy.equals("area")) config.trackpadArea = "right_half";
            FrameLayout container = gestureContainer(game);
            if (policy.equals("native")) ReflectionHelpers.setField(game, "nativeTouchEnabled", true);
            if (policy.equals("ungrabbed")) ReflectionHelpers.setField(game, "grabbedInput", false);
            com.limelight.ui.DesktopGestures desktop = enableZoom(game);
            beginTap(container, 2);
            dispatch(container, MotionEvent.ACTION_MOVE, 0, new int[] {0, 1},
                    new float[] {200, 158}, new float[] {260, 158});
            finishTap(container, 2);
            Shadows.shadowOf(Looper.getMainLooper()).idleFor(100, TimeUnit.MILLISECONDS);
            assertEquals(-200f, desktop.viewport().y(), 0);
            assertEquals(2f, desktop.viewport().scale(), 0);
            assertEquals(policy, policy.equals("allowed") ? java.util.Arrays.asList("down 3", "up 3") :
                    Collections.emptyList(), connection.buttons);
            assertTrue(connection.heldTouches.isEmpty());
        }
    }

    @Test
    public void repeatedRightTapsAndLifecycleCancellationReleaseEveryButtonExactlyOnce() {
        for (boolean zoom : new boolean[] {false, true}) {
            RecordingConnection connection = new RecordingConnection();
            Game game = game(connection);
            touchConfig(game, TouchMode.TRACKPAD);
            FrameLayout container = gestureContainer(game);
            if (zoom) enableZoom(game);
            for (int tap = 0; tap < 2; tap++) {
                beginTap(container, 2);
                finishTap(container, 2);
            }
            ReflectionHelpers.callInstanceMethod(game.touchInput, "cancelTouchInput");
            Shadows.shadowOf(Looper.getMainLooper()).idleFor(1, TimeUnit.SECONDS);
            assertEquals(java.util.Arrays.asList("down 3", "up 3", "down 3", "up 3"), connection.buttons);
        }
    }

    @Test
    public void threeFingerTapCanMiddleClick() {
        RecordingConnection connection = new RecordingConnection();
        Game game = game(connection);
        touchConfig(game, TouchMode.TRACKPAD).threeFingerTap = "middle_click";
        FrameLayout container = streamContainer(game, null);
        ReflectionHelpers.callInstanceMethod(game.touchInput, "initializeTouchContexts");
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
        ReflectionHelpers.callInstanceMethod(game.touchInput, "initializeTouchContexts");
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

    @Test
    public void aCancelRunsTheFingerTapOnlyWhenTheSettingIsOn() {
        for (boolean on : new boolean[] {false, true}) {
            RecordingConnection connection = new RecordingConnection();
            Game game = game(connection);
            PreferenceConfiguration config = touchConfig(game, TouchMode.TRACKPAD);
            config.threeFingerTap = "middle_click";
            config.fingerTapOnCancel = on;
            FrameLayout container = streamContainer(game, null);
            ReflectionHelpers.callInstanceMethod(game.touchInput, "initializeTouchContexts");
            beginTap(container, 3);
            dispatchAt(container, MotionEvent.ACTION_CANCEL, 0, new int[] {0, 1, 2}, 1045,
                    MotionEvent.FLAG_CANCELED, point(0), point(1), point(2));
            Shadows.shadowOf(Looper.getMainLooper()).idleFor(200, TimeUnit.MILLISECONDS);
            assertEquals(on ? java.util.Arrays.asList("down 2", "up 2") : java.util.Collections.emptyList(),
                    connection.buttons);
        }
    }

    @Test @Config(sdk = 34)
    public void fromAndroid13OnlyAFlaggedCancelRunsTheFingerTap() {
        for (int flags : new int[] {0, MotionEvent.FLAG_CANCELED}) {
            RecordingConnection connection = new RecordingConnection();
            Game game = game(connection);
            PreferenceConfiguration config = touchConfig(game, TouchMode.TRACKPAD);
            config.threeFingerTap = "middle_click";
            config.fingerTapOnCancel = true;
            FrameLayout container = streamContainer(game, null);
            ReflectionHelpers.callInstanceMethod(game.touchInput, "initializeTouchContexts");
            beginTap(container, 3);
            dispatchAt(container, MotionEvent.ACTION_CANCEL, 0, new int[] {0, 1, 2}, 1045,
                    flags, point(0), point(1), point(2));
            Shadows.shadowOf(Looper.getMainLooper()).idleFor(200, TimeUnit.MILLISECONDS);
            assertEquals(flags == 0 ? java.util.Collections.emptyList() : java.util.Arrays.asList("down 2", "up 2"),
                    connection.buttons);
        }
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
        FingerTapGesture tap = ReflectionHelpers.getField(game.touchInput, "fingerTap");
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
            ReflectionHelpers.callInstanceMethod(game.keyboardInput, "toggleKeyboardOverlay");
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
                ReflectionHelpers.callInstanceMethod(game.touchInput, "cancelTouchInput");
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

    @Test public void multiTouchRunsFingerTapsOnlyWhenTheSettingIsOn() {
        for (int fingers : new int[] {3, 4}) {
            RecordingConnection connection = new RecordingConnection();
            Game game = game(connection);
            PreferenceConfiguration config = touchConfig(game, TouchMode.MULTI_TOUCH);
            config.threeFingerTap = fingers == 3 ? "desktop_controls" : "none";
            config.fourFingerTap = fingers == 4 ? "desktop_controls" : "none";
            config.multiTouchFingerTaps = true;
            FrameLayout container = gestureContainer(game);
            ReflectionHelpers.setField(game, "nativeTouchEnabled", true);
            beginTap(container, fingers);
            // The first two fingers reached the PC at once; the third took them back.
            assertEquals("Two downs and the cancel", 3, connection.touches.size());
            assertTrue(connection.heldTouches.isEmpty());
            finishTap(container, fingers);
            View panel = ReflectionHelpers.getField(game, "keyboardOverlay");
            assertNotNull(panel);
            assertEquals(View.VISIBLE, panel.getVisibility());
            assertEquals("Nothing after the cancel reaches the PC", 3, connection.touches.size());

            // One finger afterwards is a touch on the PC again.
            dispatch(container, MotionEvent.ACTION_DOWN, 0, new int[] {0}, point(0));
            assertEquals(1, connection.heldTouches.size());
            dispatch(container, MotionEvent.ACTION_UP, 0, new int[] {0}, point(0));
            assertTrue(connection.heldTouches.isEmpty());
        }
    }

    @Test public void aThirdFingerBesideHeldThumbsStaysATouchInMultiTouch() {
        RecordingConnection connection = new RecordingConnection();
        Game game = game(connection);
        PreferenceConfiguration config = touchConfig(game, TouchMode.MULTI_TOUCH);
        config.threeFingerTap = "desktop_controls";
        config.multiTouchFingerTaps = true;
        FrameLayout container = gestureContainer(game);
        ReflectionHelpers.setField(game, "nativeTouchEnabled", true);
        beginTap(container, 2);
        // A second later, a brief third finger: play, not a tap.
        int[] three = {0, 1, 2};
        dispatchAt(container, MotionEvent.ACTION_POINTER_DOWN, 2, three, 2000, 0, point(0), point(1), point(2));
        assertEquals("No cancel: the thumbs stay held", 3, connection.heldTouches.size());
        dispatchAt(container, MotionEvent.ACTION_POINTER_UP, 2, three, 2050, 0, point(0), point(1), point(2));
        assertEquals(2, connection.heldTouches.size());
        View panel = ReflectionHelpers.getField(game, "keyboardOverlay");
        assertTrue(panel == null || panel.getVisibility() != View.VISIBLE);
        finishTap(container, 2, 2100, 0);
        assertTrue(connection.heldTouches.isEmpty());
    }

    @Test
    public void passivePinchZoomIsOffByDefaultAndLiveControlCenterZoomResetsWhenTurnedOff() {
        RecordingConnection connection = new RecordingConnection();
        Game game = game(connection);
        PreferenceConfiguration config = touchConfig(game, TouchMode.TRACKPAD);
        assertFalse("pinchToZoom defaults to false", config.pinchToZoom);
        FrameLayout container = gestureContainer(game);
        com.limelight.ui.StreamView stream = ReflectionHelpers.getField(game, "streamView");
        com.limelight.ui.DesktopGestures desktop = new com.limelight.ui.DesktopGestures(
                game, stream, game.touchInput::cancelHostTouchInput, game.touchInput::clickRightMouseButton);
        desktop.viewport().resize(800, 400);
        ReflectionHelpers.setField(game, "desktopGestures", desktop);

        // With checkbox_pinch_to_zoom=false and Control Center Zoom=false, wide 2-finger pinch does not zoom or cancel.
        dispatch(container, MotionEvent.ACTION_DOWN, 0, new int[] {0}, new float[] {300, 200});
        dispatch(container, MotionEvent.ACTION_POINTER_DOWN, 1, new int[] {0, 1},
                new float[] {300, 200}, new float[] {500, 200});
        dispatch(container, MotionEvent.ACTION_MOVE, 0, new int[] {0, 1},
                new float[] {200, 200}, new float[] {600, 200});
        dispatch(container, MotionEvent.ACTION_MOVE, 0, new int[] {0, 1},
                new float[] {100, 200}, new float[] {700, 200});
        dispatch(container, MotionEvent.ACTION_POINTER_UP, 1, new int[] {0, 1},
                new float[] {100, 200}, new float[] {700, 200});
        dispatch(container, MotionEvent.ACTION_UP, 0, new int[] {0}, new float[] {100, 200});
        assertEquals(1.0f, desktop.viewport().scale(), 0f);
        assertFalse(desktop.viewport().isTransformed());

        // Enabling Control Center Zoom works live even while pref pinchToZoom=false, and disabling resets cleanly.
        desktop.setEnabled(true);
        dispatch(container, MotionEvent.ACTION_DOWN, 0, new int[] {0}, new float[] {300, 200});
        dispatch(container, MotionEvent.ACTION_POINTER_DOWN, 1, new int[] {0, 1},
                new float[] {300, 200}, new float[] {500, 200});
        dispatch(container, MotionEvent.ACTION_MOVE, 0, new int[] {0, 1},
                new float[] {200, 200}, new float[] {600, 200});
        dispatch(container, MotionEvent.ACTION_POINTER_UP, 1, new int[] {0, 1},
                new float[] {200, 200}, new float[] {600, 200});
        dispatch(container, MotionEvent.ACTION_UP, 0, new int[] {0}, new float[] {200, 200});
        assertTrue(desktop.viewport().scale() > 1.0f);
        desktop.setEnabled(false);
        assertEquals(1.0f, desktop.viewport().scale(), 0f);
        assertFalse(desktop.viewport().isTransformed());
    }
}
