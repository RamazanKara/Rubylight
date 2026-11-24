package com.limelight.ui;

import android.app.Activity;
import android.app.Application;
import android.view.KeyEvent;

import com.limelight.R;
import com.limelight.binding.input.KeyboardShortcuts;
import com.limelight.nativeapi.NativeApiFixtures;
import com.limelight.nvstream.http.NvApp;

import org.junit.After;
import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;

import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;
import java.util.Set;

import static com.limelight.ui.StreamMenu.Item.*;
import static com.limelight.ui.StreamMenuTestSupport.*;
import static org.junit.Assert.*;

/**
 * Shared-model grid layout, controller hotkey binding/migration, footer placement, fallback mode,
 * shortcut editor, and Rust JNI export tests for {@link StreamMenu}.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class StreamMenuModelAndShortcutsTest {
    @Before
    public void answerFromThePolicySources() {
        StreamMenuModel.use(new RustStreamMenuModel());
    }

    @After
    public void forgetTheModel() {
        StreamMenuModel.use(null);
    }

    /** A remote has no face buttons: the ways out and the status rows show and say their caps only with a controller there. */
    @Test
    public void theWaysOutAndStatusRowsShowTheirCapsOnlyWithAController() {
        try (var controller = Robolectric.buildActivity(Activity.class).setup()) {
            Activity activity = controller.get();
            activity.setTheme(R.style.StreamTheme);
            android.preference.PreferenceManager.getDefaultSharedPreferences(activity).edit()
                    .putString("list_stream_menu_button_y", "overlay_page")
                    .putString("list_stream_menu_button_x", "disconnect")
                    .commit();
            StreamMenu.Options state = options("rubylight-serverinfo.xml", NvApp.Role.STREAM);
            for (boolean pad : new boolean[] {true, false}) {
                StreamMenu.gamepadForTest = pad;
                List<StreamMenu.Item> chosen = new ArrayList<>();
                StreamMenu menu = new StreamMenu(activity, () -> state, chosen::add, null, new SavedShortcuts());
                menu.show();
                MenuRow row = (MenuRow) menu.row(OVERLAY_PAGE);
                TileGrid.Tile out = (TileGrid.Tile) menu.row(DISCONNECT);
                assertEquals("Y", row.hotkey());
                assertEquals("X", out.hotkey());
                assertEquals("pad " + pad, pad, row.capShown());
                assertEquals("pad " + pad, pad, out.capShown());
                assertEquals("pad " + pad, pad, row.getContentDescription().toString().endsWith(", Y"));
                assertEquals("pad " + pad, pad, String.valueOf(out.getContentDescription()).endsWith(", X"));
                // The button works either way: Disconnect asks first.
                press(menu, KeyEvent.KEYCODE_BUTTON_X);
                assertEquals(Arrays.asList(CONFIRM_DISCONNECT), chosen);
                menu.sheet().dismiss();
            }
        } finally {
            StreamMenu.gamepadForTest = null;
        }
    }

    /** A button saved as close_game before Quit game became one choice shows Quit game selected, and the menu reads it as Quit game. */
    @Test
    public void aSavedCloseGameButtonReadsAsQuitGame() {
        try (var controller = Robolectric.buildActivity(Activity.class).setup()) {
            Activity activity = controller.get();
            android.content.SharedPreferences prefs = android.preference.PreferenceManager.getDefaultSharedPreferences(activity);
            prefs.edit().putString("list_stream_menu_button_lb", "close_game")
                    .putString("list_stream_menu_button_y", "overlay").commit();
            assertEquals("quit_game", StreamMenu.buttonChoices(activity).get("lb"));
            StreamMenu.upgradeButtonChoices(prefs);
            assertEquals("quit_game", prefs.getString("list_stream_menu_button_lb", null));
            assertEquals("overlay", prefs.getString("list_stream_menu_button_y", null));
            assertFalse(prefs.contains("list_stream_menu_button_x"));
            // Quit game is a choice the list has, so it shows selected.
            List<String> values = Arrays.asList(activity.getResources().getStringArray(R.array.stream_menu_button_values));
            assertTrue(values.contains("quit_game"));
            assertFalse(values.contains("close_game"));
        }
    }

    /** End session moved under More, where no button runs it: a button saved on it is back on Default, which the list shows. */
    @Test
    public void aSavedEndSessionButtonIsBackOnDefault() {
        try (var controller = Robolectric.buildActivity(Activity.class).setup()) {
            Activity activity = controller.get();
            android.content.SharedPreferences prefs = android.preference.PreferenceManager.getDefaultSharedPreferences(activity);
            prefs.edit().putString("list_stream_menu_button_x", "end_host_session").commit();
            assertFalse(StreamMenu.buttonChoices(activity).containsKey("x"));
            StreamMenu.upgradeButtonChoices(prefs);
            assertEquals("default", prefs.getString("list_stream_menu_button_x", null));
            List<String> values = Arrays.asList(activity.getResources().getStringArray(R.array.stream_menu_button_values));
            assertTrue(values.contains("default"));
            assertFalse(values.contains("end_host_session"));
        }
    }

    @Test
    public void theButtonsRunWhatTheUserGaveThemIncludingAWayOutOrNothing() {
        try (var controller = Robolectric.buildActivity(Activity.class).setup()) {
            Activity activity = controller.get();
            activity.setTheme(R.style.StreamTheme);
            android.preference.PreferenceManager.getDefaultSharedPreferences(activity).edit()
                    .putString("list_stream_menu_button_y", "overlay")
                    .putString("list_stream_menu_button_x", "disconnect")
                    .putString("list_stream_menu_button_lb", "none")
                    .putString("list_stream_menu_button_rb", "default")
                    .apply();
            StreamMenu.Options state = options("rubylight-serverinfo.xml", NvApp.Role.STREAM);
            state.appName = "Open World";
            state.controllerMouseAvailable = true;
            List<StreamMenu.Item> chosen = new ArrayList<>();
            StreamMenu menu = new StreamMenu(activity, () -> state, chosen::add, null, new SavedShortcuts());
            menu.show();

            // Y shows on the tile it was given; Keyboard loses it.
            assertEquals("Y", ((TileGrid.Tile) menu.row(OVERLAY)).hotkey());
            assertNull(((TileGrid.Tile) menu.row(KEYBOARD)).hotkey());
            press(menu, KeyEvent.KEYCODE_BUTTON_Y);
            assertEquals(Arrays.asList(OVERLAY), chosen);
            chosen.clear();
            // LB is off: Shortcuts stays reachable by its tile but the button does nothing.
            assertNull(((TileGrid.Tile) menu.row(SHORTCUTS)).hotkey());
            press(menu, KeyEvent.KEYCODE_BUTTON_L1);
            assertTrue(chosen.isEmpty());
            // RB on Default keeps its own tile, so Overlay carries the first button that names it and still runs.
            press(menu, KeyEvent.KEYCODE_BUTTON_R1);
            assertEquals(Arrays.asList(OVERLAY), chosen);
            chosen.clear();
            // X is a way out under the grid: it shows the button, and from the button Disconnect asks first.
            assertEquals("X", ((TileGrid.Tile) menu.row(DISCONNECT)).hotkey());
            press(menu, KeyEvent.KEYCODE_BUTTON_X);
            assertEquals(Arrays.asList(StreamMenu.Item.CONFIRM_DISCONNECT), chosen);
            chosen.clear();
            // Its own row still disconnects at once, as before.
            menu.row(DISCONNECT).performClick();
            assertEquals(Arrays.asList(DISCONNECT), chosen);
        }
    }

    @Test
    public void theGridIsTheSharedModelsWithItsButtonsTheUsersShortcutsAndAddThenReturnAndTheWaysOut() {
        try (var controller = Robolectric.buildActivity(Activity.class).setup()) {
            Activity activity = controller.get();
            activity.setTheme(R.style.StreamTheme);
            StreamMenu.Options state = options("rubylight-serverinfo.xml", NvApp.Role.STREAM);
            state.appName = "Open World";
            state.controllerMouseAvailable = true;
            SavedShortcuts saved = new SavedShortcuts().add("Snip", "WIN+SHIFT+S").add("Steam overlay", "SHIFT+TAB");
            List<StreamMenu.Item> chosen = new ArrayList<>();
            StreamMenu menu = new StreamMenu(activity, () -> state, item -> {
                chosen.add(item);
                if (item == OVERLAY) state.overlayShown = !state.overlayShown;
            }, null, saved);
            menu.show();

            // The tiles in the model's order, each the row of its item; the buttons on the model's tiles.
            StreamMenuModel.Layout layout = menu.layout();
            TileGrid grid = menu.grid();
            int statusRows = 0;
            for (StreamMenuModel.Cell cell : layout.grid) {
                if (menu.cell(layout.grid.indexOf(cell)) instanceof MenuRow) statusRows++;
            }
            assertEquals(layout.grid.size(), grid.getChildCount() + statusRows);
            assertSame(menu.row(KEYBOARD), grid.getChildAt(0));
            assertEquals("Y", ((TileGrid.Tile) menu.row(KEYBOARD)).hotkey());
            assertEquals("X", ((TileGrid.Tile) menu.row(CONTROLLER_MOUSE)).hotkey());
            assertEquals("RB", ((TileGrid.Tile) menu.row(OVERLAY)).hotkey());
            assertEquals("LB", ((TileGrid.Tile) menu.row(SHORTCUTS)).hotkey());
            assertNull(((TileGrid.Tile) menu.row(PC_KEYBOARD)).hotkey());
            assertTrue("A switch carries its state", menu.row(CONTROLLER_MOUSE).isCheckable());
            assertFalse("An action does not", menu.row(KEYBOARD).isCheckable());
            // More, then the user's shortcuts with their keys, then Add.
            int more = grid.indexOfChild(menu.row(MORE));
            assertEquals(more + 3, grid.getChildCount() - 1);
            assertEquals("Snip\nWin + Shift + S", text(grid.getChildAt(more + 1)));
            assertEquals("Steam overlay\nShift + Tab", text(grid.getChildAt(more + 2)));
            assertEquals(activity.getString(R.string.keyboard_shortcuts_add), text(grid.getChildAt(more + 3)));
            assertTrue(items(menu.entries()).containsAll(Arrays.asList(KEYBOARD, OVERLAY, DISCONNECT)));
            assertFalse("Rarely used items wait on the More page", items(menu.entries()).contains(RECONNECT));
            // The round X in the header goes back; Disconnect stands in the panel's footer, outside the part that scrolls.
            assertTrue(menu.sheet().isPanel());
            assertSame(menu.sheet().panelClose(), menu.row(CLOSE));
            assertEquals(activity.getString(R.string.stream_tile_return) + ", B", menu.row(CLOSE).getContentDescription().toString());
            assertFalse("the header does not scroll", inScroll(menu.row(CLOSE)));
            android.view.ViewGroup footer = (android.view.ViewGroup) menu.row(DISCONNECT).getParent();
            assertTrue(footer instanceof ActionSheet.FooterBar);
            assertTrue("the grid scrolls", inScroll(grid));
            assertFalse("the ways out do not", inScroll(footer));

            // RB flips the overlay in place, the release goes with it, and the menu stays.
            press(menu, KeyEvent.KEYCODE_BUTTON_R1);
            assertEquals(Arrays.asList(OVERLAY), chosen);
            assertTrue(menu.row(OVERLAY).isChecked());
            assertTrue(menu.sheet().isShowing());
            // A shortcut tile sends its keys.
            grid.getChildAt(more + 1).performClick();
            assertArrayEquals(KeyboardShortcuts.parseChord("WIN+SHIFT+S"), saved.sent.get(0));
            // LB opens the shortcuts, where the buttons do nothing; B returns; Y runs the keyboard.
            press(menu, KeyEvent.KEYCODE_BUTTON_L1);
            assertEquals(StreamMenu.Page.SHORTCUTS, menu.page());
            press(menu, KeyEvent.KEYCODE_BUTTON_Y);
            assertEquals(StreamMenu.Page.SHORTCUTS, menu.page());
            press(menu, KeyEvent.KEYCODE_BUTTON_B);
            assertEquals(StreamMenu.Page.MAIN, menu.page());
            press(menu, KeyEvent.KEYCODE_BUTTON_Y);
            assertEquals(Arrays.asList(OVERLAY, KEYBOARD), chosen);
            // The X closes without choosing anything.
            menu.row(CLOSE).performClick();
            assertFalse(menu.sheet().isShowing());
            assertEquals(Arrays.asList(OVERLAY, KEYBOARD), chosen);
        }
    }

    @Test
    public void everyItemTheFirstPageCanOfferIsATileOfTheSharedModel() throws Exception {
        RustStreamMenuModel model = new RustStreamMenuModel();
        Set<String> ids = new java.util.HashSet<>();
        for (RustStreamMenuModel.Tile tile : model.tiles) ids.add(tile.id);
        StreamMenu.Options o = options(nativeApiHost(), NvApp.Role.STREAM);
        o.microphoneOffered = o.micButtonAvailable = o.mouseConnected = o.controllerMouseAvailable = true;
        o.onscreenControls = o.gameControls = o.scalingAvailable = o.zoomEnabled = o.pictureInPicture = true;
        o.externalDisplays = o.hostMenu = o.usbForwarding = o.imageClipboard = o.nightModeOffered = o.headphoneSurroundOffered = true;
        o.hostAudio = NativeApiFixtures.audio(NativeApiFixtures.audioJson(true, true));
        List<StreamMenu.Entry> entries = StreamMenu.mainPage(o);
        entries.addAll(StreamMenu.mainPage(options("rubylight-serverinfo.xml", NvApp.Role.REMOTE_MONITOR)));
        assertTrue(entries.size() > 30);
        for (StreamMenu.Entry entry : entries) {
            assertTrue(entry.item + " has a tile in protocol/policy/src/stream_menu.rs",
                    ids.contains(entry.item.name().toLowerCase(java.util.Locale.ROOT)));
        }
        assertTrue(ids.contains("shortcuts"));
        // The ways out sit in the footer; End session, which leaves the game running, is a row under More.
        for (RustStreamMenuModel.Tile tile : model.tiles) {
            if (tile.id.equals("end_host_session")) {
                assertEquals(tile.id, "more", tile.place);
            } else if (tile.id.equals("disconnect") || tile.id.endsWith("_game") || tile.id.startsWith("end_")) {
                assertEquals(tile.id, "footer", tile.place);
            }
        }
    }

    /** Two ways out under the grid, Disconnect and Quit game; End session waits last on More, red, and asks first there. */
    @Test
    public void theFooterHoldsDisconnectAndQuitGameAndEndSessionIsUnderMore() {
        try (var controller = Robolectric.buildActivity(Activity.class).setup()) {
            Activity activity = controller.get();
            activity.setTheme(R.style.StreamTheme);
            for (boolean hostControl : new boolean[] {false, true}) {
                StreamMenu.Options state = options(nativeApiHost(), NvApp.Role.STREAM);
                state.nativeHostControl = hostControl;
                StreamMenu.Item quit = hostControl ? CLOSE_GAME : QUIT_GAME;
                List<StreamMenu.Item> chosen = new ArrayList<>();
                StreamMenu menu = new StreamMenu(activity, () -> state, chosen::add);
                menu.show();
                android.view.ViewGroup footer = (android.view.ViewGroup) menu.row(DISCONNECT).getParent();
                assertTrue(footer instanceof ActionSheet.FooterBar);
                assertSame(footer, menu.row(quit).getParent());
                assertNull("End session is not a third way out", menu.row(END_HOST_SESSION));
                assertEquals(activity.getString(R.string.stream_tile_quit_game), menu.row(quit).getText().toString());

                menu.row(MORE).performClick();
                assertEquals(StreamMenu.Page.MORE, menu.page());
                List<StreamMenu.Item> more = items(menu.entries());
                assertEquals("End session closes More", END_HOST_SESSION, more.get(more.size() - 1));
                assertEquals(activity.getString(R.string.stream_end_session), menu.row(END_HOST_SESSION).getText().toString());
                menu.row(END_HOST_SESSION).performClick();
                assertEquals(Arrays.asList(END_HOST_SESSION), chosen);
                menu.sheet().dismiss();
            }
        }
    }

    /** A shortcut named after its keys (Caps Lock, the editor's default name) says them once, not twice. */
    @Test
    public void aShortcutNamedAfterItsKeysShowsNoKeyLine() {
        try (var controller = Robolectric.buildActivity(Activity.class).setup()) {
            Activity activity = controller.get();
            activity.setTheme(R.style.StreamTheme);
            StreamMenu.Options state = options("rubylight-serverinfo.xml", NvApp.Role.STREAM);
            SavedShortcuts saved = new SavedShortcuts().add("Caps Lock", "CAPS_LOCK").add("Snip", "WIN+SHIFT+S");
            StreamMenu menu = new StreamMenu(activity, () -> state, item -> { }, null, saved);
            menu.show();
            TileGrid grid = menu.grid();
            int more = grid.indexOfChild(menu.row(MORE));
            assertEquals("Caps Lock", text(grid.getChildAt(more + 1)));
            assertEquals("Snip\nWin + Shift + S", text(grid.getChildAt(more + 2)));
            menu.row(SHORTCUTS).performClick();
            TileGrid tiles = findGrid(menu.sheet().getWindow().getDecorView());
            List<String> texts = new ArrayList<>();
            for (int i = 0; i < tiles.getChildCount(); i++) texts.add(text(tiles.getChildAt(i)));
            assertTrue(texts.toString(), texts.contains("Caps Lock"));
            assertTrue(texts.toString(), texts.contains("Snip\nWin + Shift + S"));
            menu.sheet().dismiss();
        }
    }

    @Test
    public void aShortcutSavedInTheStreamTakesTheKeysPickedAndItsBuiltInStepsAside() {
        try (var controller = Robolectric.buildActivity(Activity.class).setup()) {
            Activity activity = controller.get();
            activity.setTheme(R.style.StreamTheme);
            StreamMenu.Options state = options("rubylight-serverinfo.xml", NvApp.Role.STREAM);
            SavedShortcuts saved = new SavedShortcuts().add("Switch", "ALT+TAB");
            StreamMenu menu = new StreamMenu(activity, () -> state, item -> { }, null, saved);
            menu.show();
            menu.row(SHORTCUTS).performClick();
            // Alt + Tab is saved under the user's name, so its built-in steps aside. The user's own come first, so a
            // built-in added in a later version never pushes them along (nightjar#61: Game Bar and Record that did).
            TileGrid tiles = findGrid(menu.sheet().getWindow().getDecorView());
            List<String> labels = new ArrayList<>();
            for (int i = 0; i < tiles.getChildCount(); i++) labels.add(text(tiles.getChildAt(i)).split("\n")[0]);
            assertEquals(Arrays.asList("Switch", activity.getString(R.string.keyboard_task_manager), activity.getString(R.string.keyboard_close_window),
                    activity.getString(R.string.stream_menu_start_menu), activity.getString(R.string.desktop_controls_shortcut_desktop),
                    activity.getString(R.string.keyboard_game_bar), activity.getString(R.string.keyboard_record_that),
                    activity.getString(R.string.keyboard_shortcuts_add)), labels);
            tiles.getChildAt(0).performClick();
            assertArrayEquals(KeyboardShortcuts.parseChord("ALT+TAB"), saved.sent.get(0));
            tiles.getChildAt(1).performClick();
            assertArrayEquals(KeyboardShortcuts.parseChord("CTRL+SHIFT+ESC"), saved.sent.get(1));

            // Add: no keys is refused and says so; Ctrl, Shift and S without a name save as their keys.
            tiles.getChildAt(tiles.getChildCount() - 1).performClick();
            assertEquals(StreamMenu.Page.EDITOR, menu.page());
            menu.saveDraft();
            assertEquals(StreamMenu.Page.EDITOR, menu.page());
            assertEquals(1, saved.list.size());
            TileGrid caps = findGrid(menu.sheet().getWindow().getDecorView());
            for (String key : new String[] {"Ctrl", "Shift", "S"}) capNamed(caps, key).performClick();
            assertTrue(capNamed(caps, "Ctrl").isChecked());
            menu.saveDraft();
            assertEquals(StreamMenu.Page.SHORTCUTS, menu.page());
            assertEquals(2, saved.list.size());
            assertEquals("Ctrl + Shift + S", saved.list.get(1).name);
            assertArrayEquals(KeyboardShortcuts.parseChord("CTRL+SHIFT+S"), saved.list.get(1).keys());
            menu.sheet().onBackPressed();
            assertEquals(StreamMenu.Page.MAIN, menu.page());
            menu.sheet().dismiss();
        }
    }

    /**
     * Without libnightjar_core (missing, or from another build) the stream still runs on the C engine. The menu kept
     * only Return and the ways out, so Keyboard, Mouse, Overlay, Quality, the clipboard and the shortcuts were out of
     * reach: every offered entry stays as a plain list, and More lists them all.
     */
    @Test
    public void aModelThatCannotAnswerSaysWhyAndKeepsEveryEntryReachable() {
        StreamMenuModel.use(request -> "{\"error\":\"unavailable\",\"detail\":\"libnightjar_core.so not found\"}");
        try (var controller = Robolectric.buildActivity(Activity.class).setup()) {
            Activity activity = controller.get();
            activity.setTheme(R.style.StreamTheme);
            List<StreamMenu.Item> chosen = new ArrayList<>();
            StreamMenu.Options state = options(nativeApiHost(), NvApp.Role.STREAM);
            state.controllerMouseAvailable = true;
            SavedShortcuts saved = new SavedShortcuts().add("Switch", "ALT+TAB");
            StreamMenu menu = new StreamMenu(activity, () -> state, chosen::add, null, saved);
            menu.show();
            assertNotNull(menu.failure());
            assertTrue(menu.failure().getText().toString().contains("libnightjar_core.so not found"));
            assertNull(menu.grid());
            List<StreamMenu.Item> offered = items(StreamMenu.mainPage(state));
            offered.add(SHORTCUTS);
            for (StreamMenu.Item item : offered) assertNotNull(item + " is reachable", menu.row(item));
            for (StreamMenu.Item item : new StreamMenu.Item[] {KEYBOARD, CONTROLLER_MOUSE, OVERLAY, CLIPBOARD_SEND, CLIPBOARD_RECEIVE,
                    NATIVE_PAGE, DISCONNECT, QUIT_GAME}) {
                assertTrue(item + " offered here", offered.contains(item));
            }
            // Focus starts on the first entry that does not end the stream.
            assertTrue(menu.row(KEYBOARD).isFocusedByDefault());
            menu.row(CLIPBOARD_RECEIVE).performClick();
            menu.row(DISCONNECT).performClick();
            assertEquals(Arrays.asList(CLIPBOARD_RECEIVE, DISCONNECT), chosen);
            // The shortcuts still open and send without the model.
            menu.row(SHORTCUTS).performClick();
            assertEquals(StreamMenu.Page.SHORTCUTS, menu.page());
            TileGrid tiles = findGrid(menu.sheet().getWindow().getDecorView());
            tiles.getChildAt(0).performClick();
            assertArrayEquals(KeyboardShortcuts.parseChord("ALT+TAB"), saved.sent.get(0));
            menu.sheet().dismiss();
        }
    }

    /** An old libnightjar_core without the call threw on the UI thread when the menu opened. */
    @Test
    public void aLibraryWithoutTheCallLeavesTheMenuUsable() {
        StreamMenuModel.use(request -> { throw new UnsatisfiedLinkError("No implementation found for java.lang.String com.limelight.ui.StreamMenuModel.call"); });
        try (var controller = Robolectric.buildActivity(Activity.class).setup()) {
            Activity activity = controller.get();
            activity.setTheme(R.style.StreamTheme);
            StreamMenu.Options state = options("rubylight-serverinfo.xml", NvApp.Role.STREAM);
            state.appName = "Open World";
            state.streamWidth = 1920;
            state.streamHeight = 1080;
            state.streamFps = 60;
            StreamMenu menu = new StreamMenu(activity, () -> state, item -> { }, null, null);
            menu.show();
            assertTrue(menu.failure().getText().toString().contains("No implementation found"));
            assertNotNull(menu.row(KEYBOARD));
            assertNotNull(menu.row(DISCONNECT));
            menu.sheet().dismiss();
        }
    }

    /** Tapped Esc, then Ctrl and Shift: the shortcut is saved as Ctrl + Shift + Esc, so it opens Task Manager. */
    @Test
    public void keysPickedEscFirstAreSavedModifiersFirst() {
        try (var controller = Robolectric.buildActivity(Activity.class).setup()) {
            Activity activity = controller.get();
            activity.setTheme(R.style.StreamTheme);
            StreamMenu.Options state = options("rubylight-serverinfo.xml", NvApp.Role.STREAM);
            SavedShortcuts saved = new SavedShortcuts();
            StreamMenu menu = new StreamMenu(activity, () -> state, item -> { }, null, saved);
            menu.show();
            menu.row(SHORTCUTS).performClick();
            TileGrid tiles = findGrid(menu.sheet().getWindow().getDecorView());
            tiles.getChildAt(tiles.getChildCount() - 1).performClick();
            TileGrid caps = findGrid(menu.sheet().getWindow().getDecorView());
            for (String key : new String[] {"Esc", "Ctrl", "Shift"}) capNamed(caps, key).performClick();
            menu.saveDraft();
            assertEquals(StreamMenu.Page.SHORTCUTS, menu.page());
            assertArrayEquals(KeyboardShortcuts.parseChord("CTRL+SHIFT+ESC"), saved.list.get(0).keys());
            // Its name follows the saved order too: the Retroid named a Caps Lock then Shift draft "Caps Lock + Shift".
            assertEquals("Ctrl + Shift + Esc", saved.list.get(0).name);
            menu.sheet().dismiss();
        }
    }

    /**
     * An older shortcut can hold a key the picker lacks (F13, or a key the model cannot save): it shows as a cap of its
     * own, checked, the error names it, and taking it out lets the rest be saved.
     */
    @Test
    public void anOldShortcutsKeyThePickerLacksIsARemovableCapAndTheErrorNamesIt() {
        RustStreamMenuModel rust = new RustStreamMenuModel();
        // As shortcuts::key_code refuses a key it has no portable name for.
        StreamMenuModel.use(request -> request.contains("\"validate\"") && request.contains("F13")
                ? "{\"error\":\"unsupported_key\",\"detail\":\"F13\"}" : rust.call(request));
        try (var controller = Robolectric.buildActivity(Activity.class).setup()) {
            Activity activity = controller.get();
            activity.setTheme(R.style.StreamTheme);
            StreamMenu.Options state = options("rubylight-serverinfo.xml", NvApp.Role.STREAM);
            SavedShortcuts saved = new SavedShortcuts().add("Old", "CTRL+F13");
            StreamMenu menu = new StreamMenu(activity, () -> state, item -> { }, null, saved);
            menu.show();
            menu.row(SHORTCUTS).performClick();
            buttonStartingWith(menu.sheet().getWindow().getDecorView(), "Old", true).performClick();
            assertEquals(StreamMenu.Page.EDITOR, menu.page());
            TileGrid caps = findGrid(menu.sheet().getWindow().getDecorView());
            assertEquals("F13", text(caps.getChildAt(0)));
            assertTrue(capNamed(caps, "F13").isChecked());
            assertTrue(capNamed(caps, "Ctrl").isChecked());
            menu.saveDraft();
            assertEquals(StreamMenu.Page.EDITOR, menu.page());
            assertTrue(textShown(menu.sheet().getWindow().getDecorView(), activity.getString(R.string.stream_shortcut_key_unsupported, "F13")));
            capNamed(caps, "F13").performClick();
            capNamed(caps, "F1").performClick();
            menu.saveDraft();
            assertEquals(StreamMenu.Page.SHORTCUTS, menu.page());
            assertEquals(1, saved.list.size());
            assertArrayEquals(KeyboardShortcuts.parseChord("CTRL+F1"), saved.list.get(0).keys());
            menu.sheet().dismiss();
        }
    }

    @Test
    public void theAndroidCallReachesTheRustExportAndEveryPickerKeyIsOneTheAppCanSave() throws Exception {
        String rust = RustStreamMenuModel.read("../../android/src/stream_menu.rs");
        assertTrue(rust.contains("fn Java_com_limelight_ui_StreamMenuModel_call("));
        assertTrue(rust.matches("(?s).*request: JString,\\s*\\) -> jstring.*"));
        java.lang.reflect.Method call = StreamMenuModel.class.getDeclaredMethod("call", String.class);
        assertTrue(java.lang.reflect.Modifier.isNative(call.getModifiers()));
        assertTrue(java.lang.reflect.Modifier.isStatic(call.getModifiers()));
        assertEquals(String.class, call.getReturnType());
        for (StreamMenuModel.Key key : StreamMenuModel.picker()) {
            assertEquals(key.name, 1, KeyboardShortcuts.parseChord(key.name).length);
            // Saved under the picker's own name, so the model can label it and match it against a built-in.
            assertEquals(key.name, KeyboardShortcuts.format(KeyboardShortcuts.parseChord(key.name)));
        }
        for (String id : new RustStreamMenuModel().builtIns.keySet()) assertNotEquals(0, StreamMenu.builtInLabel(id));
    }
}
