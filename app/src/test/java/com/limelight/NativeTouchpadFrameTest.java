package com.limelight;

import android.app.Application;
import android.view.InputDevice;
import android.view.MotionEvent;

import com.limelight.nvstream.NvConnection;
import com.limelight.nvstream.StreamConfiguration;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.preferences.PreferenceConfiguration;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;
import org.robolectric.util.ReflectionHelpers;

import java.util.ArrayList;
import java.util.List;

import static org.junit.Assert.*;

/**
 * A physical touchpad sends 125-250 frames a second: each one reuses the same buffers instead of allocating
 * four arrays, and still carries only the fingers still on the touchpad.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class,
        shadows = PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class NativeTouchpadFrameTest {
    private static final class Frame {
        int[] ids;
        float[] xs;
        float[] ys;
        float[] pressures;
        int count;
    }

    private static final class RecordingConnection extends NvConnection {
        final List<Frame> frames = new ArrayList<>();

        RecordingConnection() {
            super(null, new ComputerDetails.AddressTuple("127.0.0.1", 47989), 47984, "test",
                    new StreamConfiguration.Builder().build(), null, null);
        }

        @Override
        public int sendTouchpadFrame(int device, int[] ids, float[] xs, float[] ys, float[] pressures, int count,
                                     boolean button, short widthMm, short heightMm) {
            Frame frame = new Frame();
            frame.ids = ids;
            frame.xs = xs;
            frame.ys = ys;
            frame.pressures = pressures;
            frame.count = count;
            frames.add(frame);
            return 0;
        }
    }

    private static MotionEvent event(int action, int actionIndex, int[] ids) {
        MotionEvent.PointerProperties[] properties = new MotionEvent.PointerProperties[ids.length];
        MotionEvent.PointerCoords[] coords = new MotionEvent.PointerCoords[ids.length];
        for (int i = 0; i < ids.length; i++) {
            properties[i] = new MotionEvent.PointerProperties();
            properties[i].id = ids[i];
            properties[i].toolType = MotionEvent.TOOL_TYPE_FINGER;
            coords[i] = new MotionEvent.PointerCoords();
            coords[i].x = 0.25f + i;
            coords[i].y = 0.5f;
            coords[i].pressure = 1;
        }
        int masked = action == MotionEvent.ACTION_POINTER_DOWN || action == MotionEvent.ACTION_POINTER_UP ?
                action | (actionIndex << MotionEvent.ACTION_POINTER_INDEX_SHIFT) : action;
        return MotionEvent.obtain(1000, 1010, masked, ids.length, properties, coords, 0, 0, 1, 1,
                0, 0, InputDevice.SOURCE_TOUCHPAD, 0);
    }

    private static void send(GameTouchInput input, MotionEvent event) {
        try {
            ReflectionHelpers.callInstanceMethod(input, "sendNativeTouchpadFrame",
                    ReflectionHelpers.ClassParameter.from(MotionEvent.class, event));
        } finally {
            event.recycle();
        }
    }

    @Test
    public void aTouchpadFrameReusesItsBuffersAndLeavesOutTheLiftedFinger() {
        RecordingConnection connection = new RecordingConnection();
        Game game = Robolectric.buildActivity(Game.class).get();
        ReflectionHelpers.setField(game, "conn", connection);
        PreferenceConfiguration config = new PreferenceConfiguration();
        config.nativeTouchpad = true;
        ReflectionHelpers.setField(game, "prefConfig", config);
        GameTouchInput input = new GameTouchInput(game);

        send(input, event(MotionEvent.ACTION_MOVE, 0, new int[] {4, 9}));
        assertEquals(1, connection.frames.size());
        Frame both = connection.frames.get(0);
        assertEquals(2, both.count);
        assertEquals(4, both.ids[0]);
        assertEquals(9, both.ids[1]);
        assertEquals(1f, both.pressures[0], 0f);

        send(input, event(MotionEvent.ACTION_POINTER_UP, 0, new int[] {4, 9}));
        assertEquals(2, connection.frames.size());
        Frame one = connection.frames.get(1);
        assertEquals("the lifted finger is not sent", 1, one.count);
        assertEquals(9, one.ids[0]);
        // The very same arrays: a touchpad event allocates none of them.
        assertSame(both.ids, one.ids);
        assertSame(both.xs, one.xs);
        assertSame(both.ys, one.ys);
        assertSame(both.pressures, one.pressures);
    }
}
