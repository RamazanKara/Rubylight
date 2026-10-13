package com.limelight;

import android.app.Application;
import android.os.Looper;
import android.view.InputDevice;
import android.view.MotionEvent;
import android.view.View;

import com.limelight.nvstream.NvConnection;
import com.limelight.nvstream.StreamConfiguration;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.preferences.PreferenceConfiguration;
import com.limelight.ui.StreamView;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.Shadows;
import org.robolectric.annotation.Config;
import org.robolectric.util.ReflectionHelpers;

import java.util.ArrayList;
import java.util.List;
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
            return 0;
        }
    }
}
