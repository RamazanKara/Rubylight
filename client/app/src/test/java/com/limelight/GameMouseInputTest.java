package com.limelight;

import android.app.Application;
import android.view.InputDevice;
import android.view.KeyEvent;

import com.limelight.binding.input.capture.InputCaptureProvider;
import com.limelight.nvstream.NvConnection;
import com.limelight.nvstream.StreamConfiguration;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.input.MouseButtonPacket;
import com.limelight.preferences.PreferenceConfiguration;

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
public class GameMouseInputTest {
    @Test
    public void syntheticRightClicksStayLocalWhenInputIsReleased() {
        for (int source : new int[] {InputDevice.SOURCE_MOUSE, InputDevice.SOURCE_MOUSE_RELATIVE}) {
            RecordingConnection connection = new RecordingConnection();
            Game game = game(connection);
            ReflectionHelpers.setField(game, "grabbedInput", false);

            click(game, source);

            assertTrue("Uncaptured right-click reached the PC", connection.rightButton.isEmpty());
            assertFalse("Synthetic Back must not exit the stream", game.isFinishing());
        }
    }

    @Test
    public void releasingCaptureReleasesHeldRightClickAndIgnoresLateMouseKeys() {
        for (int source : new int[] {InputDevice.SOURCE_MOUSE, InputDevice.SOURCE_MOUSE_RELATIVE}) {
            RecordingConnection connection = new RecordingConnection();
            Game game = game(connection);
            assertTrue(game.handleKeyDown(mouseBack(source, KeyEvent.ACTION_DOWN)));

            ReflectionHelpers.callInstanceMethod(game, "setInputGrabState",
                    ReflectionHelpers.ClassParameter.from(boolean.class, false));
            assertEquals(Arrays.asList(true, false), connection.rightButton);
            click(game, source);

            assertEquals("Late mouse events must not press the button again",
                    Arrays.asList(true, false), connection.rightButton);
        }
    }

    @Test
    public void capturedRightClicksAreForwardedUnlessMouseNavigationHandlesThem() {
        for (int source : new int[] {InputDevice.SOURCE_MOUSE, InputDevice.SOURCE_MOUSE_RELATIVE}) {
            RecordingConnection connection = new RecordingConnection();
            Game game = game(connection);

            click(game, source);
            assertEquals(Arrays.asList(true, false), connection.rightButton);

            PreferenceConfiguration prefs = ReflectionHelpers.getField(game, "prefConfig");
            prefs.mouseNavButtons = true;
            click(game, source);
            assertEquals("Mouse navigation must not produce duplicate right-clicks",
                    Arrays.asList(true, false), connection.rightButton);
        }
    }

    private static Game game(RecordingConnection connection) {
        Game game = Robolectric.buildActivity(Game.class).get();
        ReflectionHelpers.setField(game, "conn", connection);
        ReflectionHelpers.setField(game, "prefConfig", new PreferenceConfiguration());
        ReflectionHelpers.setField(game, "inputCaptureProvider", new InputCaptureProvider() {});
        ReflectionHelpers.setField(game, "grabbedInput", true);
        ReflectionHelpers.setField(game, "connected", true);
        PreferenceConfiguration prefs = ReflectionHelpers.getField(game, "prefConfig");
        prefs.backOpensMenu = true;
        return game;
    }

    private static void click(Game game, int source) {
        assertTrue(game.handleKeyDown(mouseBack(source, KeyEvent.ACTION_DOWN)));
        assertTrue(game.handleKeyUp(mouseBack(source, KeyEvent.ACTION_UP)));
    }

    private static KeyEvent mouseBack(int source, int action) {
        return new KeyEvent(0, 10, action, KeyEvent.KEYCODE_BACK, 0, 0, -1, 0, 0, source);
    }

    private static class RecordingConnection extends NvConnection {
        final List<Boolean> rightButton = new ArrayList<>();

        RecordingConnection() {
            super(null, new ComputerDetails.AddressTuple("127.0.0.1", 47989), 47984, "test",
                    new StreamConfiguration.Builder().build(), null, null);
        }

        @Override public boolean canSendInput() { return true; }
        @Override public void sendMouseButtonDown(byte button) {
            if (button == MouseButtonPacket.BUTTON_RIGHT) rightButton.add(true);
        }
        @Override public void sendMouseButtonUp(byte button) {
            if (button == MouseButtonPacket.BUTTON_RIGHT) rightButton.add(false);
        }
    }
}
