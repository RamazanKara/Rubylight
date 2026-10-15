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
}
