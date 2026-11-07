package com.limelight.ui;

import android.app.Activity;
import android.app.Application;
import android.view.KeyEvent;
import android.view.View;

import com.google.android.material.button.MaterialButton;
import com.limelight.R;
import com.limelight.binding.input.KeyboardShortcuts;
import com.limelight.nativeapi.NativeApiFixtures;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.http.TestHosts;

import org.junit.After;
import org.junit.Before;
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
    @Before
    public void answerFromThePolicySources() {
        StreamMenuModel.use(new RustStreamMenuModel());
    }

    @After
    public void forgetTheModel() {
        StreamMenuModel.use(null);
    }

    /** The Rubylight fixture on a 3.0 host that also speaks the native API. */
    private static ComputerDetails nativeApiHost() {
        ComputerDetails host = TestHosts.details("rubylight-serverinfo.xml");
        host.caps = TestHosts.caps("<root status_code=\"200\"><RustHostVersion>3.0.0</RustHostVersion>"
                + "<PairStatus>1</PairStatus><NativeApiVersion>1</NativeApiVersion></root>");
        return host;
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

    @Test
    public void theFloatingMenuButtonIsASwitchOnTheMorePage() {
        StreamMenu.Options options = options("rubylight-serverinfo.xml", NvApp.Role.STREAM);
        assertFalse("A stream without the button has nothing to show", items(StreamMenu.mainPage(options)).contains(MENU_BUTTON));
        options.menuButtonAvailable = true;
        assertEquals(R.string.stream_menu_button_show, entry(StreamMenu.mainPage(options), MENU_BUTTON).label);
        options.menuButtonShown = true;
        assertEquals(R.string.stream_menu_button_hide, entry(StreamMenu.mainPage(options), MENU_BUTTON).label);
    }

    private static boolean inScroll(View view) {
        for (android.view.ViewParent parent = view.getParent(); parent != null; parent = parent.getParent()) {
            if (parent instanceof androidx.core.widget.NestedScrollView) return true;
        }
        return false;
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
    public void rubylightGetsTheQuickRowThenTheSessionActionsThenEverySection() {
        Map<StreamMenu.Section, List<StreamMenu.Item>> menu = sections(StreamMenu.mainPage(
                options("rubylight-serverinfo.xml", NvApp.Role.STREAM)));
        assertEquals(Arrays.asList(StreamMenu.Section.QUICK, StreamMenu.Section.SESSION, StreamMenu.Section.INPUT,
                StreamMenu.Section.DISPLAY, StreamMenu.Section.QUALITY, StreamMenu.Section.HOST), new ArrayList<>(menu.keySet()));
        assertEquals(Arrays.asList(KEYBOARD, PC_KEYBOARD, OVERLAY), menu.get(StreamMenu.Section.QUICK));
        assertEquals(Arrays.asList(TOUCH_MODE, ONSCREEN_CONTROLS, KEYBOARD_KEYS), menu.get(StreamMenu.Section.INPUT));
        assertEquals(Arrays.asList(ZOOM, OVERLAY_PAGE), menu.get(StreamMenu.Section.DISPLAY));
        // Resolution and frame rate (automatic quality) are Quality, not "Bitrate".
        assertEquals(Arrays.asList(BITRATE, AUTOMATIC_QUALITY, AUTOMATIC_BITRATE), menu.get(StreamMenu.Section.QUALITY));
        assertEquals(Arrays.asList(SERVER_COMMANDS, CLIPBOARD_SEND, CLIPBOARD_RECEIVE, HOST_WEB_UI, HOST_STATUS),
                menu.get(StreamMenu.Section.HOST));
        // Disconnect first (nightjar#52), Reconnect is a stream action, not a host command; quitting closes the
        // group, in the error colour.
        assertEquals(Arrays.asList(DISCONNECT, RECONNECT, QUIT_GAME), menu.get(StreamMenu.Section.SESSION));
        StreamMenu.Entry last = lastSession(StreamMenu.mainPage(options("rubylight-serverinfo.xml", NvApp.Role.STREAM)));
        assertTrue(last.destructive);
        assertEquals(R.string.stream_quit_game, last.label);
    }

    /**
     * nightjar#52: with a TV remote, Disconnect is one step down from the quick toggles in every role and on every
     * host, the whole session group comes before Input, Display, Quality and Host, and focus never starts on a row
     * that ends the stream.
     */
    @Test
    public void theSessionActionsSitRightUnderTheQuickTogglesWithDisconnectFirst() {
        List<StreamMenu.Options> menus = new ArrayList<>();
        for (NvApp.Role role : NvApp.Role.values()) {
            for (String fixture : new String[] {"gamestream-serverinfo.xml", "sunshine-serverinfo.xml", "apollo-serverinfo.xml",
                    "vibepollo-serverinfo.xml", "rubylight-serverinfo.xml", "butterpollo-viewer-serverinfo.xml"}) {
                menus.add(options(fixture, role));
            }
            StreamMenu.Options withControl = options(nativeApiHost(), role);
            withControl.nativeHostControl = true;
            withControl.hostMenu = true;
            menus.add(withControl);
            menus.add(options(nativeApiHost(), role));
        }
        for (StreamMenu.Options o : menus) {
            String name = o.role + " on " + o.host.name;
            List<StreamMenu.Entry> entries = StreamMenu.mainPage(o);
            int at = 0;
            while (entries.get(at).section == StreamMenu.Section.QUICK) at++;
            assertEquals(name, DISCONNECT, entries.get(at).item);
            while (at < entries.size() && entries.get(at).section == StreamMenu.Section.SESSION) at++;
            for (StreamMenu.Entry entry : entries.subList(at, entries.size())) {
                assertNotEquals(name + ": " + entry, StreamMenu.Section.SESSION, entry.section);
            }
            StreamMenu.Entry first = entry(entries, StreamMenu.firstFocus(entries));
            assertNotEquals(name, DISCONNECT, first.item);
            assertFalse(name, first.destructive);
        }
    }

    @Test
    public void focusStartsOnTheFirstToggleOrOnReconnectWhenThereAreNoToggles() {
        assertEquals(KEYBOARD, StreamMenu.firstFocus(StreamMenu.mainPage(options("rubylight-serverinfo.xml", NvApp.Role.STREAM))));
        // Input-only on a view-only device has no toggles: Disconnect is the first row, so focus skips it.
        List<StreamMenu.Entry> bare = StreamMenu.mainPage(options("butterpollo-viewer-serverinfo.xml", NvApp.Role.INPUT_ONLY));
        assertEquals(DISCONNECT, bare.get(0).item);
        assertEquals(RECONNECT, StreamMenu.firstFocus(bare));
        // The pages keep their first row.
        StreamMenu.Options native_ = options(nativeApiHost(), NvApp.Role.STREAM);
        assertEquals(NATIVE_RESUME, StreamMenu.firstFocus(StreamMenu.nativePage(native_)));
        assertEquals(OVERLAY_OFF, StreamMenu.firstFocus(StreamMenu.overlayPage(native_)));
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
    public void usbDevicesShowOnlyWhenForwardingIsOfferedAndOn() {
        StreamMenu.Options options = options("rubylight-serverinfo.xml", NvApp.Role.STREAM);
        assertFalse(items(StreamMenu.mainPage(options)).contains(USB_DEVICES));
        options.usbForwarding = true;
        assertTrue(sections(StreamMenu.mainPage(options)).get(StreamMenu.Section.HOST).contains(USB_DEVICES));
        // Watching only: nothing of this device goes to the PC.
        StreamMenu.Options monitor = options("rubylight-serverinfo.xml", NvApp.Role.REMOTE_MONITOR);
        monitor.usbForwarding = true;
        assertFalse(items(StreamMenu.mainPage(monitor)).contains(USB_DEVICES));
    }

    @Test
    public void imageClipboardRowsNeedTheHostFeatureAndTheTextClipboardPermissions() {
        StreamMenu.Options options = options("rubylight-serverinfo.xml", NvApp.Role.STREAM);
        options.imageClipboard = true;
        assertEquals(Arrays.asList(SERVER_COMMANDS, CLIPBOARD_SEND, CLIPBOARD_RECEIVE, IMAGE_CLIPBOARD_SEND,
                IMAGE_CLIPBOARD_RECEIVE, HOST_WEB_UI, HOST_STATUS), sections(StreamMenu.mainPage(options)).get(StreamMenu.Section.HOST));
        // Remote Monitor never writes to the PC; it reads images where it reads text.
        StreamMenu.Options monitor = options("rubylight-serverinfo.xml", NvApp.Role.REMOTE_MONITOR);
        monitor.imageClipboard = true;
        List<StreamMenu.Item> monitorItems = items(StreamMenu.mainPage(monitor));
        assertFalse(monitorItems.contains(IMAGE_CLIPBOARD_SEND));
        assertEquals(monitorItems.contains(CLIPBOARD_RECEIVE), monitorItems.contains(IMAGE_CLIPBOARD_RECEIVE));
        // Without the clipboard permissions the feature alone shows nothing.
        StreamMenu.Options sunshine = options("sunshine-serverinfo.xml", NvApp.Role.STREAM);
        sunshine.imageClipboard = true;
        List<StreamMenu.Item> sunshineItems = items(StreamMenu.mainPage(sunshine));
        assertFalse(sunshineItems.contains(IMAGE_CLIPBOARD_SEND));
        assertFalse(sunshineItems.contains(IMAGE_CLIPBOARD_RECEIVE));
    }

    @Test
    public void filesShowOnlyOnANativeApiHostInEveryRole() {
        for (NvApp.Role role : NvApp.Role.values()) {
            List<StreamMenu.Entry> entries = StreamMenu.mainPage(options(nativeApiHost(), role));
            assertTrue(role.name(), sections(entries).get(StreamMenu.Section.HOST).contains(FILES));
            for (StreamMenu.Entry entry : entries) {
                if (entry.item == FILES) assertEquals(R.string.files_menu, entry.label);
            }
        }
        for (String fixture : new String[] {"gamestream-serverinfo.xml", "sunshine-serverinfo.xml",
                "apollo-serverinfo.xml", "vibepollo-serverinfo.xml", "rubylight-serverinfo.xml"}) {
            assertFalse(fixture, items(StreamMenu.mainPage(options(fixture, NvApp.Role.STREAM))).contains(FILES));
        }
    }

    @Test
    public void everyRowHasItsOwnIcon() {
        StreamMenu.Options options = options("rubylight-serverinfo.xml", NvApp.Role.STREAM);
        options.imageClipboard = true;
        options.microphoneOffered = true;
        options.micButtonAvailable = true;
        options.mouseConnected = true;
        options.onscreenControls = true;
        options.gameControls = true;
        options.scalingAvailable = true;
        options.zoomEnabled = true;
        options.pictureInPicture = true;
        options.externalDisplays = true;
        options.usbForwarding = true;
        List<StreamMenu.Entry> entries = new ArrayList<>(StreamMenu.mainPage(options));
        // The overlay modes are one choice that marks the chosen mode with a check.
        for (StreamMenu.Entry entry : StreamMenu.overlayPage(options)) {
            if (entry.section != StreamMenu.Section.OVERLAY_STYLE && entry.section != StreamMenu.Section.OVERLAY_POSITION) entries.add(entry);
        }
        assertTrue(items(entries).containsAll(Arrays.asList(TOUCH_MODE, MOUSE_CURSOR, ONSCREEN_CONTROLS, CONTROLS_LAYOUT,
                MIC_BUTTON, GAME_CONTROLS, KEYBOARD_KEYS, VIDEO_SCALING, ZOOM, ZOOM_RESET, PICTURE_IN_PICTURE, VIDEO_OUTPUT,
                OVERLAY_COPY, STREAM_HEALTH, LATENCY_TEST, USB_DEVICES, IMAGE_CLIPBOARD_SEND, IMAGE_CLIPBOARD_RECEIVE)));
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
        StreamMenu.Entry last = lastSession(entries);
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
        StreamMenu.Entry last = lastSession(entries);
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
        assertEquals(END_SESSION, lastSession(monitor).item);
        assertEquals(R.string.stream_end_monitor, lastSession(monitor).label);
        assertEquals(R.string.stream_monitor_title, StreamMenu.title(NvApp.Role.REMOTE_MONITOR));

        List<StreamMenu.Entry> input = StreamMenu.mainPage(options("rubylight-serverinfo.xml", NvApp.Role.INPUT_ONLY));
        Map<StreamMenu.Section, List<StreamMenu.Item>> sections = sections(input);
        assertNull("Input-only has no video to show or tune", sections.get(StreamMenu.Section.DISPLAY));
        assertNull(sections.get(StreamMenu.Section.QUALITY));
        assertEquals(Arrays.asList(KEYBOARD, PC_KEYBOARD), sections.get(StreamMenu.Section.QUICK));
        assertEquals(R.string.stream_end_input, lastSession(input).label);
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
    public void theMicrophoneButtonToggleFollowsTheHostAndTheSetting() {
        StreamMenu.Options options = options("rubylight-serverinfo.xml", NvApp.Role.STREAM);
        assertFalse("No microphone, no button to show", items(StreamMenu.mainPage(options)).contains(MIC_BUTTON));
        options.microphoneOffered = true;
        assertFalse("A stream without the button has nothing to show", items(StreamMenu.mainPage(options)).contains(MIC_BUTTON));
        options.micButtonAvailable = true;
        options.micButtonShown = true;
        StreamMenu.Entry hide = entry(StreamMenu.mainPage(options), MIC_BUTTON);
        assertEquals(StreamMenu.Section.INPUT, hide.section);
        assertEquals(R.string.stream_mic_button_hide, hide.label);
        assertFalse("Showing or hiding the button closes the menu", hide.keepsMenuOpen());
        options.micButtonShown = false;
        assertEquals(R.string.stream_mic_button_show, entry(StreamMenu.mainPage(options), MIC_BUTTON).label);
        // It is a touch control: nothing to tap without input.
        options.canSendInput = false;
        assertFalse(items(StreamMenu.mainPage(options)).contains(MIC_BUTTON));
        assertTrue(items(StreamMenu.mainPage(options)).contains(MICROPHONE));
    }

    /** The last of the session actions, which now sit under the quick toggles rather than at the end. */
    private static StreamMenu.Entry lastSession(List<StreamMenu.Entry> entries) {
        StreamMenu.Entry last = null;
        for (StreamMenu.Entry entry : entries) {
            if (entry.section == StreamMenu.Section.SESSION) last = entry;
        }
        assertNotNull("the page has session actions", last);
        return last;
    }

    private static StreamMenu.Entry entry(List<StreamMenu.Entry> entries, StreamMenu.Item item) {
        for (StreamMenu.Entry entry : entries) {
            if (entry.item == item) return entry;
        }
        throw new AssertionError(item + " is missing");
    }

    @Test
    public void aConnectedControllerGetsTheControllerMouseSwitch() {
        StreamMenu.Options options = options("rubylight-serverinfo.xml", NvApp.Role.STREAM);
        assertFalse("No controller, no switch", items(StreamMenu.mainPage(options)).contains(CONTROLLER_MOUSE));
        options.controllerMouseAvailable = true;
        assertEquals(Arrays.asList(TOUCH_MODE, CONTROLLER_MOUSE, ONSCREEN_CONTROLS, KEYBOARD_KEYS),
                sections(StreamMenu.mainPage(options)).get(StreamMenu.Section.INPUT));
        assertEquals(Boolean.FALSE, entry(StreamMenu.mainPage(options), CONTROLLER_MOUSE).checked);
        options.controllerMouseOn = true;
        assertEquals(Boolean.TRUE, entry(StreamMenu.mainPage(options), CONTROLLER_MOUSE).checked);
        options.canSendInput = false;
        assertFalse("Watching only: nothing to steer", items(StreamMenu.mainPage(options)).contains(CONTROLLER_MOUSE));
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
    public void theHostMenuItemFollowsTheCapabilityAndOnlyForTheStreamRole() {
        for (NvApp.Role role : NvApp.Role.values()) {
            StreamMenu.Options options = options(nativeApiHost(), role);
            assertFalse(role.name(), items(StreamMenu.mainPage(options)).contains(HOST_MENU));
            options.hostMenu = true;
            Map<StreamMenu.Section, List<StreamMenu.Item>> menu = sections(StreamMenu.mainPage(options));
            boolean shown = menu.containsKey(StreamMenu.Section.HOST) && menu.get(StreamMenu.Section.HOST).contains(HOST_MENU);
            assertEquals(role.name(), role == NvApp.Role.STREAM, shown);
        }
        // Whatever the host is called: the capability decides, not the brand.
        StreamMenu.Options other = options("sunshine-serverinfo.xml", NvApp.Role.STREAM);
        other.hostMenu = true;
        assertTrue(items(StreamMenu.mainPage(other)).contains(HOST_MENU));
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
            withControl.hostMenu = true;
            withControl.usbForwarding = true;
            withControl.imageClipboard = true;
            withControl.nightModeOffered = withControl.headphoneSurroundOffered = true;
            try {
                withControl.hostAudio = NativeApiFixtures.audio(NativeApiFixtures.audioJson(true, false));
            } catch (org.json.JSONException e) {
                throw new AssertionError(e);
            }
            reachable.addAll(items(StreamMenu.mainPage(withControl)));
            reachable.addAll(items(StreamMenu.nativePage(withControl)));
            StreamMenu.Options options = options("rubylight-serverinfo.xml", role);
            options.automaticQuality = true;
            options.microphoneOffered = options.micButtonAvailable = options.mouseConnected = options.onscreenControls = options.gameControls = true;
            options.controllerMouseAvailable = options.menuButtonAvailable = true;
            options.scalingAvailable = options.zoomEnabled = options.pictureInPicture = options.externalDisplays = true;
            reachable.addAll(items(StreamMenu.mainPage(options)));
            reachable.addAll(items(StreamMenu.overlayPage(options)));
            options.rateChange = true;
            reachable.addAll(items(StreamMenu.mainPage(options)));
        }
        // The menu adds the Shortcuts and More tiles and Return to stream itself (see the grid test).
        reachable.addAll(EnumSet.of(SHORTCUTS, MORE, CLOSE));
        assertEquals(EnumSet.allOf(StreamMenu.Item.class), reachable);
    }

    @Test
    public void rateChangesReplaceTheBitrateRowWhenTheHostOffersThem() {
        StreamMenu.Options options = options("rubylight-serverinfo.xml", NvApp.Role.STREAM);
        options.rateChange = true;
        assertEquals(Arrays.asList(RATE_CHANGE, AUTOMATIC_QUALITY, AUTOMATIC_BITRATE),
                sections(StreamMenu.mainPage(options)).get(StreamMenu.Section.QUALITY));

        // A host without the bitrate endpoint still gets automatic bitrate when it runs on this device.
        StreamMenu.Options plain = options("sunshine-serverinfo.xml", NvApp.Role.STREAM);
        plain.rateChange = true;
        assertFalse(items(StreamMenu.mainPage(plain)).contains(AUTOMATIC_BITRATE));
        plain.rateChangeBitrate = true;
        List<StreamMenu.Item> items = items(StreamMenu.mainPage(plain));
        assertTrue(items.contains(RATE_CHANGE));
        assertTrue(items.contains(AUTOMATIC_BITRATE));
        assertFalse(items.contains(BITRATE));

        // Only the stream that sends video changes its rates.
        for (NvApp.Role role : NvApp.Role.values()) {
            if (role == NvApp.Role.STREAM) continue;
            StreamMenu.Options other = options("rubylight-serverinfo.xml", role);
            other.rateChange = true;
            assertFalse(role.name(), items(StreamMenu.mainPage(other)).contains(RATE_CHANGE));
        }
    }

    @Test
    public void theOverlayPageChoosesAStyleAndHoldsTheDiagnostics() {
        StreamMenu.Options options = options("rubylight-serverinfo.xml", NvApp.Role.STREAM);
        options.overlayShown = true;
        options.overlayMode = "statistics";
        List<StreamMenu.Entry> page = StreamMenu.overlayPage(options);
        assertEquals(Arrays.asList(OVERLAY_OFF, OVERLAY_COMPACT, OVERLAY_STATISTICS, OVERLAY_DEBUG,
                        OVERLAY_TOP_LEFT, OVERLAY_TOP_RIGHT, OVERLAY_BOTTOM_LEFT, OVERLAY_BOTTOM_RIGHT,
                        OVERLAY_COPY, STREAM_HEALTH, LATENCY_TEST),
                items(page));
        assertEquals("Off comes first", StreamMenu.Section.OVERLAY_STYLE, page.get(0).section);
        assertEquals(Boolean.FALSE, page.get(0).checked);
        assertEquals(Boolean.FALSE, page.get(1).checked);
        assertEquals(Boolean.TRUE, page.get(2).checked);
        assertEquals(Boolean.FALSE, page.get(3).checked);
    }

    @Test
    public void offIsCheckedWhileTheOverlayIsHiddenAndNoModeIs() {
        StreamMenu.Options options = options("rubylight-serverinfo.xml", NvApp.Role.STREAM);
        options.overlayShown = false;
        options.overlayMode = "debug";
        List<StreamMenu.Entry> page = StreamMenu.overlayPage(options);
        assertEquals(Boolean.TRUE, page.get(0).checked);
        for (int i = 1; i <= 3; i++) assertEquals(page.get(i).toString(), Boolean.FALSE, page.get(i).checked);
    }

    @Test
    public void theOverlayPageOffersFourCornersWithTheCurrentOneChecked() {
        StreamMenu.Options options = options("rubylight-serverinfo.xml", NvApp.Role.STREAM);
        StreamMenu.Item[] corners = {OVERLAY_TOP_LEFT, OVERLAY_TOP_RIGHT, OVERLAY_BOTTOM_LEFT, OVERLAY_BOTTOM_RIGHT};
        String[] keys = {"top_left", "top_right", "bottom_left", "bottom_right"};
        for (int chosen = 0; chosen < keys.length; chosen++) {
            options.overlayPosition = keys[chosen];
            int found = 0;
            for (StreamMenu.Entry entry : StreamMenu.overlayPage(options)) {
                if (entry.section != StreamMenu.Section.OVERLAY_POSITION) continue;
                assertEquals(corners[found], entry.item);
                assertEquals(entry.toString(), found == chosen, Boolean.TRUE.equals(entry.checked));
                found++;
            }
            assertEquals(4, found);
        }
        options.overlayPosition = "nonsense";
        assertEquals("An unknown position reads as top left", Boolean.TRUE, StreamMenu.overlayPage(options).get(4).checked);
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
            assertEquals("Open host menu", menu.row(HOST_MENU).getText().toString());
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
    public void theHostsAudioSwitchesShowOnlyWhenOfferedAndChangeableNow() throws Exception {
        StreamMenu.Options options = options(nativeApiHost(), NvApp.Role.STREAM);
        options.nightModeOffered = true;
        options.headphoneSurroundOffered = true;
        // Until the host answers there is nothing to show.
        assertFalse(items(StreamMenu.mainPage(options)).contains(NIGHT_MODE));
        options.hostAudio = NativeApiFixtures.audio(NativeApiFixtures.audioJson(true, false));
        List<StreamMenu.Entry> entries = StreamMenu.mainPage(options);
        List<StreamMenu.Item> quick = sections(entries).get(StreamMenu.Section.QUICK);
        assertEquals(Arrays.asList(NIGHT_MODE, HEADPHONE_SURROUND), quick.subList(quick.size() - 2, quick.size()));
        for (StreamMenu.Entry entry : entries) {
            if (entry.item == NIGHT_MODE) assertEquals("the host's state", Boolean.TRUE, entry.checked);
            if (entry.item == HEADPHONE_SURROUND) assertEquals(Boolean.FALSE, entry.checked);
            if (entry.item == NIGHT_MODE || entry.item == HEADPHONE_SURROUND) assertTrue(entry.keepsMenuOpen());
        }

        // Not offered (the host lacks the feature or this device the permission): no switch, whatever it answered.
        options.headphoneSurroundOffered = false;
        assertFalse(items(StreamMenu.mainPage(options)).contains(HEADPHONE_SURROUND));
        assertTrue(items(StreamMenu.mainPage(options)).contains(NIGHT_MODE));
        // An answer without the setting leaves it out too.
        options.headphoneSurroundOffered = true;
        options.hostAudio = NativeApiFixtures.audio(NativeApiFixtures.audioJson(false, null));
        assertFalse(items(StreamMenu.mainPage(options)).contains(HEADPHONE_SURROUND));
        // Without a stream the host can't change either.
        options.hostAudio = NativeApiFixtures.audio(NativeApiFixtures.AUDIO_WITHOUT_STREAM);
        assertFalse(items(StreamMenu.mainPage(options)).contains(NIGHT_MODE));
        assertFalse(items(StreamMenu.mainPage(options)).contains(HEADPHONE_SURROUND));
        // Remote Monitor changes nothing on the PC.
        StreamMenu.Options monitor = options(nativeApiHost(), NvApp.Role.REMOTE_MONITOR);
        monitor.nightModeOffered = true;
        monitor.headphoneSurroundOffered = true;
        monitor.hostAudio = NativeApiFixtures.audio(NativeApiFixtures.audioJson(true, true));
        assertFalse(items(StreamMenu.mainPage(monitor)).contains(NIGHT_MODE));
        assertFalse(items(StreamMenu.mainPage(monitor)).contains(HEADPHONE_SURROUND));
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

    private static void press(StreamMenu menu, int key) {
        menu.sheet().dispatchKeyEvent(new KeyEvent(KeyEvent.ACTION_DOWN, key));
        menu.sheet().dispatchKeyEvent(new KeyEvent(KeyEvent.ACTION_UP, key));
    }

    private static List<StreamMenu.Item> items(List<StreamMenu.Entry> entries) {
        List<StreamMenu.Item> items = new ArrayList<>();
        for (StreamMenu.Entry entry : entries) items.add(entry.item);
        return items;
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

    /** The user's shortcuts in memory, and what the menu sent. */
    private static final class SavedShortcuts implements StreamMenu.Shortcuts {
        final List<KeyboardShortcuts.Shortcut> list = new ArrayList<>();
        final List<int[]> sent = new ArrayList<>();

        SavedShortcuts add(String name, String chord) {
            list.add(new KeyboardShortcuts.Shortcut("s" + list.size(), name, KeyboardShortcuts.parseChord(chord)));
            return this;
        }

        @Override public List<KeyboardShortcuts.Shortcut> saved() {
            return new ArrayList<>(list);
        }

        @Override public void save(List<KeyboardShortcuts.Shortcut> shortcuts) {
            list.clear();
            list.addAll(shortcuts);
        }

        @Override public void send(int[] keys) {
            sent.add(keys);
        }
    }

    private static String text(View tile) {
        return ((MaterialButton) tile).getText().toString();
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
            // X is a way out under the grid.
            press(menu, KeyEvent.KEYCODE_BUTTON_X);
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
            assertEquals(layout.grid.size(), grid.getChildCount());
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
            // Return to stream under the grid, then Disconnect, in the panel's footer outside the part that scrolls.
            assertTrue(menu.sheet().isPanel());
            assertEquals(activity.getString(R.string.stream_menu_return), text(menu.row(CLOSE)));
            android.view.ViewGroup footer = (android.view.ViewGroup) menu.row(CLOSE).getParent();
            assertTrue(footer instanceof ActionSheet.FooterBar);
            assertSame(footer, menu.row(DISCONNECT).getParent());
            assertTrue(footer.indexOfChild(menu.row(CLOSE)) < footer.indexOfChild(menu.row(DISCONNECT)));
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
            // Return to stream closes without choosing anything.
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
        // The ways out sit in the footer.
        for (RustStreamMenuModel.Tile tile : model.tiles) {
            if (tile.id.equals("disconnect") || tile.id.endsWith("_game") || tile.id.startsWith("end_")) {
                assertEquals(tile.id, "footer", tile.place);
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

    // A button whose text starts with {@code start}; {@code outsideGrid} skips the tiles.
    private static MaterialButton buttonStartingWith(View view, String start, boolean outsideGrid) {
        if (outsideGrid && view instanceof TileGrid) return null;
        if (view instanceof MaterialButton && text(view).startsWith(start)) return (MaterialButton) view;
        if (view instanceof android.view.ViewGroup) {
            for (int i = 0; i < ((android.view.ViewGroup) view).getChildCount(); i++) {
                MaterialButton found = buttonStartingWith(((android.view.ViewGroup) view).getChildAt(i), start, outsideGrid);
                if (found != null) return found;
            }
        }
        return null;
    }

    private static boolean textShown(View view, String text) {
        if (view instanceof android.widget.TextView && view.getVisibility() == View.VISIBLE
                && ((android.widget.TextView) view).getText().toString().equals(text)) return true;
        if (view instanceof android.view.ViewGroup) {
            for (int i = 0; i < ((android.view.ViewGroup) view).getChildCount(); i++) {
                if (textShown(((android.view.ViewGroup) view).getChildAt(i), text)) return true;
            }
        }
        return false;
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

    private static TileGrid findGrid(View view) {
        if (view instanceof TileGrid) return (TileGrid) view;
        if (view instanceof android.view.ViewGroup) {
            for (int i = 0; i < ((android.view.ViewGroup) view).getChildCount(); i++) {
                TileGrid found = findGrid(((android.view.ViewGroup) view).getChildAt(i));
                if (found != null) return found;
            }
        }
        return null;
    }

    private static MaterialButton capNamed(TileGrid caps, String label) {
        for (int i = 0; i < caps.getChildCount(); i++) {
            if (text(caps.getChildAt(i)).equals(label)) return (MaterialButton) caps.getChildAt(i);
        }
        throw new AssertionError("no key " + label);
    }
}
