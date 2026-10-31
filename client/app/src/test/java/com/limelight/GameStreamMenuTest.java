package com.limelight;

import android.app.Application;
import android.view.View;

import com.limelight.nvstream.NvConnection;
import com.limelight.nvstream.StreamConfiguration;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.http.TestHosts;
import com.limelight.preferences.PreferenceConfiguration;
import com.limelight.ui.StreamMenu;

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
    private static Game streamingGame(String fixture) {
        Game game = Robolectric.buildActivity(Game.class).get();
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
        MaterialButtonColors.assertErrorColour(menu.row(StreamMenu.Item.QUIT_GAME));
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

    // The chosen item runs once the menu is gone and the stream has focus: give it that focus.
    private static void runMenuAction(Game game) {
        idle();
        WindowFocusActionQueue queue = ReflectionHelpers.getField(game, "windowFocusActions");
        queue.runPending(true, android.os.SystemClock.uptimeMillis());
    }

    private static void idle() {
        org.robolectric.Shadows.shadowOf(android.os.Looper.getMainLooper()).idle();
    }

    /** The destructive row's text uses the theme's error colour. */
    static final class MaterialButtonColors {
        static void assertErrorColour(com.google.android.material.button.MaterialButton button) {
            int error = com.google.android.material.color.MaterialColors.getColor(button, androidx.appcompat.R.attr.colorError);
            assertEquals(error, button.getCurrentTextColor());
        }
    }
}
