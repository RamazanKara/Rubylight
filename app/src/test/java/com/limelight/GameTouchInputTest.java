package com.limelight;

import android.app.Application;
import android.os.Looper;
import android.view.InputDevice;
import android.view.MotionEvent;
import android.view.View;
import android.widget.FrameLayout;

import com.limelight.binding.input.HeldInputState;
import com.limelight.binding.input.TouchLayout;
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
        container.addView(controls);
        container.measure(View.MeasureSpec.makeMeasureSpec(800, View.MeasureSpec.EXACTLY),
                View.MeasureSpec.makeMeasureSpec(400, View.MeasureSpec.EXACTLY));
        container.layout(0, 0, 800, 400);
        ReflectionHelpers.setField(game, "streamView", stream);
        return container;
    }

    private static void dispatch(View view, int action, int actionIndex, int[] ids, float[]... points) {
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
        MotionEvent event = MotionEvent.obtain(1000, 1010, masked, points.length, properties, coords, 0, 0, 1, 1,
                0, 0, InputDevice.SOURCE_TOUCHSCREEN, 0);
        try { view.dispatchTouchEvent(event); } finally { event.recycle(); }
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
        @Override public void sendMouseButtonDown(byte button) { }
        @Override public void sendMouseButtonUp(byte button) { }
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
