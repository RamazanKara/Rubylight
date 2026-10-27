package com.limelight.binding.input;

import android.app.Application;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;
import java.io.IOException;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collections;
import java.util.List;
import static org.junit.Assert.*;

/** Artemis samples and their origin: src/test/resources/artemis/README.txt. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class TouchLayoutImportTest {
    private static String artemis(String name) throws IOException {
        return TouchLayoutFixturesTest.fixture("../artemis/" + name);
    }

    private static TouchLayout.Button find(TouchLayout layout, String id) {
        for (TouchLayout.Button button : layout.buttons) if (button.id.equals(id)) return button;
        throw new AssertionError("Missing " + id);
    }

    private static List<String> skipped(TouchLayoutImport.Result result) {
        List<String> out = new ArrayList<>();
        for (TouchLayoutImport.Skipped skipped : result.skipped) out.add(skipped.name + " " + skipped.reason);
        return out;
    }

    @Test public void theWikiCustomKeysSampleBecomesTwoChordButtons() throws IOException {
        String file = artemis("wiki-custom-special-keys.json");
        assertEquals(TouchLayoutImport.Format.ARTEMIS_CUSTOM_KEYS, TouchLayoutImport.detect(file));
        TouchLayoutImport.Result result = TouchLayoutImport.importFiles(Collections.singletonList(file), 2400, 1080);
        assertEquals(2, result.layout.buttons.size());
        TouchLayout.Button copy = result.layout.buttons.get(0), paste = result.layout.buttons.get(1);
        assertEquals("COPY", copy.name);
        assertEquals(TouchLayout.Kind.KEYBOARD, copy.kind);
        assertArrayEquals(new int[] {android.view.KeyEvent.KEYCODE_CTRL_LEFT, android.view.KeyEvent.KEYCODE_C}, copy.keys());
        assertArrayEquals(new int[] {android.view.KeyEvent.KEYCODE_CTRL_LEFT, android.view.KeyEvent.KEYCODE_V}, paste.keys());
        assertFalse(copy.toggle);
        // Square keys in a row, as Artemis lays them out: 10 of 72 units of the screen height.
        assertEquals(10 / 72f, copy.height, 1e-6);
        assertEquals(10 / 72f * 1080 / 2400, copy.width, 1e-6);
        assertTrue(paste.x > copy.x + copy.width - 1e-6);
        assertTrue(result.skipped.isEmpty());
        assertEquals(result.layout.encode(), TouchLayout.parse(result.layout.encode()).encode());
    }

    @Test public void mouseVirtualKeysFromIssue521BecomeMouseButtons() throws IOException {
        TouchLayoutImport.Result result = TouchLayoutImport.importFiles(
                Collections.singletonList(artemis("issue-521-mouse-keys.json")), 2400, 1080);
        assertEquals(Arrays.asList(1, 3, 2), Arrays.asList(result.layout.buttons.get(0).mouseButton,
                result.layout.buttons.get(1).mouseButton, result.layout.buttons.get(2).mouseButton));
        assertEquals(TouchLayout.Kind.MOUSE, result.layout.buttons.get(0).kind);
    }

    @Test public void keysNightjarCannotSendAreListedNotDropped() {
        String file = "{\"data\":[{\"id\":\"media\",\"name\":\"Play\",\"keys\":[\"VK_ATTN\"]}," +
                "{\"id\":\"mixed\",\"name\":\"Ctrl click\",\"keys\":[\"VK_LCONTROL\",\"VK_LBUTTON\"]}," +
                "{\"id\":\"typo\",\"name\":\"Typo\",\"keys\":[\"VK_NOPE\"]}," +
                "{\"id\":\"hex\",\"name\":\"Alt F4\",\"keys\":[\"0xA4\",\"0x73\"],\"sticky\":true}," +
                "{\"name\":\"Shift\",\"keys\":[\"VK_SHIFT\"]}]}";
        TouchLayoutImport.Result result = TouchLayoutImport.importFiles(Collections.singletonList(file), 1600, 900);
        assertEquals(Arrays.asList("Play UNSUPPORTED_KEY", "Ctrl click UNSUPPORTED_KEY", "Typo UNSUPPORTED_KEY"),
                skipped(result));
        TouchLayout.Button altF4 = result.layout.buttons.get(0);
        assertArrayEquals(new int[] {android.view.KeyEvent.KEYCODE_ALT_LEFT, android.view.KeyEvent.KEYCODE_F4}, altF4.keys());
        assertTrue("Sticky custom keys toggle", altF4.toggle);
        // The generic Shift becomes left Shift; an entry without an ID takes its position, like Artemis.
        TouchLayout.Button shift = result.layout.buttons.get(1);
        assertArrayEquals(new int[] {android.view.KeyEvent.KEYCODE_SHIFT_LEFT}, shift.keys());
        assertEquals("custom_4", shift.id);
    }

    @Test public void anExportedKeyboardLayoutMapsWhatHasAnEquivalentAndReportsTheRest() throws IOException {
        String layoutFile = artemis("derived-keyboard-layout-2400x1080.json");
        assertEquals(TouchLayoutImport.Format.ARTEMIS_LAYOUT, TouchLayoutImport.detect(layoutFile));
        TouchLayoutImport.Result result = TouchLayoutImport.importFiles(Arrays.asList(layoutFile,
                artemis("wiki-custom-special-keys.json")), 2400, 1080);
        assertEquals(TouchLayoutImport.Format.ARTEMIS_LAYOUT, result.format);
        assertFalse(result.scaled);
        // dpad_1 and m_2 are hidden, key_29 and key_51 turned off.
        assertEquals(4, result.hidden);
        assertEquals(Arrays.asList("m_9 NO_EQUIVALENT", "screenshot MISSING_CUSTOM_KEY", "rocker_1 STICK_CENTER_KEY"),
                skipped(result));
        TouchLayout layout = result.layout;
        assertEquals(10, layout.buttons.size());
        TouchLayout.Button esc = find(layout, "key_111");
        assertEquals(TouchLayout.Kind.KEYBOARD, esc.kind);
        assertArrayEquals(new int[] {android.view.KeyEvent.KEYCODE_ESCAPE}, esc.keys());
        assertEquals(15 / 2400f, esc.x, 1e-6);
        assertEquals(120 / 1080f, esc.y, 1e-6);
        assertEquals(120 / 2400f, esc.width, 1e-6);
        assertEquals(120 / 1080f, esc.height, 1e-6);
        TouchLayout.Button stick = find(layout, "rocker_1");
        assertEquals(TouchLayout.Kind.WASD, stick.kind);
        assertArrayEquals(new int[] {51, 29, 47, 32}, stick.keys());
        assertFalse(stick.floating);
        assertEquals(1, find(layout, "m_1").mouseButton);
        assertFalse(find(layout, "m_1").toggle);
        assertEquals(3, find(layout, "m_s_3").mouseButton);
        assertTrue(find(layout, "m_s_3").toggle);
        TouchLayout.Button copy = find(layout, "custom_copy");
        assertEquals("COPY", copy.name);
        assertEquals(2040 / 2400f, copy.x, 1e-6);
        assertArrayEquals(new int[] {android.view.KeyEvent.KEYCODE_CTRL_LEFT, android.view.KeyEvent.KEYCODE_C}, copy.keys());
        // Reading order: top row first, left to right.
        assertEquals("key_111", layout.buttons.get(0).id);
        assertEquals("custom_copy", layout.buttons.get(1).id);
        assertEquals("custom_paste", layout.buttons.get(2).id);
        assertEquals(layout.encode(), TouchLayout.parse(layout.encode()).encode());
    }

    @Test public void customKeysInALayoutNeedTheirCustomKeysFile() throws IOException {
        TouchLayoutImport.Result result = TouchLayoutImport.importFiles(
                Collections.singletonList(artemis("derived-keyboard-layout-2400x1080.json")), 2400, 1080);
        assertTrue(skipped(result).containsAll(Arrays.asList("copy MISSING_CUSTOM_KEY", "paste MISSING_CUSTOM_KEY")));
        assertEquals(8, result.layout.buttons.size());
    }

    @Test public void aLayoutFromALargerScreenIsScaledToFit() throws IOException {
        TouchLayoutImport.Result result = TouchLayoutImport.importFiles(
                Collections.singletonList(artemis("derived-keyboard-layout-2400x1080.json")), 1600, 720);
        assertTrue(result.scaled);
        // The rightmost element (m_1, 2160 + 120 px) ends at the right edge.
        TouchLayout.Button mouse = find(result.layout, "m_1");
        assertEquals(1, mouse.x + mouse.width, 1e-4);
        for (TouchLayout.Button button : result.layout.buttons) {
            assertTrue(button.x + button.width <= 1.00001f && button.y + button.height <= 1.00001f);
        }
    }

    @Test public void aNightjarLayoutImportsAsItIs() throws IOException {
        String file = TouchLayoutFixturesTest.fixture("v2-all-controls.json");
        TouchLayoutImport.Result result = TouchLayoutImport.importFiles(Collections.singletonList(file), 2400, 1080);
        assertEquals(TouchLayoutImport.Format.NIGHTJAR, result.format);
        assertEquals(file, result.layout.encode());
    }

    @Test public void aNightjarLayoutNeedsNoSurfaceSize() throws IOException {
        // Hidden controls have not been measured yet; a Nightjar layout is already normalized.
        String file = TouchLayoutFixturesTest.fixture("v2-all-controls.json");
        TouchLayoutImport.Result result = TouchLayoutImport.importFiles(Collections.singletonList(file), 0, 0);
        assertEquals(TouchLayoutImport.Format.NIGHTJAR, result.format);
        assertEquals(file, result.layout.encode());
    }

    private static String customKeysFile(int count) {
        StringBuilder file = new StringBuilder("{\"data\":[");
        for (int i = 0; i < count; i++) {
            String key = i < 26 ? String.valueOf((char) ('A' + i)) : String.valueOf((char) ('0' + i - 26));
            file.append(i == 0 ? "" : ",").append("{\"id\":\"k").append(i).append("\",\"name\":\"").append(key)
                    .append("\",\"keys\":[\"VK_").append(key).append("\"]}");
        }
        return file.append("]}").toString();
    }

    @Test public void customKeysFitTheSurfaceWithoutOverlapping() {
        String file = customKeysFile(TouchLayout.MAX_BUTTONS);
        for (int[] size : new int[][] {{1080, 2400}, {720, 1600}, {1080, 1080}, {2400, 1080}, {5120, 1080}, {12000, 800}}) {
            String surface = size[0] + "x" + size[1];
            TouchLayoutImport.Result result = TouchLayoutImport.importFiles(Collections.singletonList(file), size[0], size[1]);
            assertEquals(surface, TouchLayout.MAX_BUTTONS, result.layout.buttons.size());
            assertTrue(surface, result.skipped.isEmpty());
            List<TouchLayout.Button> buttons = result.layout.buttons;
            for (TouchLayout.Button a : buttons) {
                assertTrue(surface + " " + a.name, a.width >= .04f && a.height >= .04f);
                assertTrue(surface + " " + a.name, a.x >= 0 && a.y >= 0 && a.x + a.width <= 1.00001f && a.y + a.height <= 1.00001f);
                for (TouchLayout.Button b : buttons) {
                    if (a == b) continue;
                    boolean overlap = a.x + a.width > b.x + 1e-4f && b.x + b.width > a.x + 1e-4f &&
                            a.y + a.height > b.y + 1e-4f && b.y + b.height > a.y + 1e-4f;
                    assertFalse(surface + ": " + a.name + " covers " + b.name, overlap);
                }
            }
        }
    }

    @Test public void unknownOrMixedFilesAreRejectedWithAReason() throws IOException {
        TouchLayout.InvalidLayoutException e = assertThrows(TouchLayout.InvalidLayoutException.class,
                () -> TouchLayoutImport.importFiles(Collections.singletonList("{\"hello\": 1}"), 2400, 1080));
        assertEquals(TouchLayout.InvalidLayoutException.Reason.NOT_JSON, e.reason);
        assertThrows(TouchLayout.InvalidLayoutException.class,
                () -> TouchLayoutImport.importFiles(Collections.singletonList("[1, 2]"), 2400, 1080));
        String nightjar = TouchLayoutFixturesTest.fixture("v2-all-controls.json");
        String keys = artemis("wiki-custom-special-keys.json");
        assertThrows(TouchLayout.InvalidLayoutException.class,
                () -> TouchLayoutImport.importFiles(Arrays.asList(nightjar, keys), 2400, 1080));
        assertThrows(TouchLayout.InvalidLayoutException.class,
                () -> TouchLayoutImport.importFiles(Arrays.asList(keys, keys), 2400, 1080));
        assertEquals(TouchLayout.InvalidLayoutException.Reason.VERSION, assertThrows(TouchLayout.InvalidLayoutException.class,
                () -> TouchLayoutImport.importFiles(Collections.singletonList("{\"version\":9,\"buttons\":[]}"), 2400, 1080)).reason);
    }

    @Test public void moreThanThirtyTwoElementsKeepTheFirstAndListTheRest() {
        StringBuilder file = new StringBuilder("{");
        for (int i = 0; i < 34; i++) {
            if (i > 0) file.append(',');
            // Android key codes 29..62 cover letters and more; all are distinct.
            file.append("\"key_").append(29 + i).append("\":\"{\\\"LEFT\\\":").append(i * 60)
                    .append(",\\\"TOP\\\":10,\\\"WIDTH\\\":60,\\\"HEIGHT\\\":60,\\\"ENABLED\\\":true,\\\"HIDDEN\\\":false}\"");
        }
        file.append('}');
        TouchLayoutImport.Result result = TouchLayoutImport.importFiles(Collections.singletonList(file.toString()), 2400, 1080);
        assertEquals(TouchLayout.MAX_BUTTONS, result.layout.buttons.size());
        assertEquals(2, result.skipped.size());
        assertEquals(TouchLayoutImport.Skip.TOO_MANY, result.skipped.get(0).reason);
    }

    @Test public void virtualKeyNamesFollowArtemis() {
        assertEquals(0xA2, TouchLayoutImport.virtualKey("VK_LCONTROL"));
        assertEquals(0x43, TouchLayoutImport.virtualKey("VK_C"));
        assertEquals(0x7B, TouchLayoutImport.virtualKey("VK_F12"));
        assertEquals(0x87, TouchLayoutImport.virtualKey("VK_F24"));
        assertEquals(0x65, TouchLayoutImport.virtualKey("VK_NUMPAD5"));
        assertEquals(0xB3, TouchLayoutImport.virtualKey("VK_MEDIA_PLAY_PAUSE"));
        assertEquals(0xA2, TouchLayoutImport.virtualKey("0xA2"));
        assertEquals(-1, TouchLayoutImport.virtualKey("VK_NOPE"));
        assertEquals(-1, TouchLayoutImport.virtualKey("0x1FF"));
        assertEquals(-1, TouchLayoutImport.virtualKey("ctrl"));
    }
}
