package com.limelight.binding.video;

import org.junit.Test;

import java.io.File;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.util.regex.Matcher;
import java.util.regex.Pattern;

import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertTrue;

/** The VRR notes stay on the overlay's status line for the whole stream, so they are labels. */
public class VrrStatusNoteTest {
    private static final String[] NOTES = {"stream_vrr_no_arr", "stream_vrr_old_android"};

    @Test
    public void theVrrNotesOnTheStatusLineAreShortLabelsInEveryLanguage() throws Exception {
        File[] folders = new File("src/main/res").listFiles((dir, name) -> name.startsWith("values"));
        assertNotNull(folders);
        int checked = 0;
        for (File folder : folders) {
            File file = new File(folder, "expected_video.xml");
            if (!file.isFile()) continue;
            String xml = new String(Files.readAllBytes(file.toPath()), StandardCharsets.UTF_8);
            for (String name : NOTES) {
                Matcher m = Pattern.compile("<string name=\"" + name + "\">(.*?)</string>", Pattern.DOTALL).matcher(xml);
                assertTrue(folder.getName() + " has " + name, m.find());
                String text = m.group(1);
                assertTrue(folder.getName() + "/" + name + " is a label: " + text, text.length() <= 40);
                assertFalse(folder.getName() + "/" + name + " names no host: " + text, text.contains("Rubylight"));
                checked++;
            }
        }
        assertTrue("every language checked", checked >= 2 * 15);
    }
}
