package com.limelight;

import android.app.Application;
import android.view.View;

import com.limelight.nvstream.NvConnection;
import com.limelight.nvstream.StreamConfiguration;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.http.TestHosts;
import com.limelight.preferences.PreferenceConfiguration;
import com.limelight.ui.RustStreamMenuModel;
import com.limelight.ui.StreamMenu;
import com.limelight.ui.StreamMenuModel;

import org.junit.After;
import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;
import org.robolectric.util.ReflectionHelpers;

import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class,
        shadows = PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class GameStreamMenuTest {
    @Before
    public void answerFromThePolicySources() {
        StreamMenuModel.use(new RustStreamMenuModel());
    }

    @After
    public void forgetTheModel() {
        StreamMenuModel.use(null);
    }

    private static Game streamingGame(String fixture) {
        return streamingGame(Game.class, fixture);
    }

    private static <T extends Game> T streamingGame(Class<T> type, String fixture) {
        return streamingGame(type, fixture, null);
    }

    /** {@code keys} records each key sent to the PC, {key, direction}; null leaves the connection's own input path. */
    private static <T extends Game> T streamingGame(Class<T> type, String fixture, java.util.List<int[]> keys) {
        T game = Robolectric.buildActivity(type).get();
        game.setTheme(R.style.StreamTheme);
        game.setContentView(R.layout.activity_game);
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        ReflectionHelpers.setField(game, "prefConfig", prefs);
        NvApp app = new NvApp("Game", 73, false);
        ReflectionHelpers.setField(game, "app", app);
        ReflectionHelpers.setField(game, "foreground", true);
        ReflectionHelpers.setField(game, "connected", true);
        ReflectionHelpers.setField(game, "performanceOverlayView", game.findViewById(R.id.performanceOverlay));
        ReflectionHelpers.setField(game, "performanceOverlayScroll", game.findViewById(R.id.performanceOverlayScroll));
        NvConnection connection = new NvConnection(null, new ComputerDetails.AddressTuple("192.0.2.1", 47989), 47984,
                "test", new StreamConfiguration.Builder().setApp(app).build(), null, null) {
            @Override public void stop() {}
            @Override public boolean canSendInput() { return keys != null || super.canSendInput(); }
            @Override public void sendKeyboardInput(short key, byte direction, byte modifiers, byte flags) {
                if (keys != null) keys.add(new int[] {key & 0xffff, direction});
                else super.sendKeyboardInput(key, direction, modifiers, flags);
            }
        };
        ReflectionHelpers.setField(connection, "hostDetails", TestHosts.details(fixture));
        ReflectionHelpers.setField(game, "conn", connection);
        return game;
    }

    private static StreamMenu open(Game game) {
        game.showStreamMenu();
        StreamMenu menu = ReflectionHelpers.getField(game.menuActions, "streamMenuSheet");
        assertNotNull(menu);
        assertSame(menu.sheet(), ReflectionHelpers.getField(game, "streamMenu"));
        assertTrue(menu.sheet().isShowing());
        return menu;
    }

    @Test
    public void backOpensOneSectionedSheetTitledForTheStream() {
        Game game = streamingGame("rubylight-serverinfo.xml");
        game.onBackPressed();
        StreamMenu menu = ReflectionHelpers.getField(game.menuActions, "streamMenuSheet");
        assertNotNull("Back opens the stream menu", menu);
        assertEquals(StreamMenu.Page.MAIN, menu.page());
        assertNotNull(menu.row(StreamMenu.Item.QUIT_GAME));
        assertNotNull(menu.row(StreamMenu.Item.BITRATE));
        menu.sheet().dismiss();
        idle();
        assertNull(ReflectionHelpers.getField(game.menuActions, "streamMenuSheet"));
        assertNull(ReflectionHelpers.getField(game, "streamMenu"));
    }

    /** Hold Guide after a tap: the first tile shows focus at once, so the first D-pad press moves it. */
    @Test
    public void aControllerOpenedMenuShowsFocusOnItsFirstTile() {
        Game game = streamingGame("rubylight-serverinfo.xml");
        game.showStreamMenuFromController();
        StreamMenu menu = ReflectionHelpers.getField(game.menuActions, "streamMenuSheet");
        assertNotNull(menu);
        idle();
        View focused = menu.sheet().getWindow().getDecorView().findFocus();
        assertNotNull("a tile has focus", focused);
        assertSame(menu.grid().getChildAt(0), focused);
        assertFalse(focused.isInTouchMode());
        menu.sheet().dismiss();
        idle();
    }

    /** Records how an unhandled Back opened the menu; the stream's own key handling is left out. */
    public static class BackGame extends Game {
        String opened;
        @Override public boolean handleKeyUp(android.view.KeyEvent event) { return false; }
        @Override public void showStreamMenu() { opened = "touch"; }
        @Override public void showStreamMenuFromController() { opened = "controller"; }
    }

    private static String openedByBack(int source) {
        BackGame game = streamingGame(BackGame.class, "rubylight-serverinfo.xml");
        android.view.KeyEvent up = new android.view.KeyEvent(0, 0, android.view.KeyEvent.ACTION_UP,
                android.view.KeyEvent.KEYCODE_BACK, 0, 0, 3, 0, android.view.KeyEvent.FLAG_TRACKING, source);
        assertTrue(game.onKeyUp(android.view.KeyEvent.KEYCODE_BACK, up));
        return game.opened;
    }

    /**
     * On the Retroid a menu opened by the controller's Back showed no focus, unlike one opened by Guide: a Back from a
     * controller, D-pad or remote opens it as a controller does (first tile focused), the touch Back as a touch does.
     */
    @Test
    public void aControllerOrRemoteBackOpensTheMenuFocusedAndTheTouchBackDoesNot() {
        assertEquals("gamepad Back", "controller",
                openedByBack(android.view.InputDevice.SOURCE_GAMEPAD | android.view.InputDevice.SOURCE_KEYBOARD));
        assertEquals("D-pad Back", "controller", openedByBack(android.view.InputDevice.SOURCE_DPAD));
        assertEquals("navigation bar or gesture Back", "touch", openedByBack(android.view.InputDevice.SOURCE_KEYBOARD));
    }

    @Test
    public void anActionRunsAfterTheMenuClosesAndQuickTogglesStayOpen() {
        Game game = streamingGame("rubylight-serverinfo.xml");
        StreamMenu menu = open(game);
        PreferenceConfiguration prefs = ReflectionHelpers.getField(game, "prefConfig");
        assertFalse(prefs.enablePerfOverlay);
        menu.row(StreamMenu.Item.OVERLAY).performClick();
        assertTrue("The overlay toggles in place", prefs.enablePerfOverlay);
        assertEquals(View.VISIBLE, game.findViewById(R.id.performanceOverlayScroll).getVisibility());
        assertTrue(menu.sheet().isShowing());
        assertTrue(menu.row(StreamMenu.Item.OVERLAY).isChecked());

        menu.row(StreamMenu.Item.CLIPBOARD_RECEIVE).performClick();
        assertFalse(menu.sheet().isShowing());
        idle();
        assertNull(ReflectionHelpers.getField(game.menuActions, "streamMenuSheet"));
    }

    @Test
    public void theOverlayPageOffAndCornerRowsActOnTheStream() {
        Game game = streamingGame("rubylight-serverinfo.xml");
        PreferenceConfiguration prefs = ReflectionHelpers.getField(game, "prefConfig");
        android.content.SharedPreferences stored = android.preference.PreferenceManager.getDefaultSharedPreferences(game);
        prefs.enablePerfOverlay = true;
        game.findViewById(R.id.performanceOverlayScroll).setVisibility(View.VISIBLE);
        StreamMenu menu = open(game);
        menu.row(StreamMenu.Item.OVERLAY_PAGE).performClick();
        menu.row(StreamMenu.Item.OVERLAY_BOTTOM_RIGHT).performClick();
        runMenuAction(game);
        assertEquals("bottom_right", stored.getString("performance_overlay_position", ""));
        assertEquals("bottom_right", prefs.overlayPosition);
        assertTrue("A corner leaves the overlay on", prefs.enablePerfOverlay);

        menu = open(game);
        menu.row(StreamMenu.Item.OVERLAY_PAGE).performClick();
        menu.row(StreamMenu.Item.OVERLAY_OFF).performClick();
        runMenuAction(game);
        assertFalse(prefs.enablePerfOverlay);
        assertFalse(stored.getBoolean("checkbox_enable_perf_overlay", true));
        assertEquals(View.GONE, game.findViewById(R.id.performanceOverlayScroll).getVisibility());

        StreamMenu.Options options = ReflectionHelpers.callInstanceMethod(game.menuActions, "streamMenuOptions");
        assertFalse(options.overlayShown);
        assertEquals("bottom_right", options.overlayPosition);
    }

    @Test
    public void backInTheOverlayPageReturnsToTheStreamMenu() {
        Game game = streamingGame("rubylight-serverinfo.xml");
        StreamMenu menu = open(game);
        menu.row(StreamMenu.Item.OVERLAY_PAGE).performClick();
        assertEquals(StreamMenu.Page.OVERLAY, menu.page());
        menu.sheet().onBackPressed();
        assertEquals(StreamMenu.Page.MAIN, menu.page());
        assertTrue(menu.sheet().isShowing());
        assertSame(menu.sheet(), ReflectionHelpers.getField(game, "streamMenu"));
        menu.sheet().onBackPressed();
        assertFalse(menu.sheet().isShowing());
        idle();
        assertNull(ReflectionHelpers.getField(game, "streamMenu"));
    }

    @Test
    public void aUsbDriverPadStepsBackAndChoosesInTheMenu() {
        Game game = streamingGame("rubylight-serverinfo.xml");
        StreamMenu menu = open(game);
        menu.row(StreamMenu.Item.OVERLAY_PAGE).performClick();
        game.steerStreamMenu(android.view.KeyEvent.KEYCODE_BUTTON_B);
        assertEquals("B returns from the page", StreamMenu.Page.MAIN, menu.page());
        assertTrue(menu.sheet().isShowing());
        View disconnect = menu.row(StreamMenu.Item.DISCONNECT);
        disconnect.setFocusableInTouchMode(true);
        assertTrue(disconnect.requestFocus());
        game.steerStreamMenu(android.view.KeyEvent.KEYCODE_DPAD_CENTER);
        assertFalse("A chooses the focused row", menu.sheet().isShowing());
        idle();
        assertNull(ReflectionHelpers.getField(game, "streamMenu"));
        game.steerStreamMenu(android.view.KeyEvent.KEYCODE_BUTTON_B);
        assertFalse(game.isFinishing());
    }

    @Test
    public void quitGameAsksFirstInTheErrorColour() {
        Game game = streamingGame("rubylight-serverinfo.xml");
        StreamMenu menu = open(game);
        MaterialButtonColors.assertRedCapsule(menu.row(StreamMenu.Item.QUIT_GAME));
        ReflectionHelpers.callInstanceMethod(game.menuActions, "confirmQuitApp");
        androidx.appcompat.app.AlertDialog dialog = ReflectionHelpers.getField(game, "streamMenu");
        try {
            assertTrue(dialog.isShowing());
            assertEquals(game.getString(R.string.stream_quit_game),
                    dialog.getButton(androidx.appcompat.app.AlertDialog.BUTTON_POSITIVE).getText().toString());
            dialog.getButton(androidx.appcompat.app.AlertDialog.BUTTON_NEGATIVE).performClick();
            assertNull("Cancelling quits nothing", ReflectionHelpers.getField(game, "streamMenuAction"));
        } finally {
            dialog.dismiss();
        }
    }

    @Test
    public void gameStreamGetsNoHostSectionAndNoQualitySection() {
        Game game = streamingGame("gamestream-serverinfo.xml");
        StreamMenu menu = open(game);
        for (StreamMenu.Entry entry : menu.entries()) {
            assertNotEquals(entry.toString(), StreamMenu.Section.HOST, entry.section);
            assertNotEquals(entry.toString(), StreamMenu.Section.QUALITY, entry.section);
        }
        menu.sheet().dismiss();
    }

    /**
     * A shortcut tile ran nothing when input had been released before the menu opened: the shortcut waits for the menu
     * to close, captures input and then reaches the PC.
     */
    @Test
    public void aShortcutTileReachesThePcAfterTheMenuClosesEvenWithInputReleasedBefore() {
        java.util.List<int[]> keys = new java.util.ArrayList<>();
        Game game = streamingGame(Game.class, "rubylight-serverinfo.xml", keys);
        ReflectionHelpers.setField(game, "keyboardTranslator", new com.limelight.binding.input.KeyboardTranslator());
        ReflectionHelpers.setField(game, "inputCaptureProvider", new com.limelight.binding.input.capture.InputCaptureProvider() { });
        ReflectionHelpers.setField(game, "grabbedInput", false);
        com.limelight.binding.input.KeyboardShortcuts.save(game, java.util.Collections.singletonList(new com.limelight.binding.input.KeyboardShortcuts.Shortcut(
                "a", "Switch", com.limelight.binding.input.KeyboardShortcuts.parseChord("ALT+TAB"))));
        StreamMenu menu = open(game);
        // Status cells of the model are rows, so a cell's index is not its tile's: the tile goes by its name.
        View tile = null;
        for (int i = 0; i < menu.grid().getChildCount(); i++) {
            View child = menu.grid().getChildAt(i);
            if (((android.widget.TextView) child).getText().toString().contains("Switch")) tile = child;
        }
        assertNotNull("the saved shortcut is a tile", tile);
        tile.performClick();
        assertFalse(menu.sheet().isShowing());
        assertTrue("nothing goes out while the menu is up", keys.isEmpty());
        runMenuAction(game);
        assertEquals("pressed, and held for a moment", 2, keys.size());
        org.robolectric.Shadows.shadowOf(android.os.Looper.getMainLooper()).idleFor(java.time.Duration.ofMillis(GameKeyboardInput.SHORTCUT_HOLD_MS));
        assertEquals(4, keys.size());
        assertTrue(ReflectionHelpers.<Boolean>getField(game, "grabbedInput"));
    }

    /**
     * The header named the size asked for in Settings: a host without 4K streams 1080p, native and host-chosen sizes
     * differ from the setting, and a reconfigure changes it mid-stream. It names the size the decoder gets.
     */
    @Test
    public void theHeaderNamesTheNegotiatedSizeNotTheOneAskedFor() {
        Game game = streamingGame("rubylight-serverinfo.xml");
        PreferenceConfiguration prefs = ReflectionHelpers.getField(game, "prefConfig");
        prefs.width = 3840;
        prefs.height = 2160;
        StreamMenu.Options unknown = ReflectionHelpers.callInstanceMethod(game.menuActions, "streamMenuOptions");
        assertEquals("unknown until the decoder is set up", 0, unknown.streamWidth);
        // The decoder set up for what the host negotiated: 1080p.
        ReflectionHelpers.setField(game, "streamWidth", 1920);
        ReflectionHelpers.setField(game, "streamHeight", 1080);
        StreamMenu.Options options = ReflectionHelpers.callInstanceMethod(game.menuActions, "streamMenuOptions");
        assertEquals(1920, options.streamWidth);
        assertEquals(1080, options.streamHeight);
        // Every size the decoder reports is kept, also one the window already follows.
        prefs.width = 2560;
        prefs.height = 1440;
        game.videoOutput.applyStreamSize(2560, 1440);
        options = ReflectionHelpers.callInstanceMethod(game.menuActions, "streamMenuOptions");
        assertEquals(2560, options.streamWidth);
        assertEquals(1440, options.streamHeight);
    }

    // The chosen item runs once the menu is gone and the stream has focus: give it that focus.
    private static void runMenuAction(Game game) {
        idle();
        WindowFocusActionQueue queue = ReflectionHelpers.getField(game, "windowFocusActions");
        queue.runPending(true, android.os.SystemClock.uptimeMillis());
    }

    private static void idle() {
        org.robolectric.Shadows.shadowOf(android.os.Looper.getMainLooper()).idle();
    }

    /** The one way out that ends the game is the panel's red capsule, in the ink that reads on its fill. */
    static final class MaterialButtonColors {
        static void assertRedCapsule(com.google.android.material.button.MaterialButton button) {
            int ink = androidx.core.content.ContextCompat.getColor(button.getContext(), R.color.nj_color_on_danger_fill);
            assertEquals(ink, button.getCurrentTextColor());
        }
    }
}
