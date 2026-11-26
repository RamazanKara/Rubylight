package com.limelight.ui;

import android.app.Application;

import com.limelight.R;
import com.limelight.nativeapi.NativeApiFixtures;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvApp;

import org.junit.Test;
import org.junit.runner.RunWith;
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
import static com.limelight.ui.StreamMenuTestSupport.*;
import static org.junit.Assert.*;

/**
 * Entry catalog, capability gating, session ordering, overlay page, and host-audio entry tests for
 * {@link StreamMenu}.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class StreamMenuTest {
    @Test
    public void theFloatingMenuButtonIsASwitchOnTheMorePage() {
        StreamMenu.Options options = options("rubylight-serverinfo.xml", NvApp.Role.STREAM);
        assertFalse("A stream without the button has nothing to show", items(StreamMenu.mainPage(options)).contains(MENU_BUTTON));
        options.menuButtonAvailable = true;
        assertEquals(R.string.stream_menu_button_show, entry(StreamMenu.mainPage(options), MENU_BUTTON).label);
        options.menuButtonShown = true;
        assertEquals(R.string.stream_menu_button_hide, entry(StreamMenu.mainPage(options), MENU_BUTTON).label);
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
    public void guestsShowOnlyToTheDeviceThatManagesThem() {
        StreamMenu.Options options = options("rubylight-serverinfo.xml", NvApp.Role.STREAM);
        assertFalse(items(StreamMenu.mainPage(options)).contains(StreamMenu.Item.GUESTS));
        options.sessionGuests = true;
        assertTrue(sections(StreamMenu.mainPage(options)).get(StreamMenu.Section.HOST).contains(StreamMenu.Item.GUESTS));
        StreamMenu.Options monitor = options("rubylight-serverinfo.xml", NvApp.Role.REMOTE_MONITOR);
        monitor.sessionGuests = true;
        assertFalse(items(StreamMenu.mainPage(monitor)).contains(StreamMenu.Item.GUESTS));
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
        options.picturePositionAvailable = true;
        options.flipAvailable = true;
        options.zoomEnabled = true;
        options.pictureInPicture = true;
        options.externalDisplays = true;
        options.usbForwarding = true;
        options.sessionGuests = true;
        List<StreamMenu.Entry> entries = new ArrayList<>(StreamMenu.mainPage(options));
        // The overlay modes are one choice that marks the chosen mode with a check.
        for (StreamMenu.Entry entry : StreamMenu.overlayPage(options)) {
            if (entry.section != StreamMenu.Section.OVERLAY_STYLE && entry.section != StreamMenu.Section.OVERLAY_POSITION) entries.add(entry);
        }
        assertTrue(items(entries).containsAll(Arrays.asList(TOUCH_MODE, MOUSE_CURSOR, ONSCREEN_CONTROLS, CONTROLS_LAYOUT,
                MIC_BUTTON, GAME_CONTROLS, KEYBOARD_KEYS, VIDEO_SCALING, PICTURE_POSITION, FLIP_SCREEN, ZOOM, ZOOM_RESET, PICTURE_IN_PICTURE, VIDEO_OUTPUT,
                OVERLAY_COPY, STREAM_HEALTH, LATENCY_TEST, USB_DEVICES, IMAGE_CLIPBOARD_SEND, IMAGE_CLIPBOARD_RECEIVE)));
        Map<Integer, StreamMenu.Item> seen = new LinkedHashMap<>();
        for (StreamMenu.Entry entry : entries) {
            StreamMenu.Item before = seen.put(entry.icon, entry.item);
            assertNull(entry.item + " has the same icon as " + before, before);
        }
    }

    @Test
    public void picturePositionAndFlipShowOnlyWhereTheyApplyOnThisScreen() {
        StreamMenu.Options options = options("rubylight-serverinfo.xml", NvApp.Role.STREAM);
        assertFalse(items(StreamMenu.mainPage(options)).contains(PICTURE_POSITION));
        assertFalse(items(StreamMenu.mainPage(options)).contains(FLIP_SCREEN));
        options.picturePositionAvailable = options.flipAvailable = true;
        assertTrue(items(StreamMenu.mainPage(options)).containsAll(Arrays.asList(PICTURE_POSITION, FLIP_SCREEN)));
        // Video on another display: this phone is the touchpad, there is no picture here to move or turn.
        options.localOutput = false;
        assertFalse(items(StreamMenu.mainPage(options)).contains(PICTURE_POSITION));
        assertFalse(items(StreamMenu.mainPage(options)).contains(FLIP_SCREEN));
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
            withControl.sessionGuests = true;
            withControl.nightModeOffered = withControl.headphoneSurroundOffered = true;
            withControl.hostAudio = NativeApiFixtures.audio(NativeApiFixtures.audioJson(true, false));
            reachable.addAll(items(StreamMenu.mainPage(withControl)));
            reachable.addAll(items(StreamMenu.nativePage(withControl)));
            StreamMenu.Options options = options("rubylight-serverinfo.xml", role);
            options.automaticQuality = true;
            options.microphoneOffered = options.micButtonAvailable = options.mouseConnected = options.onscreenControls = options.gameControls = true;
            options.controllerMouseAvailable = options.menuButtonAvailable = true;
            options.scalingAvailable = options.zoomEnabled = options.pictureInPicture = options.externalDisplays = true;
            options.picturePositionAvailable = options.flipAvailable = true;
            reachable.addAll(items(StreamMenu.mainPage(options)));
            reachable.addAll(items(StreamMenu.overlayPage(options)));
            options.rateChange = true;
            reachable.addAll(items(StreamMenu.mainPage(options)));
        }
        // The menu adds the Shortcuts and More tiles and Return to stream itself (see the grid test), and asks before
        // Disconnect run from a controller button (see the buttons test).
        reachable.addAll(EnumSet.of(SHORTCUTS, MORE, CLOSE, StreamMenu.Item.CONFIRM_DISCONNECT));
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

    /** T9-18: non-QUICK boolean switches on Page.MORE keep their feature icon visible when OFF and refresh on toggle. */
    @Test
    public void morePageBooleanSwitchKeepsFeatureIconVisibleAndRefreshesState() {
        try (var controller = org.robolectric.Robolectric.buildActivity(StreamGlassTest.Stream.class).setup().visible()) {
            android.app.Activity activity = controller.get();
            activity.setTheme(com.limelight.R.style.StreamTheme);
            StreamMenu.Options opts = options("rubylight-serverinfo.xml", NvApp.Role.STREAM);
            opts.rateChange = true;
            opts.rateChangeBitrate = true;
            opts.automaticBitrateOn = false;
            StreamMenu menu = new StreamMenu(activity, () -> opts, item -> {
                if (item == AUTOMATIC_BITRATE) opts.automaticBitrateOn = !opts.automaticBitrateOn;
            });
            menu.show();
            com.google.android.material.button.MaterialButton more = menu.row(MORE);
            if (more != null) more.performClick();
            com.google.android.material.button.MaterialButton autoBitrate = menu.row(AUTOMATIC_BITRATE);
            assertNotNull("Automatic bitrate row shown on More", autoBitrate);
            assertFalse("starts unselected", autoBitrate.isSelected());
            android.content.res.ColorStateList tint = autoBitrate.getIconTint();
            assertNotEquals("feature icon must not be tinted transparent when OFF",
                    android.graphics.Color.TRANSPARENT, tint == null ? -1 : tint.getDefaultColor());
            opts.automaticBitrateOn = true;
            menu.refresh();
            assertTrue("refresh() updates Page.MORE switch to ON in place", autoBitrate.isSelected());
            assertEquals(activity.getString(com.limelight.R.string.stream_choice_selected),
                    androidx.core.view.ViewCompat.getStateDescription(autoBitrate));
            opts.automaticBitrateOn = false;
            menu.refresh();
            assertFalse("refresh() updates Page.MORE switch back to OFF in place", autoBitrate.isSelected());
            assertEquals(activity.getString(com.limelight.R.string.stream_choice_not_selected),
                    androidx.core.view.ViewCompat.getStateDescription(autoBitrate));
            android.app.Dialog shown = org.robolectric.shadows.ShadowDialog.getLatestDialog();
            if (shown != null) shown.dismiss();
        }
    }
}
