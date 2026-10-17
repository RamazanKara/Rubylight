package com.limelight;

import android.app.Application;
import android.view.KeyEvent;
import android.view.View;
import android.widget.FrameLayout;

import com.limelight.binding.input.KeyboardTranslator;
import com.limelight.binding.input.capture.InputCaptureProvider;
import com.limelight.nvstream.NvConnection;
import com.limelight.nvstream.StreamConfiguration;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.preferences.PreferenceConfiguration;
import com.limelight.ui.DesktopControlsPanel;
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
        ReflectionHelpers.callInstanceMethod(fixture.game, "toggleKeyboardOverlay");
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
            ReflectionHelpers.callInstanceMethod(game, "toggleKeyboardOverlay");
            panel = ReflectionHelpers.getField(game, "keyboardOverlay");
            sink = ReflectionHelpers.getField(panel, "sink");
        }

        void hide() { ReflectionHelpers.callInstanceMethod(game, "hideKeyboardOverlay"); }
    }

    private static final class RecordingConnection extends NvConnection {
        final List<int[]> motion = new ArrayList<>();
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
