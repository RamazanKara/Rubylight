package com.limelight;

import android.app.Application;
import android.view.InputDevice;
import android.view.MotionEvent;
import android.view.View;

import com.limelight.binding.input.capture.InputCaptureProvider;
import com.limelight.nvstream.NvConnection;
import com.limelight.nvstream.StreamConfiguration;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.preferences.PreferenceConfiguration;
import com.limelight.ui.DesktopGestures;
import com.limelight.ui.StreamView;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.Shadows;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowInputDevice;
import org.robolectric.util.ReflectionHelpers;

import java.util.ArrayList;
import java.util.List;

import static org.junit.Assert.*;

/**
 * Fingers and pens passed to the host as they are: every value of every pointer is the value the
 * host was sent before those values stopped being worked out in arrays made for each pointer.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class,
        shadows = PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class NativeTouchValuesTest {
    private static final short NO_ROTATION = MoonBridge.LI_ROT_UNKNOWN;

    // Float.toString tells any two different floats apart, so packets that read the same are the
    // same bit for bit.
    private static String touch(byte event, int pointer, float x, float y, float pressure,
                                float major, float minor, short rotation) {
        return "touch " + event + " pointer " + pointer + " at " + x + "," + y + " pressure " + pressure +
                " area " + major + "x" + minor + " rotation " + rotation;
    }

    private static String pen(byte event, byte tool, byte buttons, float x, float y, float pressure,
                              float major, float minor, short rotation, byte tilt) {
        return "pen " + event + " tool " + tool + " buttons " + buttons + " at " + x + "," + y +
                " pressure " + pressure + " area " + major + "x" + minor + " rotation " + rotation + " tilt " + tilt;
    }

    private static final class Sent extends NvConnection {
        final List<String> packets = new ArrayList<>();
        // The place, pressure and contact area of each packet, to check against plain numbers.
        final List<float[]> values = new ArrayList<>();

        Sent() {
            super(null, new ComputerDetails.AddressTuple("127.0.0.1", 47989), 47984, "test",
                    new StreamConfiguration.Builder().build(), null, null);
        }

        @Override public boolean canSendInput() { return true; }

        @Override public int sendTouchEvent(byte event, int pointer, float x, float y, float pressure,
                                            float major, float minor, short rotation) {
            packets.add(touch(event, pointer, x, y, pressure, major, minor, rotation));
            values.add(new float[] {x, y, pressure, major, minor});
            return 0;
        }

        @Override public int sendPenEvent(byte event, byte tool, byte buttons, float x, float y, float pressure,
                                          float major, float minor, short rotation, byte tilt) {
            packets.add(pen(event, tool, buttons, x, y, pressure, major, minor, rotation, tilt));
            values.add(new float[] {x, y, pressure, major, minor});
            return 0;
        }
    }

    // The arithmetic the client had for where a pointer is, with the array it made for each pointer.
    private static float[] placeBefore(View view, View stream, MotionEvent event, int pointerIndex) {
        float normalizedX = event.getX(pointerIndex) + (view.getX() - stream.getX());
        float normalizedY = event.getY(pointerIndex) + (view.getY() - stream.getY());

        normalizedX = Math.max(normalizedX, 0.0f);
        normalizedY = Math.max(normalizedY, 0.0f);

        normalizedX = Math.min(normalizedX, stream.getWidth());
        normalizedY = Math.min(normalizedY, stream.getHeight());

        normalizedX /= stream.getWidth();
        normalizedY /= stream.getHeight();

        return new float[] { normalizedX, normalizedY };
    }

    private static float[] polarToCartesian(float r, float theta) {
        return new float[] { (float)(r * Math.cos(theta)), (float)(r * Math.sin(theta)) };
    }

    private static float cartesianToR(float[] point) {
        return (float)Math.sqrt(Math.pow(point[0], 2) + Math.pow(point[1], 2));
    }

    // And the arithmetic it had for the contact area, with the three arrays it made for each pointer.
    private static float[] contactAreaBefore(View stream, MotionEvent event, int pointerIndex) {
        float orientation;
        if (event.getDevice() == null || event.getDevice().getMotionRange(MotionEvent.AXIS_ORIENTATION, event.getSource()) == null) {
            orientation = (float)(Math.PI / 4);
        }
        else {
            orientation = event.getOrientation(pointerIndex);
        }

        float contactAreaMajor, contactAreaMinor;
        switch (event.getActionMasked()) {
            case MotionEvent.ACTION_HOVER_ENTER:
            case MotionEvent.ACTION_HOVER_MOVE:
            case MotionEvent.ACTION_HOVER_EXIT:
                contactAreaMajor = event.getToolMajor(pointerIndex);
                contactAreaMinor = event.getToolMinor(pointerIndex);
                break;

            default:
                contactAreaMajor = event.getTouchMajor(pointerIndex);
                contactAreaMinor = event.getTouchMinor(pointerIndex);
                break;
        }

        float[] contactAreaMajorCartesian = polarToCartesian(contactAreaMajor, orientation);
        float[] contactAreaMinorCartesian = polarToCartesian(contactAreaMinor, (float)(orientation + (Math.PI / 2)));

        contactAreaMajorCartesian[0] = Math.min(Math.abs(contactAreaMajorCartesian[0]), stream.getWidth()) / stream.getWidth();
        contactAreaMinorCartesian[0] = Math.min(Math.abs(contactAreaMinorCartesian[0]), stream.getWidth()) / stream.getWidth();
        contactAreaMajorCartesian[1] = Math.min(Math.abs(contactAreaMajorCartesian[1]), stream.getHeight()) / stream.getHeight();
        contactAreaMinorCartesian[1] = Math.min(Math.abs(contactAreaMinorCartesian[1]), stream.getHeight()) / stream.getHeight();

        return new float[] { cartesianToR(contactAreaMajorCartesian), cartesianToR(contactAreaMinorCartesian) };
    }

    private static String touchBefore(View view, View stream, MotionEvent event, byte type, int pointerIndex, short rotation) {
        float[] place = placeBefore(view, stream, event, pointerIndex);
        float[] area = contactAreaBefore(stream, event, pointerIndex);
        return touch(type, event.getPointerId(pointerIndex), place[0], place[1], event.getPressure(pointerIndex),
                area[0], area[1], rotation);
    }

    private static String penBefore(View view, View stream, MotionEvent event, byte type, byte tool, byte buttons,
                                    float pressure, int pointerIndex, short rotation, byte tilt) {
        float[] place = placeBefore(view, stream, event, pointerIndex);
        float[] area = contactAreaBefore(stream, event, pointerIndex);
        return pen(type, tool, buttons, place[0], place[1], pressure, area[0], area[1], rotation, tilt);
    }

    /** A finger or pen on the screen; its values change between the events of a gesture. */
    private static final class Pointer {
        final MotionEvent.PointerProperties properties = new MotionEvent.PointerProperties();
        final MotionEvent.PointerCoords coords = new MotionEvent.PointerCoords();

        Pointer(int id, int tool, float x, float y) {
            properties.id = id;
            properties.toolType = tool;
            at(x, y);
        }

        Pointer at(float x, float y) { coords.x = x; coords.y = y; return this; }
        Pointer pressure(float pressure) { coords.pressure = pressure; return this; }
        Pointer touching(float major, float minor) { coords.touchMajor = major; coords.touchMinor = minor; return this; }
        Pointer tool(float major, float minor) { coords.toolMajor = major; coords.toolMinor = minor; return this; }
        Pointer lying(float orientation) { coords.orientation = orientation; return this; }
        Pointer axis(int axis, float value) { coords.setAxisValue(axis, value); return this; }
    }

    // An event of the device, or of a screen that reports nothing about itself when there is none.
    private static MotionEvent event(int action, int actionIndex, InputDevice device, int source, int buttons, Pointer... pointers) {
        MotionEvent.PointerProperties[] properties = new MotionEvent.PointerProperties[pointers.length];
        MotionEvent.PointerCoords[] coords = new MotionEvent.PointerCoords[pointers.length];
        for (int i = 0; i < pointers.length; i++) {
            properties[i] = pointers[i].properties;
            coords[i] = pointers[i].coords;
        }
        int masked = action == MotionEvent.ACTION_POINTER_DOWN || action == MotionEvent.ACTION_POINTER_UP ?
                action | (actionIndex << MotionEvent.ACTION_POINTER_INDEX_SHIFT) : action;
        MotionEvent event = MotionEvent.obtain(1000, 1010, masked, pointers.length, properties, coords, 0, buttons, 1, 1,
                device != null ? device.getId() : 0, 0, source, 0);
        // Robolectric's events have the device they are given and look none up.
        Shadows.shadowOf(event).setDevice(device);
        return event;
    }

    private static void touch(Game game, View view, MotionEvent event) {
        try {
            assertTrue(game.onTouch(view, event));
        } finally {
            recycle(event);
        }
    }

    // Events are reused, and the next test to get this one must not find this test's device on it.
    private static void recycle(MotionEvent event) {
        Shadows.shadowOf(event).setDevice(null);
        event.recycle();
    }

    private static Game game(Sent connection) {
        Game game = Robolectric.buildActivity(Game.class).get();
        ReflectionHelpers.setField(game, "conn", connection);
        ReflectionHelpers.setField(game, "prefConfig", new PreferenceConfiguration());
        ReflectionHelpers.setField(game, "grabbedInput", true);
        ReflectionHelpers.setField(game, "foreground", true);
        ReflectionHelpers.setField(game, "connected", true);
        ReflectionHelpers.setField(game, "nativeTouchEnabled", true);
        return game;
    }

    // The stream is 800 by 400, 100 right of and 50 below the view the events arrive on.
    private static View layout(Game game) {
        View background = new View(game);
        background.layout(1050, 600, 2050, 1100);
        StreamView stream = new StreamView(game);
        stream.layout(1150, 650, 1950, 1050);
        ReflectionHelpers.setField(game, "streamView", stream);
        return background;
    }

    // An input device that reports the given axes, each as {axis, minimum, maximum}, for a source.
    private static InputDevice device(int id, int sources, int rangeSource, float[]... ranges) {
        InputDevice device = ShadowInputDevice.makeInputDeviceNamed("Screen " + id);
        ReflectionHelpers.setField(device, "mId", id);
        ReflectionHelpers.setField(device, "mDescriptor", "screen-" + id);
        ReflectionHelpers.setField(device, "mSources", sources);
        ReflectionHelpers.setField(device, "mMotionRanges", new ArrayList<InputDevice.MotionRange>());
        for (float[] range : ranges) {
            ReflectionHelpers.callInstanceMethod(device, "addMotionRange",
                    ReflectionHelpers.ClassParameter.from(int.class, (int) range[0]),
                    ReflectionHelpers.ClassParameter.from(int.class, rangeSource),
                    ReflectionHelpers.ClassParameter.from(float.class, range[1]),
                    ReflectionHelpers.ClassParameter.from(float.class, range[2]),
                    ReflectionHelpers.ClassParameter.from(float.class, 0f),
                    ReflectionHelpers.ClassParameter.from(float.class, 0f),
                    ReflectionHelpers.ClassParameter.from(float.class, 0f));
        }
        return device;
    }

    @Test
    public void everyFingerIsSentWithThePlacePressureAndContactAreaItHadBefore() {
        Sent sent = new Sent();
        Game game = game(sent);
        View background = layout(game);
        List<String> expected = new ArrayList<>();

        Pointer thumb = new Pointer(0, MotionEvent.TOOL_TYPE_FINGER, 500, 250).pressure(.4f).touching(30, 12).lying(.6f);
        Pointer index = new Pointer(3, MotionEvent.TOOL_TYPE_FINGER, 140, 90).pressure(.7f).touching(1200, 20).lying(-1.1f);

        MotionEvent event = event(MotionEvent.ACTION_DOWN, 0, null, InputDevice.SOURCE_TOUCHSCREEN, 0, thumb);
        assertNull("This screen does not say how a finger lies", event.getDevice());
        assertEquals(30f, event.getTouchMajor(0), 0);
        assertEquals(12f, event.getTouchMinor(0), 0);
        expected.add(touchBefore(background, game.streamView, event, MoonBridge.LI_TOUCH_EVENT_DOWN, 0, NO_ROTATION));
        touch(game, background, event);
        // The middle of the stream. A finger whose angle is unknown is taken to lie at 45 degrees:
        // 30 pixels are 21.2 across of 800 and 21.2 down of 400.
        assertArrayEquals(new float[] {.5f, .5f, .4f, .0592927f, .0237171f}, sent.values.get(0), .000001f);

        event = event(MotionEvent.ACTION_POINTER_DOWN, 1, null, InputDevice.SOURCE_TOUCHSCREEN, 0, thumb, index);
        expected.add(touchBefore(background, game.streamView, event, MoonBridge.LI_TOUCH_EVENT_DOWN, 1, NO_ROTATION));
        touch(game, background, event);
        // A contact wider and taller than the stream covers all of it in both directions.
        assertArrayEquals(new float[] {.05f, .1f, .7f, 1.4142135f, .0395285f}, sent.values.get(1), .000001f);

        // The thumb slides out of the stream at its lower left; a move is sent for every finger.
        thumb.at(20, 700).pressure(.9f).touching(34, 15);
        index.at(141.5f, 92.25f);
        event = event(MotionEvent.ACTION_MOVE, 0, null, InputDevice.SOURCE_TOUCHSCREEN, 0, thumb, index);
        expected.add(touchBefore(background, game.streamView, event, MoonBridge.LI_TOUCH_EVENT_MOVE, 0, NO_ROTATION));
        expected.add(touchBefore(background, game.streamView, event, MoonBridge.LI_TOUCH_EVENT_MOVE, 1, NO_ROTATION));
        touch(game, background, event);
        assertEquals(0f, sent.values.get(2)[0], 0);
        assertEquals(1f, sent.values.get(2)[1], 0);
        assertEquals(.9f, sent.values.get(2)[2], 0);

        event = event(MotionEvent.ACTION_POINTER_UP, 0, null, InputDevice.SOURCE_TOUCHSCREEN, 0, thumb, index);
        expected.add(touchBefore(background, game.streamView, event, MoonBridge.LI_TOUCH_EVENT_UP, 0, NO_ROTATION));
        touch(game, background, event);

        event = event(MotionEvent.ACTION_UP, 0, null, InputDevice.SOURCE_TOUCHSCREEN, 0, index);
        expected.add(touchBefore(background, game.streamView, event, MoonBridge.LI_TOUCH_EVENT_UP, 0, NO_ROTATION));
        touch(game, background, event);

        assertEquals(6, expected.size());
        assertEquals(expected, sent.packets);
        assertTrue(sent.packets.get(5), sent.packets.get(5).startsWith("touch 2 pointer 3 at "));
    }

    @Test
    public void aScreenThatSaysHowAFingerLiesHasThatAngleSentAndUsedForTheContactArea() {
        Sent sent = new Sent();
        Game game = game(sent);
        View background = layout(game);
        InputDevice screen = device(11, InputDevice.SOURCE_TOUCHSCREEN, InputDevice.SOURCE_TOUCHSCREEN,
                new float[] {MotionEvent.AXIS_ORIENTATION, (float) -Math.PI / 2, (float) Math.PI / 2});
        List<String> expected = new ArrayList<>();

        // Upright: the long axis of the contact runs across the stream and the short one down it.
        Pointer finger = new Pointer(0, MotionEvent.TOOL_TYPE_FINGER, 300, 150).pressure(.5f).touching(40, 20).lying(0);
        MotionEvent event = event(MotionEvent.ACTION_DOWN, 0, screen, InputDevice.SOURCE_TOUCHSCREEN, 0, finger);
        assertSame(screen, event.getDevice());
        expected.add(touchBefore(background, game.streamView, event, MoonBridge.LI_TOUCH_EVENT_DOWN, 0, (short) 0));
        touch(game, background, event);
        assertArrayEquals(new float[] {.25f, .25f, .5f, .05f, .05f}, sent.values.get(0), .000001f);

        // Turned to the left; the host is sent angles from 0 to 360 degrees.
        finger.at(310, 160).lying(-.5f);
        event = event(MotionEvent.ACTION_MOVE, 0, screen, InputDevice.SOURCE_TOUCHSCREEN, 0, finger);
        assertEquals(-.5f, event.getOrientation(0), 0);
        expected.add(touchBefore(background, game.streamView, event, MoonBridge.LI_TOUCH_EVENT_MOVE, 0, (short) 332));
        touch(game, background, event);
        // 40 pixels at -0.5 radians are 35.1 across of 800 and 19.2 down of 400.
        assertEquals(.0649913f, sent.values.get(1)[3], .000001f);

        finger.lying(1.2f);
        event = event(MotionEvent.ACTION_UP, 0, screen, InputDevice.SOURCE_TOUCHSCREEN, 0, finger);
        expected.add(touchBefore(background, game.streamView, event, MoonBridge.LI_TOUCH_EVENT_UP, 0, (short) 68));
        touch(game, background, event);

        assertEquals(expected, sent.packets);
    }

    @Test
    public void aPenIsSentWithThePlaceDistancePressureAndContactAreaItHadBefore() {
        Sent sent = new Sent();
        Game game = game(sent);
        View background = layout(game);
        InputCaptureProvider capturing = new InputCaptureProvider() { };
        capturing.enableCapture();
        ReflectionHelpers.setField(game, "inputCaptureProvider", capturing);
        int source = InputDevice.SOURCE_STYLUS;
        InputDevice digitizer = device(12, InputDevice.SOURCE_TOUCHSCREEN | source, source,
                new float[] {MotionEvent.AXIS_ORIENTATION, (float) -Math.PI, (float) Math.PI},
                new float[] {MotionEvent.AXIS_DISTANCE, 0, 10},
                new float[] {MotionEvent.AXIS_TILT, 0, (float) Math.PI / 2});
        List<String> expected = new ArrayList<>();
        byte penTool = MoonBridge.LI_TOOL_TYPE_PEN;
        // 0.3 radians around and 0.5 radians from upright, in whole degrees.
        short rotation = 17;
        byte tilt = 28;

        // Above the screen a pen reports how far away it is and the size of its tip.
        Pointer pen = new Pointer(0, MotionEvent.TOOL_TYPE_STYLUS, 700, 350).lying(.3f).tool(24, 8).touching(3, 2)
                .axis(MotionEvent.AXIS_DISTANCE, 2.5f).axis(MotionEvent.AXIS_TILT, .5f);
        MotionEvent event = event(MotionEvent.ACTION_HOVER_MOVE, 0, digitizer, source, 0, pen);
        assertEquals(24f, event.getToolMajor(0), 0);
        expected.add(penBefore(background, game.streamView, event, MoonBridge.LI_TOUCH_EVENT_HOVER, penTool, (byte) 0,
                .25f, 0, rotation, tilt));
        try {
            assertTrue(game.onGenericMotion(background, event));
        } finally {
            recycle(event);
        }
        assertArrayEquals(new float[] {.75f, .75f, .25f}, java.util.Arrays.copyOf(sent.values.get(0), 3), .000001f);
        // 24 pixels at 0.3 radians are 22.9 across of 800 and 7.1 down of 400.
        assertEquals(.0337016f, sent.values.get(0)[3], .000001f);

        // On the screen it reports its pressure and what it touches, here with its button held.
        pen.pressure(.8f).touching(6, 4).axis(MotionEvent.AXIS_DISTANCE, 0);
        event = event(MotionEvent.ACTION_DOWN, 0, digitizer, source, MotionEvent.BUTTON_STYLUS_PRIMARY, pen);
        expected.add(penBefore(background, game.streamView, event, MoonBridge.LI_TOUCH_EVENT_DOWN, penTool,
                MoonBridge.LI_PEN_BUTTON_PRIMARY, .8f, 0, rotation, tilt));
        touch(game, background, event);

        pen.at(1200, 20).pressure(.6f);
        event = event(MotionEvent.ACTION_MOVE, 0, digitizer, source, MotionEvent.BUTTON_STYLUS_SECONDARY, pen);
        expected.add(penBefore(background, game.streamView, event, MoonBridge.LI_TOUCH_EVENT_MOVE, penTool,
                MoonBridge.LI_PEN_BUTTON_SECONDARY, .6f, 0, rotation, tilt));
        touch(game, background, event);
        assertArrayEquals(new float[] {1f, 0f, .6f}, java.util.Arrays.copyOf(sent.values.get(2), 3), 0);

        pen.pressure(0);
        event = event(MotionEvent.ACTION_UP, 0, digitizer, source, 0, pen);
        expected.add(penBefore(background, game.streamView, event, MoonBridge.LI_TOUCH_EVENT_UP, penTool, (byte) 0,
                0f, 0, rotation, tilt));
        touch(game, background, event);

        // The other end of the pen.
        Pointer eraser = new Pointer(0, MotionEvent.TOOL_TYPE_ERASER, 400, 200).pressure(.3f).touching(18, 18).lying(-2f)
                .axis(MotionEvent.AXIS_TILT, 0);
        event = event(MotionEvent.ACTION_DOWN, 0, digitizer, source, 0, eraser);
        expected.add(penBefore(background, game.streamView, event, MoonBridge.LI_TOUCH_EVENT_DOWN,
                MoonBridge.LI_TOOL_TYPE_ERASER, (byte) 0, .3f, 0, (short) 246, (byte) 0));
        touch(game, background, event);

        assertEquals(5, expected.size());
        assertEquals(expected, sent.packets);
    }

    // The zoom is undone with one matrix kept for every event, so each event must be mapped from
    // where the desktop is at that event and from nowhere it was before.
    @Test
    public void aZoomedDesktopMapsEveryTouchFromWhereTheDesktopIsNow() {
        Sent sent = new Sent();
        Game game = game(sent);
        View background = layout(game);
        DesktopGestures desktop = new DesktopGestures(game, game.streamView, () -> { }, () -> { });
        desktop.setEnabled(true);
        desktop.viewport().resize(800, 400);
        ReflectionHelpers.setField(game, "desktopGestures", desktop);
        ReflectionHelpers.callInstanceMethod(game.touchInput, "initializeTouchContexts");
        List<float[]> expected = new ArrayList<>();

        Pointer finger = new Pointer(0, MotionEvent.TOOL_TYPE_FINGER, 500, 250).pressure(.5f).touching(30, 12);
        desktop.viewport().zoom(2, 400, 200);
        desktop.viewport().pan(100, 0);
        expected.add(new float[] {desktop.viewport().inputX(400) / 800, desktop.viewport().inputY(200) / 400});
        touch(game, background, event(MotionEvent.ACTION_DOWN, 0, null, InputDevice.SOURCE_TOUCHSCREEN, 0, finger));
        assertArrayEquals(new float[] {.4375f, .5f}, expected.get(0), .00001f);

        // The same finger in the same place after the desktop moved under it.
        desktop.viewport().pan(-160, -90);
        expected.add(new float[] {desktop.viewport().inputX(400) / 800, desktop.viewport().inputY(200) / 400});
        touch(game, background, event(MotionEvent.ACTION_MOVE, 0, null, InputDevice.SOURCE_TOUCHSCREEN, 0, finger));

        desktop.viewport().zoom(1.5f, 250, 120);
        finger.at(330, 410);
        expected.add(new float[] {desktop.viewport().inputX(230) / 800, desktop.viewport().inputY(360) / 400});
        touch(game, background, event(MotionEvent.ACTION_MOVE, 0, null, InputDevice.SOURCE_TOUCHSCREEN, 0, finger));

        // Zoomed all the way out the event is used as it arrived.
        desktop.viewport().reset();
        expected.add(new float[] {230f / 800, 360f / 400});
        touch(game, background, event(MotionEvent.ACTION_MOVE, 0, null, InputDevice.SOURCE_TOUCHSCREEN, 0, finger));

        desktop.viewport().zoom(4, 0, 0);
        expected.add(new float[] {desktop.viewport().inputX(230) / 800, desktop.viewport().inputY(360) / 400});
        touch(game, background, event(MotionEvent.ACTION_UP, 0, null, InputDevice.SOURCE_TOUCHSCREEN, 0, finger));

        assertEquals(expected.size(), sent.values.size());
        for (int i = 0; i < expected.size(); i++) {
            assertArrayEquals("Event " + i, expected.get(i),
                    java.util.Arrays.copyOf(sent.values.get(i), 2), .00001f);
            // The zoom moves a touch and leaves the size of its contact alone.
            assertEquals("Event " + i, .0592927f, sent.values.get(i)[3], .000001f);
        }
        assertNotEquals(expected.get(0)[0], expected.get(1)[0], .01f);
        assertNotEquals(expected.get(1)[1], expected.get(2)[1], .01f);
    }
}
