package com.limelight.ui;

import android.app.Activity;
import android.app.Application;
import android.view.KeyEvent;
import android.view.View;

import com.google.android.material.button.MaterialButton;
import com.limelight.R;
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

import static com.limelight.ui.StreamMenu.Item.*;
import static com.limelight.ui.StreamMenuTestSupport.*;
import static org.junit.Assert.*;

/**
 * Interactive navigation, header/mode display, bitrate row updates, TV/D-pad focus, font scaling,
 * and live host-audio refresh tests for {@link StreamMenu}.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class StreamMenuNavigationTest {
    @Before
    public void answerFromThePolicySources() {
        StreamMenuModel.use(new RustStreamMenuModel());
    }

    @After
    public void forgetTheModel() {
        StreamMenuModel.use(null);
    }

    /** As desktop Nightjar's: the game over the stream's mode from the shared model, the PC while the mode is unknown. */
    @Test
    public void theHeaderNamesTheGameOverTheStreamsMode() {
        try (var controller = Robolectric.buildActivity(Activity.class).setup()) {
            Activity activity = controller.get();
            activity.setTheme(R.style.StreamTheme);
            StreamMenu.Options state = options("rubylight-serverinfo.xml", NvApp.Role.STREAM);
            state.appName = "Open World";
            state.host.name = "Gaming PC";
            state.streamWidth = 2560;
            state.streamHeight = 1440;
            state.streamFps = 144;
            state.videoFormat = 0x0200;
            state.hdr = true;
            StreamMenu menu = new StreamMenu(activity, () -> state, item -> { }, null, null);
            assertArrayEquals(new String[] {"Open World", "1440p · 144 FPS · HEVC HDR"}, menu.header(state));
            StreamMenu.Options starting = options("rubylight-serverinfo.xml", NvApp.Role.STREAM);
            starting.appName = "Open World";
            starting.host.name = "Gaming PC";
            assertArrayEquals(new String[] {"Open World", "Gaming PC"}, menu.header(starting));
            menu.show();
            assertEquals("Open World", menu.sheet().heading().toString());
            assertEquals("1440p · 144 FPS · HEVC HDR", menu.sheet().subtitle().getText().toString());
            // Another page shows its own title and no mode.
            menu.row(MORE).performClick();
            assertEquals(View.GONE, menu.sheet().subtitle().getVisibility());
            menu.sheet().dismiss();

            StreamMenu.Options monitor = options("rubylight-serverinfo.xml", NvApp.Role.REMOTE_MONITOR);
            monitor.appName = "Desktop";
            assertEquals(activity.getString(R.string.stream_monitor_title), menu.header(monitor)[0]);
        }
    }

    /** Without the live frame-rate change the row sets the bitrate, so it shows the bitrate, not the frame rate (nightjar #134). */
    @Test
    public void theBitrateRowShowsTheBitrateAndTheRateChangeRowTheFrameRate() {
        try (var controller = Robolectric.buildActivity(Activity.class).setup()) {
            Activity activity = controller.get();
            activity.setTheme(R.style.StreamTheme);
            StreamMenu.Options state = options("rubylight-serverinfo.xml", NvApp.Role.STREAM);
            state.streamFps = 60;
            state.bitrateKbps = 80000;
            state.bitrateChange = kbps -> kbps;
            StreamMenu menu = new StreamMenu(activity, () -> state, item -> { }, null, null);
            menu.show();
            MenuRow bitrate = (MenuRow) menu.row(BITRATE);
            assertEquals(BitrateSlider.text(activity, 80000), bitrate.value().toString());
            // Moving the slider moves the row's value with it.
            long now = android.os.SystemClock.uptimeMillis();
            assertTrue(menu.bitrateSlider().onKeyDown(android.view.KeyEvent.KEYCODE_DPAD_LEFT,
                    new android.view.KeyEvent(now, now, android.view.KeyEvent.ACTION_DOWN, android.view.KeyEvent.KEYCODE_DPAD_LEFT, 0)));
            org.robolectric.shadows.ShadowLooper.idleMainLooper();
            assertNotEquals(BitrateSlider.text(activity, 80000), bitrate.value().toString());
            menu.sheet().dismiss();

            state.rateChange = true;
            menu = new StreamMenu(activity, () -> state, item -> { }, null, null);
            menu.show();
            assertEquals("60 FPS", ((MenuRow) menu.row(RATE_CHANGE)).value().toString());
            menu.sheet().dismiss();
        }
    }

    @Test
    public void backInsideThePageReturnsToTheMenuAndBackAgainClosesIt() {
        try (var controller = Robolectric.buildActivity(Activity.class).setup()) {
            Activity activity = controller.get();
            activity.setTheme(R.style.StreamTheme);
            List<StreamMenu.Item> chosen = new ArrayList<>();
            StreamMenu menu = new StreamMenu(activity, () -> options("rubylight-serverinfo.xml", NvApp.Role.STREAM), chosen::add);
            menu.show();
            View back = menu.sheet().findViewById(R.id.action_sheet_back);
            assertEquals("The first page has no back arrow", View.GONE, back.getVisibility());
            menu.row(OVERLAY_PAGE).performClick();
            assertEquals(StreamMenu.Page.OVERLAY, menu.page());
            assertNotNull(menu.row(OVERLAY_DEBUG));
            assertNull(menu.row(DISCONNECT));
            assertTrue(chosen.isEmpty());
            // Touch has a back arrow on the page; it does what Back does.
            assertEquals(View.VISIBLE, back.getVisibility());
            back.performClick();
            assertEquals(StreamMenu.Page.MAIN, menu.page());
            assertEquals(View.GONE, back.getVisibility());
            menu.row(OVERLAY_PAGE).performClick();
            menu.sheet().onBackPressed();
            assertTrue(menu.sheet().isShowing());
            assertEquals(StreamMenu.Page.MAIN, menu.page());
            assertNotNull(menu.row(DISCONNECT));
            menu.sheet().onBackPressed();
            assertFalse(menu.sheet().isShowing());
        }
    }

    /** The text size of a quick toggle, a page row and an action row, in pixels. */
    private static float[] textSizes(float fontScale) {
        org.robolectric.RuntimeEnvironment.setFontScale(fontScale);
        try (var controller = Robolectric.buildActivity(Activity.class).setup()) {
            Activity activity = controller.get();
            activity.setTheme(R.style.StreamTheme);
            StreamMenu menu = new StreamMenu(activity, () -> options("rubylight-serverinfo.xml", NvApp.Role.STREAM), item -> { });
            menu.show();
            return new float[] {
                    ((MaterialButton) menu.row(KEYBOARD)).getTextSize(),
                    ((MaterialButton) menu.row(OVERLAY_PAGE)).getTextSize(),
                    ((MaterialButton) menu.row(DISCONNECT)).getTextSize()};
        } finally {
            org.robolectric.RuntimeEnvironment.setFontScale(1f);
        }
    }

    @Test
    public void theMenuTextFollowsTheSystemFontScale() {
        float[] normal = textSizes(1f);
        float[] large = textSizes(1.3f);
        for (int i = 0; i < normal.length; i++) {
            assertTrue("text " + i + " is drawn", normal[i] > 0);
            // A tile's 12sp lands on whole pixels, so its ratio is 16/12 rather than 1.3.
            assertEquals("text " + i + " grows with the font scale", 1.3f, large[i] / normal[i], 0.04f);
        }
    }

    @Test @Config(qualifiers = "television")
    public void tvMenusKeepRemoteActionsAndLeaveOutTouchOnlyControls() {
        try (var controller = Robolectric.buildActivity(Activity.class).setup()) {
            Activity activity = controller.get();
            activity.setTheme(R.style.StreamTheme);
            StreamMenu.Options state = options("rubylight-serverinfo.xml", NvApp.Role.STREAM);
            state.onscreenControls = true;
            state.gameControls = true;
            state.hostMenu = true;
            List<StreamMenu.Item> chosen = new ArrayList<>();
            StreamMenu menu = new StreamMenu(activity, () -> state, chosen::add);
            menu.show();
            for (StreamMenu.Item item : new StreamMenu.Item[]{TOUCH_MODE, ONSCREEN_CONTROLS, CONTROLS_LAYOUT, GAME_CONTROLS, ZOOM}) {
                assertNull(menu.row(item));
                assertFalse(items(menu.entries()).contains(item));
            }
            assertEquals("Keyboard", menu.row(KEYBOARD).getText().toString());
            assertNotNull(menu.row(DISCONNECT));
            menu.row(MORE).performClick();
            assertEquals(StreamMenu.Page.MORE, menu.page());
            assertNotNull(menu.row(KEYBOARD_KEYS));
            assertEquals("Open the PC’s menu", menu.row(HOST_MENU).getText().toString());
            menu.row(HOST_MENU).performClick();
            assertEquals(Arrays.asList(HOST_MENU), chosen);
            menu.back();
            assertEquals(StreamMenu.Page.MAIN, menu.page());
            menu.row(OVERLAY_PAGE).performClick();
            assertEquals(StreamMenu.Page.OVERLAY, menu.page());
            menu.back();
            assertEquals(StreamMenu.Page.MAIN, menu.page());
            menu.sheet().dismiss();
        }
    }

    @Test
    public void controllerBAndEscapeStepBackFromThePageToo() {
        try (var controller = Robolectric.buildActivity(Activity.class).setup()) {
            Activity activity = controller.get();
            activity.setTheme(R.style.StreamTheme);
            for (int key : new int[] {KeyEvent.KEYCODE_BUTTON_B, KeyEvent.KEYCODE_ESCAPE}) {
                StreamMenu menu = new StreamMenu(activity, () -> options("rubylight-serverinfo.xml", NvApp.Role.STREAM), item -> { });
                menu.show();
                menu.row(OVERLAY_PAGE).performClick();
                press(menu, key);
                assertEquals(StreamMenu.Page.MAIN, menu.page());
                assertTrue(menu.sheet().isShowing());
                press(menu, key);
                assertFalse(menu.sheet().isShowing());
            }
        }
    }

    @Test
    public void quickTogglesChangeInPlaceAndOtherItemsReportOnce() {
        try (var controller = Robolectric.buildActivity(Activity.class).setup()) {
            Activity activity = controller.get();
            activity.setTheme(R.style.StreamTheme);
            StreamMenu.Options state = options("rubylight-serverinfo.xml", NvApp.Role.STREAM);
            List<StreamMenu.Item> chosen = new ArrayList<>();
            StreamMenu menu = new StreamMenu(activity, () -> state, item -> {
                chosen.add(item);
                if (item == OVERLAY) state.overlayShown = !state.overlayShown;
            });
            menu.show();
            assertFalse("Keyboard is an action, not a switch", menu.row(KEYBOARD).isCheckable());
            assertEquals("the model's short label, as desktop Nightjar's", activity.getString(R.string.stream_tile_keyboard),
                    menu.row(KEYBOARD).getText().toString());
            assertFalse("The PC keyboard is an action too", menu.row(PC_KEYBOARD).isCheckable());
            assertEquals(activity.getString(R.string.pc_keyboard_title), menu.row(PC_KEYBOARD).getText().toString());
            MaterialButton overlay = menu.row(OVERLAY);
            assertTrue(overlay.isCheckable());
            assertFalse(overlay.isChecked());
            overlay.performClick();
            assertTrue(menu.row(OVERLAY).isChecked());
            assertSame("The row is updated, not rebuilt", overlay, menu.row(OVERLAY));
            menu.row(KEYBOARD).performClick();
            menu.row(DISCONNECT).performClick();
            assertEquals(Arrays.asList(OVERLAY, KEYBOARD, DISCONNECT), chosen);
        }
    }

    @Test
    public void aFailedAudioReadSaysSoInTheMenu() throws Exception {
        try (var controller = Robolectric.buildActivity(Activity.class).setup()) {
            Activity activity = controller.get();
            activity.setTheme(R.style.StreamTheme);
            StreamMenu.Options state = options(nativeApiHost(), NvApp.Role.STREAM);
            state.nightModeOffered = true;
            state.headphoneSurroundOffered = true;
            StreamMenu menu = new StreamMenu(activity, () -> state, item -> { });
            menu.show();
            assertNull("Nothing while the host has not answered", menu.audioNote());

            // GET /audio failed: no switches, and a line says why.
            state.hostAudioUnread = true;
            menu.refresh();
            assertNull(menu.row(NIGHT_MODE));
            assertNotNull(menu.audioNote());
            assertEquals(activity.getString(R.string.stream_audio_unread), menu.audioNote().getText().toString());
            assertTrue(menu.audioNote().isShown());

            // The next answer that works replaces the line with the switches, on the More page.
            state.hostAudioUnread = false;
            state.hostAudio = NativeApiFixtures.audio(NativeApiFixtures.audioJson(false, false));
            menu.refresh();
            assertNull(menu.audioNote());
            menu.row(MORE).performClick();
            assertNotNull(menu.row(NIGHT_MODE));

            // Remote Monitor changes nothing on the PC, so it has nothing to say about it either.
            StreamMenu.Options monitor = options(nativeApiHost(), NvApp.Role.REMOTE_MONITOR);
            monitor.hostAudioUnread = true;
            StreamMenu watching = new StreamMenu(activity, () -> monitor, item -> { });
            watching.show();
            assertNull(watching.audioNote());
            menu.sheet().dismiss();
            watching.sheet().dismiss();
        }
    }

    @Test
    public void theAudioSwitchesAppearWhenTheHostAnswersAndShowItsNewState() throws Exception {
        try (var controller = Robolectric.buildActivity(Activity.class).setup()) {
            Activity activity = controller.get();
            activity.setTheme(R.style.StreamTheme);
            StreamMenu.Options state = options(nativeApiHost(), NvApp.Role.STREAM);
            state.nightModeOffered = true;
            state.headphoneSurroundOffered = true;
            List<StreamMenu.Item> chosen = new ArrayList<>();
            StreamMenu menu = new StreamMenu(activity, () -> state, chosen::add);
            menu.show();
            assertNull(menu.row(NIGHT_MODE));

            // GET /audio answers after the menu opened; the switches sit on the More page.
            state.hostAudio = NativeApiFixtures.audio(NativeApiFixtures.audioJson(false, false));
            menu.refresh();
            assertNull(menu.row(NIGHT_MODE));
            menu.row(MORE).performClick();
            MaterialButton night = menu.row(NIGHT_MODE);
            assertEquals(activity.getString(R.string.stream_night_mode), night.getText().toString());
            assertTrue(night.isCheckable());
            assertFalse(night.isChecked());
            assertNotNull(menu.row(HEADPHONE_SURROUND));

            // The switch asks; until the host answers it shows the host's state, then the new one in place.
            night.performClick();
            assertEquals(Arrays.asList(NIGHT_MODE), chosen);
            assertTrue(menu.sheet().isShowing());
            assertFalse(menu.row(NIGHT_MODE).isChecked());
            state.hostAudio = NativeApiFixtures.audio(NativeApiFixtures.audioJson(true, false));
            menu.refresh();
            assertTrue(menu.row(NIGHT_MODE).isChecked());
            assertSame("The row is updated, not rebuilt", night, menu.row(NIGHT_MODE));
            menu.sheet().dismiss();
        }
    }

    @Test
    public void everyRowTakesDpadFocusAndTheFirstToggleHasItFirst() {
        try (var controller = Robolectric.buildActivity(Activity.class).setup()) {
            Activity activity = controller.get();
            activity.setTheme(R.style.StreamTheme);
            StreamMenu menu = new StreamMenu(activity, () -> options("rubylight-serverinfo.xml", NvApp.Role.STREAM), item -> { });
            menu.show();
            for (StreamMenu.Entry entry : menu.entries()) {
                View row = menu.row(entry.item);
                assertTrue(entry.item + " is focusable", row.isFocusable());
                assertTrue(entry.item + " is clickable", row.isClickable());
            }
            assertTrue(menu.row(KEYBOARD).isFocusedByDefault());
            assertFalse(menu.row(DISCONNECT).isFocusedByDefault());
            menu.row(OVERLAY_PAGE).performClick();
            assertTrue("The page's first row takes the focus", menu.row(OVERLAY_OFF).isFocusedByDefault());
        }
    }

    @Test
    public void withoutTogglesFocusStartsPastDisconnectAndBackClosesWithoutLeaving() {
        try (var controller = Robolectric.buildActivity(Activity.class).setup()) {
            Activity activity = controller.get();
            activity.setTheme(R.style.StreamTheme);
            List<StreamMenu.Item> chosen = new ArrayList<>();
            StreamMenu menu = new StreamMenu(activity,
                    () -> options("butterpollo-viewer-serverinfo.xml", NvApp.Role.INPUT_ONLY), chosen::add);
            menu.show();
            assertFalse("OK right after opening must not disconnect", menu.row(DISCONNECT).isFocusedByDefault());
            assertTrue("The first tile has the focus", menu.grid().getChildAt(0).isFocusedByDefault());
            // Back opened the menu; Back again only closes it.
            menu.sheet().onBackPressed();
            assertFalse(menu.sheet().isShowing());
            assertTrue(chosen.isEmpty());
        }
    }

    /** The stream menu on a 1080p TV, both pages: no text under the TV caption size, and the sheet inside the safe area. */
    @Test @Config(qualifiers = "w960dp-h540dp-land-television-xhdpi")
    public void tvMenuTextIsReadableAndTheSheetFitsInsideTheOverscan() {
        try (var controller = Robolectric.buildActivity(Activity.class).setup()) {
            Activity activity = controller.get();
            activity.setTheme(R.style.StreamTheme);
            StreamMenu.Options state = options("rubylight-serverinfo.xml", NvApp.Role.STREAM);
            state.hostMenu = true;
            StreamMenu menu = new StreamMenu(activity, () -> state, item -> { });
            menu.show();
            assertEquals(java.util.Collections.emptyList(), TvAudit.findings(menu.sheet()));
            menu.row(OVERLAY_PAGE).performClick();
            assertEquals(java.util.Collections.emptyList(), TvAudit.findings(menu.sheet()));
            menu.sheet().dismiss();
        }
    }

    /** Disconnect's and End session's confirms say different things: one waits to resume, the other gives the PC back. */
    @Test
    public void endSessionsConfirmSaysWhatSetsItApartFromDisconnect() {
        android.content.Context context = org.robolectric.RuntimeEnvironment.getApplication();
        String disconnect = context.getString(R.string.stream_disconnect_confirm);
        String end = context.getString(R.string.stream_end_session_confirm);
        assertNotEquals(disconnect, end);
        assertTrue(disconnect, disconnect.contains("resume"));
        assertFalse(end, end.contains("resume"));
        assertTrue(end, end.contains("game keeps running"));
    }
}
