package com.limelight.mic;

import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

import java.io.File;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;

public class MicOnlyNotificationIconTest {
    // A theme attribute in a notification's small icon resolves to nothing there: the icon showed blank.
    @Test
    public void theNotificationIconIsPlainWhiteWithNoThemeColour() throws Exception {
        String icon = new String(Files.readAllBytes(new File("src/main/res/drawable/ic_stat_mic.xml").toPath()),
                StandardCharsets.UTF_8);
        assertFalse(icon, icon.contains("?attr/"));
        assertTrue(icon, icon.contains("#FFFFFFFF"));
        String service = new String(Files.readAllBytes(
                new File("src/main/java/com/limelight/mic/MicOnlyService.java").toPath()), StandardCharsets.UTF_8);
        assertTrue(service.contains("setSmallIcon(R.drawable.ic_stat_mic)"));
    }
}
