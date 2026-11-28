package com.limelight;

import android.app.Application;
import android.view.View;
import android.widget.FrameLayout;

import com.limelight.binding.input.ControllerHandler;
import com.limelight.binding.input.pad.PadLayout;
import com.limelight.binding.input.touch.TrackpadPointer;
import com.limelight.nvstream.NvConnection;
import com.limelight.nvstream.StreamConfiguration;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvApp;
import com.limelight.preferences.PreferenceConfiguration;
import com.limelight.ui.DesktopViewport;
import com.limelight.ui.StreamView;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;
import org.robolectric.util.ReflectionHelpers;

import java.util.ArrayList;
import java.util.List;

import static org.junit.Assert.*;

/** The on-screen controls' place in a live stream: a pad's presence, the pointer touchpad's route, and turning them on. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 30, application = Application.class, qualifiers = "w800dp-h360dp-land-xhdpi",
        shadows = PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class GameOscControlsTest {
    /** Records what the controls did, and what reached the host. */
    public static class RecordingGame extends Game {
        int hides, shows;
        @Override void hideOnscreenControllers() { hides++; }
        @Override void showOnscreenController() { shows++; }
    }

    private static class PermissionConnection extends NvConnection {
        final List<int[]> positions = new ArrayList<>();
        final List<int[]> moves = new ArrayList<>();

        PermissionConnection() {
            super(null, new ComputerDetails.AddressTuple("127.0.0.1", 47989), 47984, "test",
                    new StreamConfiguration.Builder().build(), null, null);
        }

        @Override public boolean canSendInput() { return true; }
        @Override public void sendMousePosition(short x, short y, short width, short height) {
            positions.add(new int[] {x, y, width, height});
        }
        @Override public void sendMouseMove(short dx, short dy) {
            moves.add(new int[] {dx, dy});
        }
    }

    private static PreferenceConfiguration oscPrefs() {
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        prefs.onscreenController = true;
        return prefs;
    }

    /**
     * H1: a pad's arrival changes the controls only when the settings say so. Only L3/R3, the on-screen controls as
     * their own player, and Controllers off all keep touch available, and the presence listener is what a pad fires.
     */
    @Test public void aPadArrivingKeepsTheControlsTheSettingsKeepWithIt() {
        RecordingGame game = Robolectric.buildActivity(RecordingGame.class).get();
        PreferenceConfiguration prefs = oscPrefs();
        prefs.onlyL3R3 = true;
        ReflectionHelpers.setField(game, "prefConfig", prefs);
        ReflectionHelpers.setField(game, "conn", new PermissionConnection());
        ControllerHandler.PresenceListener listener = game::setPhysicalControllerConnected;

        listener.onControllerPresenceChanged(true);
        assertEquals("Only L3/R3 uses touch with a pad", 0, game.hides);
        game.shows = 0;
        listener.onControllerPresenceChanged(false);
        assertEquals("Leaving shows them again", 1, game.shows);

        // The on-screen controls as their own player beside a pad use touch too.
        game.hides = game.shows = 0;
        prefs.onlyL3R3 = false;
        prefs.onscreenOwnPlayer = true;
        prefs.multiController = true;
        listener.onControllerPresenceChanged(true);
        assertEquals("Its own player keeps the controls", 0, game.hides);

        // Controllers off: a pad the app does not send cannot replace them.
        game.hides = game.shows = 0;
        prefs.onscreenOwnPlayer = false;
        prefs.multiController = false;
        prefs.sendControllers = false;
        listener.onControllerPresenceChanged(true);
        assertEquals("A pad that sends nothing keeps the controls", 0, game.hides);

        // Any other pad hides them, as the setting says.
        game.hides = game.shows = 0;
        prefs.sendControllers = true;
        listener.onControllerPresenceChanged(true);
        assertEquals("A controlling pad hides them", 1, game.hides);
    }

    /**
     * L4: turning the controls on mid-stream loads the current orientation's layout, not the one the stream started
     * in. The stream is turned while the controls are off, then the stream menu's switch brings them back.
     */
    @Test public void switchingTheControlsOnLoadsThisOrientationsLayout() {
        Game game = Robolectric.buildActivity(Game.class).get();
        game.setTheme(R.style.AppTheme);
        PreferenceConfiguration prefs = oscPrefs();
        ReflectionHelpers.setField(game, "prefConfig", prefs);
        ReflectionHelpers.setField(game, "conn", new PermissionConnection());
        ReflectionHelpers.setField(game, "app", new NvApp("Game", 7, false));
        StreamView stream = new StreamView(game);
        FrameLayout parent = new FrameLayout(game);
        parent.addView(stream);
        parent.measure(View.MeasureSpec.makeMeasureSpec(1600, View.MeasureSpec.EXACTLY),
                View.MeasureSpec.makeMeasureSpec(720, View.MeasureSpec.EXACTLY));
        parent.layout(0, 0, 1600, 720);
        ReflectionHelpers.setField(game, "streamView", stream);
        ReflectionHelpers.setField(game, "performanceOverlayScroll", new View(game));

        game.showOnscreenController();
        PadLayout landscape = layout(game.modernController).copy();
        landscape.setScale(PadLayout.Control.FACE, 1.3f);
        save(game.modernController, landscape);

        // A pad arrived and the stream turned while the controls were away; the switch is turned back on in portrait.
        RuntimeEnvironment.setQualifiers("w360dp-h800dp-port-xhdpi");
        game.touchInput.setOnscreenControlsEnabled(true);
        assertEquals("Portrait has no layout of its own yet", 1f,
                layout(game.modernController).spec(PadLayout.Control.FACE).scale(), 1e-3);
        PadLayout portrait = layout(game.modernController).copy();
        portrait.setScale(PadLayout.Control.FACE, 1.4f);
        save(game.modernController, portrait);

        // Back in landscape, the switch brings landscape's own layout, not portrait's.
        RuntimeEnvironment.setQualifiers("w800dp-h360dp-land-xhdpi");
        game.touchInput.setOnscreenControlsEnabled(true);
        assertEquals(1.3f, layout(game.modernController).spec(PadLayout.Control.FACE).scale(), 1e-3);
        game.modernController.destroy();
    }

    /** The modern controller's own layout, which is package-private where it lives. */
    private static PadLayout layout(com.limelight.binding.input.pad.ModernController controller) {
        return ReflectionHelpers.callInstanceMethod(controller, "layout");
    }

    private static void save(com.limelight.binding.input.pad.ModernController controller, PadLayout draft) {
        ReflectionHelpers.callInstanceMethod(controller, "save", ReflectionHelpers.ClassParameter.from(PadLayout.class,
                draft), ReflectionHelpers.ClassParameter.from(boolean.class, true));
    }

    /**
     * M4: the pointer touchpad moves a Local cursor through TrackpadPointer, as the screen touchpad does, with the
     * desktop view's zoom applied, rather than sending a mouse move on its own.
     */
    @Test public void thePointerTouchpadMovesTheLocalCursorThroughTrackpadPointerAtZoom() {
        PermissionConnection connection = new PermissionConnection();
        Game game = Robolectric.buildActivity(Game.class).get();
        game.setTheme(R.style.AppTheme);
        PreferenceConfiguration prefs = oscPrefs();
        prefs.width = 1920;
        prefs.height = 1080;
        prefs.trackpadSpeed = 100;
        ReflectionHelpers.setField(game, "prefConfig", prefs);
        ReflectionHelpers.setField(game, "conn", connection);
        ReflectionHelpers.setField(game, "app", new NvApp("Game", 7, false));
        StreamView stream = new StreamView(game);
        FrameLayout parent = new FrameLayout(game);
        parent.addView(stream);
        parent.measure(View.MeasureSpec.makeMeasureSpec(1600, View.MeasureSpec.EXACTLY),
                View.MeasureSpec.makeMeasureSpec(720, View.MeasureSpec.EXACTLY));
        parent.layout(0, 0, 1600, 720);
        ReflectionHelpers.setField(game, "streamView", stream);

        TrackpadPointer pointer = new TrackpadPointer(connection, stream, null, false, null, null);
        pointer.setReferenceSize(1920, 1080);
        pointer.setLocalCursor(true);
        DesktopViewport viewport = new DesktopViewport();
        viewport.resize(1600, 720);
        viewport.zoom(2, 800, 360);
        pointer.setViewport(viewport);
        assertEquals(2f, pointer.zoomScale(), 0);
        ReflectionHelpers.setField(game, "trackpadPointer", pointer);

        game.showOnscreenController();
        com.limelight.binding.input.pad.ModernController.PointerSink sink =
                ReflectionHelpers.getField(game.modernController, "pointerSink");
        sink.move(100, 0);
        // 1920 host px over a 1600 px view is 120; at 2x zoom the view shows half of that, so 60.
        assertTrue("The local cursor's position goes to the host", !connection.positions.isEmpty());
        assertArrayEquals(new int[] {1020, 540, 1920, 1080},
                connection.positions.get(connection.positions.size() - 1));
        assertTrue("No mouse move bypasses TrackpadPointer: " + connection.moves, connection.moves.isEmpty());
        game.modernController.destroy();
    }

    @Test public void modernControllerMouseSinkAndKeyboardModifierStateIntegrateEndToEnd() {
        List<int[]> keyEvents = new ArrayList<>();
        List<int[]> mouseEvents = new ArrayList<>();
        List<Integer> scrollEvents = new ArrayList<>();
        PermissionConnection spy = new PermissionConnection() {
            @Override
            public boolean canSendInput() {
                return true;
            }

            @Override
            public void sendKeyboardInput(short keyMap, byte keyDirection, byte modifier, byte flags) {
                keyEvents.add(new int[] {keyMap & 0xFFFF, keyDirection & 0xFF, modifier & 0xFF});
            }

            @Override
            public void sendOverlayMouseButton(byte mouseButton, boolean down) {
                mouseEvents.add(new int[] {down ? 1 : 0, mouseButton & 0xFF});
            }

            @Override
            public void sendMouseButtonDown(byte mouseButton) {
                mouseEvents.add(new int[] {1, mouseButton & 0xFF});
            }

            @Override
            public void sendMouseButtonUp(byte mouseButton) {
                mouseEvents.add(new int[] {0, mouseButton & 0xFF});
            }

            @Override
            public void sendMouseHighResScroll(short amount) {
                scrollEvents.add((int) amount);
            }
        };
        Game game = Robolectric.buildActivity(Game.class).get();
        game.setTheme(R.style.AppTheme);
        ReflectionHelpers.setField(game, "prefConfig", oscPrefs());
        ReflectionHelpers.setField(game, "conn", spy);
        ReflectionHelpers.setField(game, "connected", true);
        ReflectionHelpers.setField(game, "app", new NvApp("Game", 7, false));
        ReflectionHelpers.setField(game, "keyboardTranslator",
                new com.limelight.binding.input.KeyboardTranslator());
        StreamView stream = new StreamView(game);
        FrameLayout parent = new FrameLayout(game);
        parent.addView(stream);
        parent.measure(View.MeasureSpec.makeMeasureSpec(1600, View.MeasureSpec.EXACTLY),
                View.MeasureSpec.makeMeasureSpec(720, View.MeasureSpec.EXACTLY));
        parent.layout(0, 0, 1600, 720);
        ReflectionHelpers.setField(game, "streamView", stream);

        game.showOnscreenController();
        Object heldInputs = ReflectionHelpers.callInstanceMethod(game.modernController, "heldInputs");
        com.limelight.binding.input.HeldInputState.Sink sink =
                ReflectionHelpers.getField(heldInputs, "sink");
        com.limelight.binding.input.pad.ModernController.PointerSink pointerSink =
                ReflectionHelpers.getField(game.modernController, "pointerSink");

        sink.mouse(com.limelight.nvstream.input.MouseButtonPacket.BUTTON_LEFT, true);
        sink.mouse(com.limelight.nvstream.input.MouseButtonPacket.BUTTON_LEFT, false);
        pointerSink.scroll(1, 0);

        assertEquals(2, mouseEvents.size());
        assertArrayEquals(new int[] {1, com.limelight.nvstream.input.MouseButtonPacket.BUTTON_LEFT}, mouseEvents.get(0));
        assertArrayEquals(new int[] {0, com.limelight.nvstream.input.MouseButtonPacket.BUTTON_LEFT}, mouseEvents.get(1));
        assertFalse("Custom scroll wheel reaches NvConnection", scrollEvents.isEmpty());

        sink.key(0xA0, true); // VK_LSHIFT held on screen
        assertEquals(com.limelight.nvstream.input.KeyboardPacket.MODIFIER_SHIFT,
                game.keyboardInput.onScreenModifierState() & 0xFF);
        game.keyboardInput.sendCustomKey(android.view.KeyEvent.KEYCODE_C, true); // Press C while LSHIFT held
        assertFalse(keyEvents.isEmpty());
        int[] cDown = keyEvents.get(keyEvents.size() - 1);
        assertEquals(com.limelight.nvstream.input.KeyboardPacket.MODIFIER_SHIFT,
                cDown[2] & com.limelight.nvstream.input.KeyboardPacket.MODIFIER_SHIFT);
        game.keyboardInput.sendOnScreenKey(0x45, true); // Press E while LSHIFT held
        int[] eDown = keyEvents.get(keyEvents.size() - 1);
        assertEquals(com.limelight.nvstream.input.KeyboardPacket.MODIFIER_SHIFT,
                eDown[2] & com.limelight.nvstream.input.KeyboardPacket.MODIFIER_SHIFT);
        sink.key(0xA0, false);
        assertEquals(0, game.keyboardInput.onScreenModifierState() & 0xFF);
        game.modernController.destroy();
    }
}
