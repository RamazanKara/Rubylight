package com.limelight.ui;

import android.app.Application;
import android.os.Looper;
import android.view.MotionEvent;

import com.limelight.binding.input.DesktopControlsConfig;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;

import java.time.Duration;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;

import static org.junit.Assert.*;
import static org.robolectric.Shadows.shadowOf;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class DesktopTrackpadTest {
    private static final class Sink implements DesktopTrackpad.Sink {
        final List<float[]> moves = new ArrayList<>(), scrolls = new ArrayList<>();
        final List<String> buttons = new ArrayList<>();
        @Override public void move(float x, float y) { moves.add(new float[] {x, y}); }
        @Override public void scroll(float vertical, float horizontal) { scrolls.add(new float[] {vertical, horizontal}); }
        @Override public void mouseButton(int button, boolean down) { buttons.add(button + ":" + down); }
    }

    @Test public void movementUsesSensitivityAndNeverClicksAfterDragging() {
        try (ActivityController<UiNavigationTest.Screen> screen = Robolectric.buildActivity(UiNavigationTest.Screen.class).setup()) {
            DesktopControlsConfig config = new DesktopControlsConfig();
            config.pointerSpeedPercent = 200;
            Sink sink = new Sink();
            DesktopTrackpad pad = new DesktopTrackpad(screen.get(), config, sink);
            event(pad, 0, MotionEvent.ACTION_DOWN, 10, 10);
            event(pad, 50, MotionEvent.ACTION_MOVE, 30, 40);
            event(pad, 90, MotionEvent.ACTION_UP, 30, 40);
            float density = screen.get().getResources().getDisplayMetrics().density;
            assertArrayEquals(new float[] {40 / density, 60 / density}, sink.moves.get(0), .001f);
            assertTrue(sink.buttons.isEmpty());
        }
    }

    @Test public void tapReleasesAfter100msAndCancelClearsPendingCallbacks() {
        try (ActivityController<UiNavigationTest.Screen> screen = Robolectric.buildActivity(UiNavigationTest.Screen.class).setup()) {
            Sink sink = new Sink();
            DesktopTrackpad pad = new DesktopTrackpad(screen.get(), new DesktopControlsConfig(), sink);
            event(pad, 0, MotionEvent.ACTION_DOWN, 10, 10);
            event(pad, 60, MotionEvent.ACTION_UP, 10, 10);
            assertEquals(Arrays.asList("1:true"), sink.buttons);
            pad.releaseInputs();
            assertEquals(Arrays.asList("1:true", "1:false"), sink.buttons);
            shadowOf(Looper.getMainLooper()).idleFor(Duration.ofMillis(200));
            assertEquals(2, sink.buttons.size());
            event(pad, 300, MotionEvent.ACTION_DOWN, 10, 10);
            event(pad, 350, MotionEvent.ACTION_CANCEL, 10, 10);
            event(pad, 400, MotionEvent.ACTION_UP, 10, 10);
            assertEquals(2, sink.buttons.size());
        }
    }

    @Test public void twoFingerScrollHasNoPointerJumpOrClickWhenOneFingerLifts() {
        try (ActivityController<UiNavigationTest.Screen> screen = Robolectric.buildActivity(UiNavigationTest.Screen.class).setup()) {
            Sink sink = new Sink();
            DesktopControlsConfig config = new DesktopControlsConfig();
            config.naturalScroll = false;
            DesktopTrackpad pad = new DesktopTrackpad(screen.get(), config, sink);
            event(pad, 0, MotionEvent.ACTION_DOWN, 10, 10);
            multi(pad, 20, MotionEvent.ACTION_POINTER_DOWN | (1 << MotionEvent.ACTION_POINTER_INDEX_SHIFT), 10, 10, 30, 10);
            multi(pad, 80, MotionEvent.ACTION_MOVE, 50, 50, 70, 50);
            multi(pad, 100, MotionEvent.ACTION_POINTER_UP | (1 << MotionEvent.ACTION_POINTER_INDEX_SHIFT), 50, 50, 70, 50);
            event(pad, 120, MotionEvent.ACTION_MOVE, 80, 50);
            event(pad, 150, MotionEvent.ACTION_UP, 80, 50);
            assertTrue(sink.moves.isEmpty());
            assertTrue(sink.buttons.isEmpty());
            assertEquals(1, sink.scrolls.size());
            assertEquals(-120 / screen.get().getResources().getDisplayMetrics().density, sink.scrolls.get(0)[0], .001f);
            assertEquals(120 / screen.get().getResources().getDisplayMetrics().density, sink.scrolls.get(0)[1], .001f);
        }
    }

    @Test public void twoFingerTapUsesRightButtonAndTapPreferenceCanDisableIt() {
        try (ActivityController<UiNavigationTest.Screen> screen = Robolectric.buildActivity(UiNavigationTest.Screen.class).setup()) {
            for (boolean enabled : new boolean[] {true, false}) {
                Sink sink = new Sink();
                DesktopControlsConfig config = new DesktopControlsConfig();
                config.tapToClick = enabled;
                DesktopTrackpad pad = new DesktopTrackpad(screen.get(), config, sink);
                event(pad, 0, MotionEvent.ACTION_DOWN, 10, 10);
                multi(pad, 20, MotionEvent.ACTION_POINTER_DOWN | (1 << MotionEvent.ACTION_POINTER_INDEX_SHIFT), 10, 10, 30, 10);
                multi(pad, 80, MotionEvent.ACTION_POINTER_UP | (1 << MotionEvent.ACTION_POINTER_INDEX_SHIFT), 10, 10, 30, 10);
                event(pad, 100, MotionEvent.ACTION_UP, 10, 10);
                shadowOf(Looper.getMainLooper()).idleFor(Duration.ofMillis(200));
                assertEquals(enabled ? Arrays.asList("3:true", "3:false") : new ArrayList<String>(), sink.buttons);
            }
        }
    }

    private static void event(DesktopTrackpad pad, long time, int action, float x, float y) {
        MotionEvent event = MotionEvent.obtain(0, time, action, x, y, 0);
        try { pad.onTouchEvent(event); } finally { event.recycle(); }
    }

    private static void multi(DesktopTrackpad pad, long time, int action, float x, float y, float x2, float y2) {
        MotionEvent.PointerProperties[] properties = new MotionEvent.PointerProperties[2];
        MotionEvent.PointerCoords[] coordinates = new MotionEvent.PointerCoords[2];
        for (int i = 0; i < 2; i++) {
            properties[i] = new MotionEvent.PointerProperties();
            properties[i].id = i;
            properties[i].toolType = MotionEvent.TOOL_TYPE_FINGER;
            coordinates[i] = new MotionEvent.PointerCoords();
            coordinates[i].x = i == 0 ? x : x2;
            coordinates[i].y = i == 0 ? y : y2;
        }
        MotionEvent event = MotionEvent.obtain(0, time, action, 2, properties, coordinates,
                0, 0, 1, 1, 0, 0, android.view.InputDevice.SOURCE_TOUCHSCREEN, 0);
        try { pad.onTouchEvent(event); } finally { event.recycle(); }
    }
}
