package com.limelight;

import android.app.Application;
import android.graphics.Rect;
import android.view.InputDevice;
import android.view.KeyEvent;
import android.view.MotionEvent;
import android.view.View;
import android.view.ViewGroup;
import android.widget.FrameLayout;

import com.limelight.binding.input.KeyboardTranslator;
import com.limelight.binding.input.capture.InputCaptureProvider;
import com.limelight.binding.input.touch.TrackpadPointer;
import com.limelight.nvstream.NvConnection;
import com.limelight.nvstream.StreamConfiguration;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.preferences.PreferenceConfiguration;
import com.limelight.ui.DesktopControlsPanel;
import com.limelight.ui.DesktopViewport;
import com.limelight.ui.KeyboardOverlay;
import com.limelight.ui.StreamView;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;
import org.robolectric.util.ReflectionHelpers;

import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;

import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class,
        shadows = PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class GameDesktopControlsTest {
    @Test
    public void visiblePanelRoutesPointerWheelButtonsAndShortcuts() {
        Fixture fixture = new Fixture();

        fixture.sink.move(4, -5);
        fixture.sink.scroll(1, 2);
        fixture.sink.mouseButton(1, true);
        fixture.sink.mouseButton(1, false);
        fixture.sink.sendKeys(new int[] {KeyEvent.KEYCODE_ENTER});

        assertArrayEquals(new int[] {4, -5}, fixture.connection.motion.get(0));
        assertEquals(Arrays.asList(1), fixture.connection.vertical);
        assertEquals(Arrays.asList(2), fixture.connection.horizontal);
        assertEquals(Arrays.asList("1:down", "1:up"), fixture.connection.buttons);
        assertEquals(2, fixture.connection.keys.size());
    }

    @Test
    public void panelMovesTheLocalCursorAndTheNextStreamTouchKeepsItsPosition() {
        Fixture fixture = new Fixture();
        TrackpadPointer pointer = fixture.pointer(true, false);
        fixture.sink.move(20, -10);
        pointer.touchDown();
        assertEquals(2, fixture.connection.positions.size());
        for (int[] position : fixture.connection.positions) {
            assertArrayEquals(new int[] {1040, 480, 2000, 1000}, position);
        }
        assertTrue(fixture.connection.motion.isEmpty());
    }

    @Test
    public void panelUsesDesktopMouseRoutingAndStreamPixelScaling() {
        for (boolean absolute : new boolean[] {false, true}) {
            Fixture fixture = new Fixture();
            fixture.pointer(false, absolute);
            fixture.sink.move(20, -10);
            if (absolute) {
                assertArrayEquals(new int[] {40, -20, 2000, 1000}, fixture.connection.absoluteMotion.get(0));
                assertTrue(fixture.connection.motion.isEmpty());
            } else {
                assertArrayEquals(new int[] {40, -20}, fixture.connection.motion.get(0));
                assertTrue(fixture.connection.absoluteMotion.isEmpty());
            }
        }
    }

    @Test
    public void zoomedLocalCursorKeepsFractionalMovementWithoutChangingWheelUnits() {
        Fixture fixture = new Fixture();
        TrackpadPointer pointer = fixture.pointer(true, false);
        DesktopViewport viewport = new DesktopViewport();
        viewport.resize(1000, 500);
        viewport.zoom(2, 500, 250);
        pointer.setViewport(viewport);
        for (int i = 0; i < 5; i++) fixture.sink.move(.2f, -.2f);
        fixture.sink.scroll(120, -120);
        assertArrayEquals(new int[] {1001, 499, 2000, 1000}, fixture.connection.positions.get(0));
        assertEquals(Arrays.asList(120), fixture.connection.vertical);
        assertEquals(Arrays.asList(-120), fixture.connection.horizontal);
    }

    @Test
    @Config(sdk = {29, 34})
    public void directPanelPressWorksBeforeWindowFocusReturnsAndReleasesExactlyOnce() {
        Fixture fixture = new Fixture();
        fixture.attach();
        fixture.game.onWindowFocusChanged(false);
        fixture.touchButton(MotionEvent.ACTION_DOWN);
        assertEquals(Arrays.asList("1:down"), fixture.connection.buttons);
        // Dispatch ended, so unrelated callbacks still cannot use the unfocused window.
        fixture.sink.mouseButton(3, true);
        fixture.sink.move(10, 10);
        fixture.sink.sendKeys(new int[] {KeyEvent.KEYCODE_ENTER});
        assertEquals(Arrays.asList("1:down"), fixture.connection.buttons);
        assertTrue(fixture.connection.motion.isEmpty());
        assertTrue(fixture.connection.keys.isEmpty());
        fixture.game.onWindowFocusChanged(true);
        fixture.touchButton(MotionEvent.ACTION_UP);
        assertEquals(Arrays.asList("1:down", "1:up"), fixture.connection.buttons);
        fixture.hide();
        assertEquals(2, fixture.connection.buttons.size());
    }

    @Test
    public void directTouchDoesNotBypassSessionGatesAndCancelClearsAnUnfocusedPress() {
        for (String gate : new String[] {"foreground", "connected", "grabbedInput", "permission", "finishing"}) {
            Fixture fixture = new Fixture();
            fixture.attach();
            fixture.game.onWindowFocusChanged(false);
            if (gate.equals("permission")) fixture.connection.inputAllowed = false;
            else if (gate.equals("finishing")) fixture.game.finish();
            else ReflectionHelpers.setField(fixture.game, gate, false);
            fixture.touchButton(MotionEvent.ACTION_DOWN);
            fixture.touchButton(MotionEvent.ACTION_UP);
            assertTrue(gate, fixture.connection.buttons.isEmpty());
        }
        Fixture fixture = new Fixture();
        fixture.attach();
        fixture.game.onWindowFocusChanged(false);
        fixture.touchButton(MotionEvent.ACTION_DOWN);
        fixture.touchButton(MotionEvent.ACTION_CANCEL);
        fixture.game.onWindowFocusChanged(true);
        assertEquals(Arrays.asList("1:down", "1:up"), fixture.connection.buttons);
    }

    @Test
    public void lateCallbacksCannotSendInputFromAnUnavailablePanelOrSession() {
        for (String gate : new String[] {"foreground", "connected", "grabbedInput",
                "desktopWindowFocused", "hidden", "permission", "finishing"}) {
            Fixture fixture = new Fixture();
            if (gate.equals("hidden")) fixture.panel.setVisibility(View.GONE);
            else if (gate.equals("permission")) fixture.connection.inputAllowed = false;
            else if (gate.equals("finishing")) fixture.game.finish();
            else ReflectionHelpers.setField(fixture.game, gate, false);

            fixture.sink.move(20, -30);
            fixture.sink.scroll(120, -120);
            fixture.sink.mouseButton(1, true);
            fixture.sink.mouseButton(1, false);
            fixture.sink.sendKeys(new int[] {KeyEvent.KEYCODE_ENTER});

            assertTrue(gate, fixture.connection.motion.isEmpty());
            assertTrue(gate, fixture.connection.vertical.isEmpty());
            assertTrue(gate, fixture.connection.horizontal.isEmpty());
            assertTrue(gate, fixture.connection.buttons.isEmpty());
            assertTrue(gate, fixture.connection.keys.isEmpty());
        }
    }

    @Test
    public void smallPointerAndHighResolutionWheelDeltasAccumulateWithoutBeingLost() {
        Fixture fixture = new Fixture();

        for (int i = 0; i < 5; i++) {
            fixture.sink.move(.2f, -.2f);
            fixture.sink.scroll(.2f, -.4f);
        }

        assertEquals(1, sumAxis(fixture.connection.motion, 0));
        assertEquals(-1, sumAxis(fixture.connection.motion, 1));
        assertEquals(1, sum(fixture.connection.vertical));
        assertEquals(-2, sum(fixture.connection.horizontal));
    }

    @Test
    public void nonfiniteDeltasAndInvalidButtonsDoNotPoisonLaterInput() {
        Fixture fixture = new Fixture();
        for (float invalid : new float[] {Float.NaN, Float.POSITIVE_INFINITY, Float.NEGATIVE_INFINITY}) {
            fixture.sink.move(invalid, 40);
            fixture.sink.move(40, invalid);
            fixture.sink.scroll(invalid, 120);
            fixture.sink.scroll(120, invalid);
        }
        fixture.sink.mouseButton(0, true);
        fixture.sink.mouseButton(6, true);
        fixture.sink.move(2, -3);
        fixture.sink.scroll(7, -9);

        assertEquals(1, fixture.connection.motion.size());
        assertArrayEquals(new int[] {2, -3}, fixture.connection.motion.get(0));
        assertEquals(Arrays.asList(7), fixture.connection.vertical);
        assertEquals(Arrays.asList(-9), fixture.connection.horizontal);
        assertTrue(fixture.connection.buttons.isEmpty());
    }

    @Test
    public void saturatedDeltasKeepTheirDirectionEvenWithFractionalCarry() {
        Fixture fixture = new Fixture();
        fixture.sink.move(.4999f, -.4999f);
        fixture.sink.scroll(.4999f, .4999f);
        assertTrue(fixture.connection.motion.isEmpty());

        fixture.sink.move(Float.MAX_VALUE, -Float.MAX_VALUE);
        fixture.sink.scroll(Float.MAX_VALUE, Float.MAX_VALUE);

        assertArrayEquals(new int[] {Short.MAX_VALUE, Short.MIN_VALUE}, fixture.connection.motion.get(0));
        assertEquals(Arrays.asList((int) Short.MAX_VALUE), fixture.connection.vertical);
        assertEquals(Arrays.asList((int) Short.MAX_VALUE), fixture.connection.horizontal);
    }

    @Test
    public void hidingReleasesEachHeldButtonOnceEvenAfterInputPermissionIsLost() {
        Fixture fixture = new Fixture();
        fixture.sink.mouseButton(1, true);
        fixture.sink.mouseButton(1, true);
        fixture.sink.mouseButton(3, true);
        fixture.connection.inputAllowed = false;

        fixture.hide();
        fixture.hide();
        fixture.sink.mouseButton(1, false);
        fixture.sink.mouseButton(3, false);

        assertEquals(Arrays.asList("1:down", "3:down", "1:up", "3:up"), fixture.connection.buttons);
        assertEquals(View.GONE, fixture.panel.getVisibility());
        assertEquals(0, (int) ReflectionHelpers.getField(fixture.game, "desktopHeldButtons"));
    }

    @Test
    public void focusLossReleasesDesktopDragAndBlocksLateCallbacksUntilFocusReturns() {
        Fixture fixture = new Fixture();
        fixture.sink.mouseButton(1, true);

        fixture.game.onWindowFocusChanged(false);
        fixture.sink.mouseButton(1, true);
        fixture.sink.move(5, 6);
        fixture.sink.scroll(120, 120);
        fixture.game.onWindowFocusChanged(false);

        assertEquals(Arrays.asList("1:down", "1:up"), fixture.connection.buttons);
        assertTrue(fixture.connection.motion.isEmpty());
        assertTrue(fixture.connection.vertical.isEmpty());
        assertTrue(fixture.connection.horizontal.isEmpty());

        fixture.game.onWindowFocusChanged(true);
        fixture.sink.mouseButton(1, true);
        assertEquals(Arrays.asList("1:down", "1:up", "1:down"), fixture.connection.buttons);
        fixture.hide();
    }

    @Test
    public void pausingReleasesDesktopDragBeforeBlockingFurtherInput() {
        Fixture fixture = new Fixture();
        fixture.sink.mouseButton(3, true);

        fixture.game.onPause();
        fixture.sink.mouseButton(3, true);
        fixture.sink.move(5, 6);

        assertEquals(Arrays.asList("3:down", "3:up"), fixture.connection.buttons);
        assertTrue(fixture.connection.motion.isEmpty());
        assertFalse((boolean) ReflectionHelpers.getField(fixture.game, "foreground"));
    }

    @Test
    public void ungrabbingInputAlsoReleasesDesktopDrag() {
        Fixture fixture = new Fixture();
        fixture.sink.mouseButton(1, true);

        ReflectionHelpers.callInstanceMethod(fixture.game, "setInputGrabState",
                ReflectionHelpers.ClassParameter.from(boolean.class, false));
        fixture.sink.mouseButton(1, true);

        assertEquals(Arrays.asList("1:down", "1:up"), fixture.connection.buttons);
        assertFalse((boolean) ReflectionHelpers.getField(fixture.game, "grabbedInput"));
    }

    @Test
    public void reopeningStartsWithoutFractionalMovementFromThePreviousGesture() {
        Fixture fixture = new Fixture();
        fixture.sink.move(.49f, .49f);
        fixture.sink.scroll(.49f, .49f);

        fixture.hide();
        fixture.sink.move(10, 10);
        fixture.sink.scroll(10, 10);
        ReflectionHelpers.callInstanceMethod(fixture.game.keyboardInput, "toggleKeyboardOverlay");
        fixture.sink.move(.02f, .02f);
        fixture.sink.scroll(.02f, .02f);

        assertTrue(fixture.connection.motion.isEmpty());
        assertTrue(fixture.connection.vertical.isEmpty());
        assertTrue(fixture.connection.horizontal.isEmpty());
    }

    private static int sumAxis(List<int[]> values, int axis) {
        int total = 0;
        for (int[] value : values) total += value[axis];
        return total;
    }

    private static int sum(List<Integer> values) {
        int total = 0;
        for (int value : values) total += value;
        return total;
    }

    private static final class Fixture {
        final RecordingConnection connection = new RecordingConnection();
        final Game game = Robolectric.buildActivity(Game.class).get();
        final KeyboardOverlay panel;
        final DesktopControlsPanel.Sink sink;

        Fixture() {
            game.setTheme(R.style.AppTheme);
            ReflectionHelpers.setField(game, "conn", connection);
            ReflectionHelpers.setField(game, "prefConfig", new PreferenceConfiguration());
            ReflectionHelpers.setField(game, "keyboardTranslator", new KeyboardTranslator());
            ReflectionHelpers.setField(game, "inputCaptureProvider", new InputCaptureProvider() {});
            ReflectionHelpers.setField(game, "grabbedInput", true);
            ReflectionHelpers.setField(game, "foreground", true);
            ReflectionHelpers.setField(game, "connected", true);
            StreamView stream = new StreamView(game);
            FrameLayout parent = new FrameLayout(game);
            parent.addView(stream);
            ReflectionHelpers.setField(game, "streamView", stream);
            ReflectionHelpers.callInstanceMethod(game.keyboardInput, "toggleKeyboardOverlay");
            panel = ReflectionHelpers.getField(game, "keyboardOverlay");
            sink = ReflectionHelpers.getField(panel, "sink");
        }

        void hide() { ReflectionHelpers.callInstanceMethod(game.keyboardInput, "hideKeyboardOverlay"); }

        TrackpadPointer pointer(boolean local, boolean absolute) {
            StreamView stream = ReflectionHelpers.getField(game, "streamView");
            stream.layout(0, 0, 1000, 500);
            PreferenceConfiguration prefs = ReflectionHelpers.getField(game, "prefConfig");
            prefs.width = 2000;
            prefs.height = 1000;
            TrackpadPointer pointer = new TrackpadPointer(connection, stream, null, absolute, null, null);
            pointer.setReferenceSize(prefs.width, prefs.height);
            pointer.setLocalCursor(local);
            ReflectionHelpers.setField(game, "trackpadPointer", pointer);
            return pointer;
        }

        void attach() {
            panel.setMode(DesktopControlsPanel.Mode.TRACKPAD);
            game.setContentView((View) panel.getParent());
            View decor = game.getWindow().getDecorView();
            decor.measure(View.MeasureSpec.makeMeasureSpec(1200, View.MeasureSpec.EXACTLY),
                    View.MeasureSpec.makeMeasureSpec(800, View.MeasureSpec.EXACTLY));
            decor.layout(0, 0, 1200, 800);
        }

        void touchButton(int action) {
            View button = described(panel, game.getString(R.string.desktop_controls_left_button));
            assertNotNull(button);
            Rect bounds = new Rect(0, 0, button.getWidth(), button.getHeight());
            ((ViewGroup) game.getWindow().getDecorView()).offsetDescendantRectToMyCoords(button, bounds);
            MotionEvent event = MotionEvent.obtain(0, 10, action, bounds.centerX(), bounds.centerY(), 0);
            event.setSource(InputDevice.SOURCE_TOUCHSCREEN);
            try { game.dispatchTouchEvent(event); } finally { event.recycle(); }
        }
    }

    private static View described(ViewGroup group, String description) {
        for (int i = 0; i < group.getChildCount(); i++) {
            View child = group.getChildAt(i);
            if (description.contentEquals(child.getContentDescription() == null ? "" : child.getContentDescription())) return child;
            if (child instanceof ViewGroup) {
                View found = described((ViewGroup) child, description);
                if (found != null) return found;
            }
        }
        return null;
    }

    private static final class RecordingConnection extends NvConnection {
        final List<int[]> motion = new ArrayList<>();
        final List<int[]> positions = new ArrayList<>(), absoluteMotion = new ArrayList<>();
        final List<Integer> vertical = new ArrayList<>(), horizontal = new ArrayList<>();
        final List<String> buttons = new ArrayList<>();
        final List<int[]> keys = new ArrayList<>();
        boolean inputAllowed = true;

        RecordingConnection() {
            super(null, new ComputerDetails.AddressTuple("127.0.0.1", 47989), 47984, "test",
                    new StreamConfiguration.Builder().build(), null, null);
        }

        @Override public boolean canSendInput() { return inputAllowed; }
        @Override public void sendMouseMove(short x, short y) { motion.add(new int[] {x, y}); }
        @Override public void sendMousePosition(short x, short y, short width, short height) {
            positions.add(new int[] {x, y, width, height});
        }
        @Override public void sendMouseMoveAsMousePosition(short x, short y, short width, short height) {
            absoluteMotion.add(new int[] {x, y, width, height});
        }
        @Override public void sendMouseHighResScroll(short amount) { vertical.add((int) amount); }
        @Override public void sendMouseHighResHScroll(short amount) { horizontal.add((int) amount); }
        @Override public void sendDesktopMouseButton(byte button, boolean down) {
            buttons.add(button + (down ? ":down" : ":up"));
        }
        @Override public void sendKeyboardInput(short key, byte direction, byte modifiers, byte flags) {
            keys.add(new int[] {key & 0xffff, direction, modifiers});
        }
        // Lifecycle cleanup also releases ordinary inputs. These are deliberately separate from
        // the panel's ownership and must not produce native calls in this stream-free fixture.
        @Override public void sendMouseButtonUp(byte button) { }
    }
}
