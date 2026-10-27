package com.limelight.binding.input;

import android.app.Application;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;
import java.io.IOException;
import java.io.InputStream;
import java.nio.charset.StandardCharsets;
import static org.junit.Assert.*;

/** Layouts saved or shared by earlier releases must load and save back byte for byte. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class TouchLayoutFixturesTest {
    static String fixture(String name) throws IOException {
        try (InputStream input = TouchLayoutFixturesTest.class.getResourceAsStream("/touch-layouts/" + name)) {
            assertNotNull(name, input);
            java.io.ByteArrayOutputStream bytes = new java.io.ByteArrayOutputStream();
            byte[] buffer = new byte[4096];
            for (int count; (count = input.read(buffer)) != -1; ) bytes.write(buffer, 0, count);
            return new String(bytes.toByteArray(), StandardCharsets.UTF_8).replace("\r\n", "\n").trim();
        }
    }

    @Test public void versionOneLayoutsRoundTripUnchanged() throws IOException {
        for (String name : new String[] {"v1-keyboard-mouse.json", "v1-empty.json"}) {
            String saved = fixture(name);
            assertEquals(name, saved, TouchLayout.parse(saved).encode());
        }
    }

    @Test public void versionTwoKeepsEveryControlTypeAndItsSettings() throws IOException {
        String saved = fixture("v2-all-controls.json");
        TouchLayout layout = TouchLayout.parse(saved);
        assertEquals(saved, layout.encode());
        TouchLayout.Kind[] kinds = {TouchLayout.Kind.KEYBOARD, TouchLayout.Kind.MOUSE, TouchLayout.Kind.GAMEPAD,
                TouchLayout.Kind.TRIGGER, TouchLayout.Kind.STICK, TouchLayout.Kind.DPAD, TouchLayout.Kind.WASD};
        for (int i = 0; i < kinds.length; i++) assertEquals(kinds[i], layout.buttons.get(i).kind);
        assertEquals(com.limelight.nvstream.input.ControllerPacket.A_FLAG, layout.buttons.get(2).gamepadButton);
        assertEquals(TouchLayout.RIGHT, layout.buttons.get(3).side);
        TouchLayout.Button stick = layout.buttons.get(4);
        assertTrue(stick.floating);
        assertEquals(.125f, stick.deadzone, 0);
        TouchLayout.Button walk = layout.buttons.get(6);
        assertArrayEquals(new int[] {51, 29, 47, 32}, walk.keys());
        assertEquals(45, walk.diagonal, 0);
        assertEquals(.25f, walk.deadzone, 0);
        // Moving keeps a control's own settings.
        TouchLayout.Button moved = walk.move(.1f, .2f);
        assertEquals(TouchLayout.Kind.WASD, moved.kind);
        assertArrayEquals(walk.keys(), moved.keys());
        assertEquals(.1f, moved.x, 0);
    }

    @Test public void onlyLayoutsWithNewControlsSaveAsVersionTwo() throws IOException {
        TouchLayout keysOnly = TouchLayout.parse(fixture("v2-all-controls.json"));
        java.util.List<TouchLayout.Button> first = keysOnly.buttons.subList(0, 2);
        assertTrue(new TouchLayout(first).encode().contains("\"version\": 1"));
        assertTrue(keysOnly.encode().contains("\"version\": 2"));
    }

    @Test public void versionOneFilesStillReadNamedKeys() {
        TouchLayout layout = TouchLayout.parse("{\"version\":1,\"buttons\":[{\"id\":\"jump\",\"name\":\"Jump\"," +
                "\"type\":\"keyboard\",\"toggle\":false,\"x\":0.1,\"y\":0.1,\"width\":0.2,\"height\":0.2,\"keys\":\"SPACE\"}]}");
        assertArrayEquals(new int[] {62}, layout.buttons.get(0).keys());
    }

    private static TouchLayout.InvalidLayoutException rejected(String json) {
        try {
            TouchLayout.parse(json);
        } catch (TouchLayout.InvalidLayoutException e) {
            return e;
        }
        throw new AssertionError("Accepted " + json);
    }

    @Test public void theValidatorSaysWhatIsWrong() throws IOException {
        assertEquals(TouchLayout.InvalidLayoutException.Reason.NOT_JSON, rejected("not json").reason);
        TouchLayout.InvalidLayoutException version = rejected("{\"version\":3,\"buttons\":[]}");
        assertEquals(TouchLayout.InvalidLayoutException.Reason.VERSION, version.reason);
        assertEquals("3", version.detail);
        // A version 1 file cannot carry the controls version 2 introduced.
        String stickInVersionOne = fixture("v2-all-controls.json").replace("\"version\": 2", "\"version\": 1");
        TouchLayout.InvalidLayoutException control = rejected(stickInVersionOne);
        assertEquals(TouchLayout.InvalidLayoutException.Reason.CONTROL, control.reason);
        assertEquals(3, control.control);
        assertEquals("A", control.detail);
        String saved = fixture("v2-all-controls.json");
        assertEquals(5, rejected(saved.replace("\"deadzone\": 0.125", "\"deadzone\": 0.9")).control);
        assertEquals(7, rejected(saved.replace("\"diagonal\": 45", "\"diagonal\": 120")).control);
        assertEquals(7, rejected(saved.replace("\"down\": \"S\"", "\"down\": \"W\"")).control);
        assertEquals(3, rejected(saved.replace("\"button\": \"A\"", "\"button\": \"Z\"")).control);
        assertEquals(4, rejected(saved.replace("\"side\": \"right\"", "\"side\": \"middle\"")).control);
        assertEquals(TouchLayout.InvalidLayoutException.Reason.DUPLICATE,
                rejected(saved.replace("\"id\": \"walk\"", "\"id\": \"jump\"")).reason);
        // A WASD stick needs four keys.
        assertThrows(IllegalArgumentException.class, () -> TouchLayout.Button.wasd("w", "W", new int[] {51, 29, 47},
                false, .1f, 45, 0, 0, .2f, .2f));
    }
}
