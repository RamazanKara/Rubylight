package com.limelight.ui;

import android.app.Activity;
import android.app.Application;
import android.view.KeyEvent;
import android.view.View;

import com.google.android.material.button.MaterialButton;
import com.limelight.R;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.http.TestHosts;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;

import java.util.ArrayList;
import java.util.Arrays;
import java.util.EnumSet;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.Set;

import static com.limelight.ui.StreamMenu.Item.*;
import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class StreamMenuTest {
    /** The Rubylight fixture on a 3.0 host that also speaks the native API. */
    private static ComputerDetails nativeApiHost() {
        ComputerDetails host = TestHosts.details("rubylight-serverinfo.xml");
        host.caps = TestHosts.caps("<root status_code=\"200\"><RustHostVersion>3.0.0</RustHostVersion>"
                + "<PairStatus>1</PairStatus><NativeApiVersion>1</NativeApiVersion></root>");
        return host;
    }

    private static StreamMenu.Options options(String fixture, NvApp.Role role) {
        return options(TestHosts.details(fixture), role);
    }

    private static StreamMenu.Options options(ComputerDetails host, NvApp.Role role) {
        StreamMenu.Options options = new StreamMenu.Options(role, host);
        // As NvConnection decides: the host's input permission, never for Remote Monitor.
        options.canSendInput = role != NvApp.Role.REMOTE_MONITOR && host.hasPermission(ComputerDetails.PERMISSION_INPUT);
        options.webUiAddress = true;
        // Only Rubylight changes resolution and frame rate in place, which automatic quality needs.
        options.automaticQuality = host.caps.family() == com.limelight.nvstream.http.HostCapabilities.Family.RUBYLIGHT;
        return options;
    }

    private static Map<StreamMenu.Section, List<StreamMenu.Item>> sections(List<StreamMenu.Entry> entries) {
        Map<StreamMenu.Section, List<StreamMenu.Item>> sections = new LinkedHashMap<>();
        for (StreamMenu.Entry entry : entries) {
            sections.computeIfAbsent(entry.section, ignored -> new ArrayList<>()).add(entry.item);
        }
        return sections;
    }

    @Test
    public void rubylightGetsTheQuickRowThenEverySectionThenTheSessionActions() {
        Map<StreamMenu.Section, List<StreamMenu.Item>> menu = sections(StreamMenu.mainPage(
                options("rubylight-serverinfo.xml", NvApp.Role.STREAM)));
        assertEquals(Arrays.asList(StreamMenu.Section.QUICK, StreamMenu.Section.INPUT, StreamMenu.Section.DISPLAY,
                StreamMenu.Section.QUALITY, StreamMenu.Section.HOST, StreamMenu.Section.SESSION), new ArrayList<>(menu.keySet()));
        assertEquals(Arrays.asList(KEYBOARD, PC_KEYBOARD, OVERLAY), menu.get(StreamMenu.Section.QUICK));
        assertEquals(Arrays.asList(TOUCH_MODE, ONSCREEN_CONTROLS, KEYBOARD_KEYS), menu.get(StreamMenu.Section.INPUT));
        assertEquals(Arrays.asList(ZOOM, OVERLAY_PAGE), menu.get(StreamMenu.Section.DISPLAY));
        // Resolution and frame rate (automatic quality) are Quality, not "Bitrate".
        assertEquals(Arrays.asList(BITRATE, AUTOMATIC_QUALITY, AUTOMATIC_BITRATE), menu.get(StreamMenu.Section.QUALITY));
        assertEquals(Arrays.asList(SERVER_COMMANDS, CLIPBOARD_SEND, CLIPBOARD_RECEIVE, HOST_WEB_UI, HOST_STATUS),
                menu.get(StreamMenu.Section.HOST));
        // Reconnect is a stream action, not a host command; quitting comes last, in the error colour.
        assertEquals(Arrays.asList(RECONNECT, DISCONNECT, QUIT_GAME), menu.get(StreamMenu.Section.SESSION));
        List<StreamMenu.Entry> entries = StreamMenu.mainPage(options("rubylight-serverinfo.xml", NvApp.Role.STREAM));
        StreamMenu.Entry last = entries.get(entries.size() - 1);
        assertTrue(last.destructive);
        assertEquals(R.string.stream_quit_game, last.label);
    }

    @Test
    public void eachHostFamilyShowsOnlyWhatItSupports() {
        Map<String, Set<StreamMenu.Item>> expected = new LinkedHashMap<>();
        // GameStream: no permissions, clipboard, commands, web UI or runtime bitrate.
        expected.put("gamestream-serverinfo.xml", EnumSet.noneOf(StreamMenu.Item.class));
        expected.put("sunshine-serverinfo.xml", EnumSet.of(HOST_WEB_UI));
        expected.put("apollo-serverinfo.xml", EnumSet.of(SERVER_COMMANDS, CLIPBOARD_SEND, CLIPBOARD_RECEIVE, HOST_WEB_UI, HOST_STATUS));
        // Vibepollo changes bitrate at runtime but reports no encoder bitrate for automatic bitrate.
        expected.put("vibepollo-serverinfo.xml", EnumSet.of(SERVER_COMMANDS, CLIPBOARD_SEND, CLIPBOARD_RECEIVE, HOST_WEB_UI,
                HOST_STATUS, BITRATE));
        expected.put("rubylight-serverinfo.xml", EnumSet.of(SERVER_COMMANDS, CLIPBOARD_SEND, CLIPBOARD_RECEIVE, HOST_WEB_UI,
                HOST_STATUS, BITRATE, AUTOMATIC_QUALITY, AUTOMATIC_BITRATE));
        Set<StreamMenu.Item> hostGated = EnumSet.of(SERVER_COMMANDS, CLIPBOARD_SEND, CLIPBOARD_RECEIVE, HOST_WEB_UI,
                HOST_STATUS, BITRATE, AUTOMATIC_QUALITY, AUTOMATIC_BITRATE);
        for (Map.Entry<String, Set<StreamMenu.Item>> host : expected.entrySet()) {
            Set<StreamMenu.Item> shown = EnumSet.noneOf(StreamMenu.Item.class);
            for (StreamMenu.Entry entry : StreamMenu.mainPage(options(host.getKey(), NvApp.Role.STREAM))) {
                if (hostGated.contains(entry.item)) shown.add(entry.item);
                // Every family keeps the stream's own actions.
            }
            assertEquals(host.getKey(), host.getValue(), shown);
            List<StreamMenu.Item> items = items(StreamMenu.mainPage(options(host.getKey(), NvApp.Role.STREAM)));
            assertTrue(host.getKey(), items.containsAll(Arrays.asList(KEYBOARD, OVERLAY, TOUCH_MODE, OVERLAY_PAGE,
                    RECONNECT, DISCONNECT, QUIT_GAME)));
            assertFalse(host.getKey(), items.contains(MICROPHONE));
        }
    }

    @Test
    public void everyRowHasItsOwnIcon() {
        StreamMenu.Options options = options("rubylight-serverinfo.xml", NvApp.Role.STREAM);
        options.microphoneOffered = true;
        options.mouseConnected = true;
        options.onscreenControls = true;
        options.gameControls = true;
        options.scalingAvailable = true;
        options.zoomEnabled = true;
        options.pictureInPicture = true;
        options.externalDisplays = true;
        List<StreamMenu.Entry> entries = new ArrayList<>(StreamMenu.mainPage(options));
        // The overlay modes are one choice that marks the chosen mode with a check.
        for (StreamMenu.Entry entry : StreamMenu.overlayPage(options)) {
            if (entry.section != StreamMenu.Section.OVERLAY_STYLE) entries.add(entry);
        }
        assertTrue(items(entries).containsAll(Arrays.asList(TOUCH_MODE, MOUSE_CURSOR, ONSCREEN_CONTROLS, CONTROLS_LAYOUT,
                GAME_CONTROLS, KEYBOARD_KEYS, VIDEO_SCALING, ZOOM, ZOOM_RESET, PICTURE_IN_PICTURE, VIDEO_OUTPUT,
                OVERLAY_COPY, STREAM_HEALTH, LATENCY_TEST)));
        Map<Integer, StreamMenu.Item> seen = new LinkedHashMap<>();
        for (StreamMenu.Entry entry : entries) {
            StreamMenu.Item before = seen.put(entry.icon, entry.item);
            assertNull(entry.item + " has the same icon as " + before, before);
        }
    }

    @Test
    public void aNativeHostWithHostControlEndsTheSessionAndClosesTheGame() {
        ComputerDetails host = nativeApiHost();
        StreamMenu.Options options = options(host, NvApp.Role.STREAM);
        options.nativeHostControl = true;
        List<StreamMenu.Entry> entries = StreamMenu.mainPage(options);
        List<StreamMenu.Item> items = items(entries);
        assertFalse(items.contains(QUIT_GAME));
        assertTrue(items.contains(DISCONNECT));
        // Disconnect leaves the game, then End session, then the destructive Close game last.
        assertTrue(items.indexOf(DISCONNECT) < items.indexOf(END_HOST_SESSION));
        assertTrue(items.indexOf(END_HOST_SESSION) < items.indexOf(CLOSE_GAME));
        StreamMenu.Entry last = entries.get(entries.size() - 1);
        assertEquals(CLOSE_GAME, last.item);
        assertTrue(last.destructive);
        assertEquals(R.string.stream_close_game, last.label);
        // A device that may not launch or quit gets neither, as before.
        host.permission = ComputerDetails.PERMISSION_VIEW;
        List<StreamMenu.Item> viewOnly = items(StreamMenu.mainPage(options(host, NvApp.Role.STREAM)));
        assertFalse(viewOnly.contains(CLOSE_GAME));
        assertFalse(viewOnly.contains(END_HOST_SESSION));
    }

    @Test
    public void aNativeHostWithoutHostControlStillEndsTheSessionAndKeepsQuitApp() {
        StreamMenu.Options options = options(nativeApiHost(), NvApp.Role.STREAM);
        List<StreamMenu.Entry> entries = StreamMenu.mainPage(options);
        List<StreamMenu.Item> items = items(entries);
        assertFalse(items.contains(CLOSE_GAME));
        assertTrue(items.contains(DISCONNECT));
        assertTrue(items.contains(END_HOST_SESSION));
        StreamMenu.Entry last = entries.get(entries.size() - 1);
        assertEquals(QUIT_GAME, last.item);
        assertTrue(last.destructive);
        assertEquals(R.string.stream_quit_game, last.label);
    }

    @Test
    public void otherHostsKeepQuitAppAndNeverShowTheNativeActions() {
        for (String fixture : new String[] {"rubylight-serverinfo.xml", "sunshine-serverinfo.xml"}) {
            List<StreamMenu.Item> items = items(StreamMenu.mainPage(options(fixture, NvApp.Role.STREAM)));
            assertTrue(fixture, items.contains(QUIT_GAME));
            assertFalse(fixture, items.contains(END_HOST_SESSION));
            assertFalse(fixture, items.contains(CLOSE_GAME));
        }
    }

    @Test
    public void automaticQualityNeedsAHostThatChangesTheStreamInPlace() {
        StreamMenu.Options options = options("rubylight-serverinfo.xml", NvApp.Role.STREAM);
        options.automaticQuality = false;
        assertFalse(items(StreamMenu.mainPage(options)).contains(AUTOMATIC_QUALITY));
        assertTrue(items(StreamMenu.mainPage(options)).contains(BITRATE));
    }

    @Test
    public void aViewOnlyDeviceGetsNoInputNorQuitAndNoClipboard() {
        List<StreamMenu.Item> items = items(StreamMenu.mainPage(options("butterpollo-viewer-serverinfo.xml", NvApp.Role.STREAM)));
        assertFalse(items.contains(KEYBOARD));
        assertFalse(items.contains(TOUCH_MODE));
        assertFalse(items.contains(QUIT_GAME));
        assertFalse(items.contains(CLIPBOARD_SEND));
        assertFalse(items.contains(SERVER_COMMANDS));
        assertTrue(items.contains(OVERLAY));
        assertTrue(items.contains(DISCONNECT));
    }

    @Test
    public void remoteMonitorAndInputOnlyEndTheirSessionAndKeepTheirLimits() {
        List<StreamMenu.Entry> monitor = StreamMenu.mainPage(options("rubylight-serverinfo.xml", NvApp.Role.REMOTE_MONITOR));
        List<StreamMenu.Item> items = items(monitor);
        assertFalse(items.contains(SERVER_COMMANDS));
        assertFalse(items.contains(CLIPBOARD_SEND));
        assertTrue(items.contains(CLIPBOARD_RECEIVE));
        assertFalse(items.contains(QUIT_GAME));
        assertEquals(END_SESSION, monitor.get(monitor.size() - 1).item);
        assertEquals(R.string.stream_end_monitor, monitor.get(monitor.size() - 1).label);
        assertEquals(R.string.stream_monitor_title, StreamMenu.title(NvApp.Role.REMOTE_MONITOR));

        List<StreamMenu.Entry> input = StreamMenu.mainPage(options("rubylight-serverinfo.xml", NvApp.Role.INPUT_ONLY));
        Map<StreamMenu.Section, List<StreamMenu.Item>> sections = sections(input);
        assertNull("Input-only has no video to show or tune", sections.get(StreamMenu.Section.DISPLAY));
        assertNull(sections.get(StreamMenu.Section.QUALITY));
        assertEquals(Arrays.asList(KEYBOARD, PC_KEYBOARD), sections.get(StreamMenu.Section.QUICK));
        assertEquals(R.string.stream_end_input, input.get(input.size() - 1).label);
        assertEquals(R.string.stream_input_title, StreamMenu.title(NvApp.Role.INPUT_ONLY));
    }

    @Test
    public void theMicrophoneToggleAppearsOnlyWhenTheHostOffersIt() {
        StreamMenu.Options options = options("rubylight-serverinfo.xml", NvApp.Role.STREAM);
        options.microphoneOffered = true;
        options.microphoneMuted = true;
        StreamMenu.Entry mic = StreamMenu.mainPage(options).get(3);
        assertEquals(MICROPHONE, mic.item);
        assertEquals(Boolean.FALSE, mic.checked);
        assertEquals(R.drawable.ic_mic_off, mic.icon);
        assertTrue(mic.keepsMenuOpen());
    }

    @Test
    public void deviceStateAddsAndRemovesItsOwnItems() {
        StreamMenu.Options options = options("rubylight-serverinfo.xml", NvApp.Role.STREAM);
        options.mouseConnected = true;
        options.onscreenControls = true;
        options.gameControls = true;
        options.keyboardKeysShown = true;
        options.scalingAvailable = true;
        options.zoomEnabled = true;
        options.pictureInPicture = true;
        options.externalDisplays = true;
        Map<StreamMenu.Section, List<StreamMenu.Item>> menu = sections(StreamMenu.mainPage(options));
        assertEquals(Arrays.asList(TOUCH_MODE, MOUSE_CURSOR, ONSCREEN_CONTROLS, CONTROLS_LAYOUT, GAME_CONTROLS, KEYBOARD_KEYS),
                menu.get(StreamMenu.Section.INPUT));
        assertEquals(Arrays.asList(VIDEO_SCALING, ZOOM, ZOOM_RESET, OVERLAY_PAGE, PICTURE_IN_PICTURE, VIDEO_OUTPUT),
                menu.get(StreamMenu.Section.DISPLAY));
        // Video on an external display: this device is a touchpad, so its own display items go.
        options.localOutput = false;
        menu = sections(StreamMenu.mainPage(options));
        assertFalse(menu.get(StreamMenu.Section.INPUT).contains(TOUCH_MODE));
        assertEquals(Arrays.asList(OVERLAY_PAGE, VIDEO_OUTPUT), menu.get(StreamMenu.Section.DISPLAY));
    }

    @Test
    public void aNativeHostGetsTheNativePageAndOthersDoNot() {
        assertTrue(items(StreamMenu.mainPage(options(nativeApiHost(), NvApp.Role.STREAM))).contains(NATIVE_PAGE));
        assertFalse(items(StreamMenu.mainPage(options(nativeApiHost(), NvApp.Role.INPUT_ONLY))).contains(NATIVE_PAGE));
        assertFalse(items(StreamMenu.mainPage(options("rubylight-serverinfo.xml", NvApp.Role.STREAM))).contains(NATIVE_PAGE));
    }

    @Test
    public void theNativePageOffersResumeDisconnectAndEndSessionAndClosesTheGameOnlyWithHostControl() {
        StreamMenu.Options options = options(nativeApiHost(), NvApp.Role.STREAM);
        options.nativeHostControl = true;
        assertEquals(Arrays.asList(NATIVE_RESUME, DISCONNECT, END_HOST_SESSION, CLOSE_GAME), items(StreamMenu.nativePage(options)));
        options.nativeHostControl = false;
        List<StreamMenu.Item> without = items(StreamMenu.nativePage(options));
        assertEquals(Arrays.asList(NATIVE_RESUME, DISCONNECT, END_HOST_SESSION, QUIT_GAME), without);
        assertFalse("Close game needs host control", without.contains(CLOSE_GAME));
    }

    @Test
    public void everyExistingActionIsStillReachable() {
        Set<StreamMenu.Item> reachable = EnumSet.noneOf(StreamMenu.Item.class);
        for (NvApp.Role role : NvApp.Role.values()) {
            // Quit app belongs to hosts without the native API; End session and Close game to those with it.
            StreamMenu.Options withControl = options(nativeApiHost(), role);
            withControl.nativeHostControl = true;
            reachable.addAll(items(StreamMenu.mainPage(withControl)));
            reachable.addAll(items(StreamMenu.nativePage(withControl)));
            StreamMenu.Options options = options("rubylight-serverinfo.xml", role);
            options.automaticQuality = true;
            options.microphoneOffered = options.mouseConnected = options.onscreenControls = options.gameControls = true;
            options.scalingAvailable = options.zoomEnabled = options.pictureInPicture = options.externalDisplays = true;
            reachable.addAll(items(StreamMenu.mainPage(options)));
            reachable.addAll(items(StreamMenu.overlayPage(options)));
        }
        assertEquals(EnumSet.allOf(StreamMenu.Item.class), reachable);
    }

    @Test
    public void theOverlayPageChoosesAStyleAndHoldsTheDiagnostics() {
        StreamMenu.Options options = options("rubylight-serverinfo.xml", NvApp.Role.STREAM);
        options.overlayMode = "statistics";
        List<StreamMenu.Entry> page = StreamMenu.overlayPage(options);
        assertEquals(Arrays.asList(OVERLAY_COMPACT, OVERLAY_STATISTICS, OVERLAY_DEBUG, OVERLAY_COPY, STREAM_HEALTH, LATENCY_TEST),
                items(page));
        assertEquals(Boolean.FALSE, page.get(0).checked);
        assertEquals(Boolean.TRUE, page.get(1).checked);
        assertEquals(Boolean.FALSE, page.get(2).checked);
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

    @Test @Config(qualifiers = "television")
    public void tvMenusKeepRemoteActionsAndLeaveOutTouchOnlyControls() {
        try (var controller = Robolectric.buildActivity(Activity.class).setup()) {
            Activity activity = controller.get();
            activity.setTheme(R.style.StreamTheme);
            StreamMenu.Options state = options("rubylight-serverinfo.xml", NvApp.Role.STREAM);
            state.onscreenControls = true;
            state.gameControls = true;
            StreamMenu menu = new StreamMenu(activity, () -> state, item -> { });
            menu.show();
            for (StreamMenu.Item item : new StreamMenu.Item[]{TOUCH_MODE, ONSCREEN_CONTROLS, CONTROLS_LAYOUT, GAME_CONTROLS, ZOOM}) {
                assertNull(menu.row(item));
                assertFalse(items(menu.entries()).contains(item));
            }
            assertEquals("System keyboard", menu.row(KEYBOARD).getText().toString());
            assertNotNull(menu.row(KEYBOARD_KEYS));
            assertNotNull(menu.row(DISCONNECT));
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
            assertEquals(activity.getString(R.string.desktop_controls_system_keyboard),
                    menu.row(KEYBOARD).getText().toString());
            assertFalse("The PC keyboard is an action too", menu.row(PC_KEYBOARD).isCheckable());
            assertEquals(activity.getString(R.string.pc_keyboard_title), menu.row(PC_KEYBOARD).getText().toString());
            assertEquals(activity.getString(R.string.desktop_controls_open),
                    menu.row(KEYBOARD_KEYS).getText().toString());
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
            menu.row(OVERLAY_PAGE).performClick();
            assertTrue("The page's first row takes the focus", menu.row(OVERLAY_COMPACT).isFocusedByDefault());
        }
    }

    private static void press(StreamMenu menu, int key) {
        menu.sheet().dispatchKeyEvent(new KeyEvent(KeyEvent.ACTION_DOWN, key));
        menu.sheet().dispatchKeyEvent(new KeyEvent(KeyEvent.ACTION_UP, key));
    }

    private static List<StreamMenu.Item> items(List<StreamMenu.Entry> entries) {
        List<StreamMenu.Item> items = new ArrayList<>();
        for (StreamMenu.Entry entry : entries) items.add(entry.item);
        return items;
    }
}
