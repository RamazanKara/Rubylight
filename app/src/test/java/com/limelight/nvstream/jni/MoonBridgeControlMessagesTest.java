package com.limelight.nvstream.jni;

import org.junit.Test;

import java.io.BufferedReader;
import java.io.InputStream;
import java.io.InputStreamReader;
import java.nio.charset.StandardCharsets;
import java.util.Map;
import java.util.TreeMap;

import static org.junit.Assert.*;

/**
 * The 3.0 control message types MoonBridge names, against src/test/resources/protocol/control-messages.txt.
 * nightjar-core's tests/control_messages_contract.rs checks the same file against rubylight_protocol, so the
 * Java constants and ids.rs cannot drift apart unnoticed.
 */
public class MoonBridgeControlMessagesTest {
    @Test
    public void theControlMessageTypesMatchRubylightProtocol() throws Exception {
        // Compile-time constants: reading them does not load the native library.
        Map<String, Integer> java = new TreeMap<>();
        java.put("CONTROL_MESSAGE_FEC_REPORT", MoonBridge.CONTROL_MESSAGE_FEC_REPORT);
        java.put("CONTROL_MESSAGE_USB", MoonBridge.CONTROL_MESSAGE_USB);
        java.put("CONTROL_MESSAGE_TEXT_INPUT", MoonBridge.CONTROL_MESSAGE_TEXT_INPUT);
        java.put("CONTROL_MESSAGE_RATE_CHANGE", MoonBridge.CONTROL_MESSAGE_RATE_CHANGE);
        java.put("CONTROL_MESSAGE_PIPELINE_RATES", MoonBridge.CONTROL_MESSAGE_PIPELINE_RATES);
        java.put("CONTROL_MESSAGE_KEYBOARD_LAYOUT", MoonBridge.CONTROL_MESSAGE_KEYBOARD_LAYOUT);
        java.put("CONTROL_MESSAGE_CURSOR_SHAPE_REQUEST", MoonBridge.CONTROL_MESSAGE_CURSOR_SHAPE_REQUEST);
        java.put("CONTROL_MESSAGE_CLIPBOARD_SYNC", MoonBridge.CONTROL_MESSAGE_CLIPBOARD_SYNC);

        Map<String, Integer> listed = new TreeMap<>();
        try (InputStream input = getClass().getResourceAsStream("/protocol/control-messages.txt")) {
            assertNotNull(input);
            BufferedReader reader = new BufferedReader(new InputStreamReader(input, StandardCharsets.UTF_8));
            for (String line; (line = reader.readLine()) != null; ) {
                if (line.isEmpty() || line.startsWith("#")) {
                    continue;
                }
                String[] fields = line.split(" ");
                assertEquals(line, 2, fields.length);
                assertTrue(line, fields[1].startsWith("0x"));
                assertNull(fields[0] + " is listed twice",
                        listed.put(fields[0], Integer.parseInt(fields[1].substring(2), 16)));
            }
        }
        assertEquals(listed, java);
    }
}
